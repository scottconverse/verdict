//! `vdt hook <host> <event>`: read the payload on stdin, record it, always let the agent continue.
//!
//! The contract, in shadow mode:
//! * the process exits 0 whatever happens (bad JSON, unknown host or event, no ledger, a locked
//!   database, a crash, a timeout);
//! * it prints nothing on stdout (Codex reads JSON decisions there; Claude Code reads exit 2 and
//!   stdout JSON), and nothing on stderr;
//! * when it can, it writes a `check_unavailable` record saying why it could not record;
//! * the whole thing, including that record, ends well inside the 20 seconds the design allows
//!   for a Stop hook (15 s of work plus at most 2 s for the record; other events get 8 s).

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::payload::{EventKind, Host, parse_payload, peek_context};
use crate::recorder::{Options, record_event, record_unavailable};

/// Work budget for a Stop hook.
pub const STOP_BUDGET: Duration = Duration::from_secs(15);
/// Work budget for every other event.
pub const OTHER_BUDGET: Duration = Duration::from_secs(8);
/// Extra time allowed for writing the `check_unavailable` record after a failure.
pub const UNAVAILABLE_BUDGET: Duration = Duration::from_secs(2);
/// The most stdin is read; a larger payload is treated as unreadable.
pub const MAX_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Default)]
struct Context {
    cwd: Option<PathBuf>,
    session_id: Option<String>,
}

/// The budget for `event`, honoring `VDT_HOOK_BUDGET_MS` (tests) but never beyond the Stop budget.
pub fn budget_for(event: &str) -> Duration {
    if let Some(ms) = std::env::var("VDT_HOOK_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        return Duration::from_millis(ms).min(STOP_BUDGET);
    }
    if EventKind::parse(event) == Some(EventKind::Stop) {
        STOP_BUDGET
    } else {
        OTHER_BUDGET
    }
}

/// Entry point for `vdt hook ...`: `args` are the words after `hook`. Never fails and never
/// prints; the caller exits 0 afterwards.
pub fn run_hook_cli(args: &[String]) {
    std::panic::set_hook(Box::new(|_| {}));
    let host = args.first().map(String::as_str).unwrap_or("");
    let event = args.get(1).map(String::as_str).unwrap_or("");
    run_hook(
        host,
        event,
        Box::new(std::io::stdin()),
        Options::default(),
        budget_for(event),
    );
}

/// Run one hook. See the module documentation for the contract.
pub fn run_hook(
    host_arg: &str,
    event_arg: &str,
    stdin: Box<dyn Read + Send>,
    opts: Options,
    budget: Duration,
) {
    let ctx = Arc::new(Mutex::new(Context::default()));
    let (tx, rx) = mpsc::channel();
    {
        let ctx = Arc::clone(&ctx);
        let host_arg = host_arg.to_string();
        let event_arg = event_arg.to_string();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work(&host_arg, &event_arg, stdin, &opts, &ctx)
            }));
            let _ = tx.send(match result {
                Ok(r) => r,
                Err(_) => Err("the adapter crashed while handling the event".to_string()),
            });
        });
    }
    let failure = match rx.recv_timeout(budget) {
        Ok(Ok(())) => return,
        Ok(Err(reason)) => reason,
        Err(mpsc::RecvTimeoutError::Timeout) => format!(
            "the adapter did not finish within {} ms and was abandoned so the agent could continue",
            budget.as_millis()
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            "the adapter stopped without a result".to_string()
        }
    };
    let (cwd, sid) = match ctx.lock() {
        Ok(c) => (c.cwd.clone(), c.session_id.clone()),
        Err(_) => (None, None),
    };
    let label = if host_arg.trim().is_empty() {
        "unknown-host".to_string()
    } else {
        host_arg.to_string()
    };
    let reason = format!("{event_arg}: {failure}");
    let (tx2, rx2) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx2.send(record_unavailable(
            &label,
            cwd.as_deref(),
            sid.as_deref(),
            &reason,
        ));
    });
    let _ = rx2.recv_timeout(UNAVAILABLE_BUDGET);
}

fn work(
    host_arg: &str,
    event_arg: &str,
    stdin: Box<dyn Read + Send>,
    opts: &Options,
    ctx: &Mutex<Context>,
) -> Result<(), String> {
    let mut bytes = Vec::new();
    stdin
        .take(MAX_PAYLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read the payload: {e}"))?;
    let (cwd, sid) = peek_context(&bytes);
    if let Ok(mut c) = ctx.lock() {
        c.cwd = cwd;
        c.session_id = sid;
    }
    if bytes.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err("the payload was larger than 16 MB and was not read".into());
    }
    let host = Host::parse(host_arg).ok_or_else(|| format!("unknown host {host_arg:?}"))?;
    let kind = EventKind::parse(event_arg).ok_or_else(|| format!("unknown event {event_arg:?}"))?;
    let ev = parse_payload(host, kind, &bytes)?;
    record_event(&ev, opts)
}
