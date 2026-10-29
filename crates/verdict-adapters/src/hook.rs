//! `vdt hook <host> <event>`: read the payload on stdin, record it, always let the agent continue.
//!
//! The contract:
//! * the process exits 0 whatever happens (bad JSON, unknown host or event, no ledger, a locked
//!   database, a crash, a timeout);
//! * it prints nothing on stdout for an event with nothing to say, and one JSON decision when
//!   enforce mode holds the agent (Claude Code and Codex; other hosts record and reply nothing);
//! * when it can, it writes a `check_unavailable` record saying why it could not record;
//! * the Stop hook's work budget is the policy's `gate.stop_timeout` (the design's configurable
//!   timeout, 20s by default), not a fixed constant. A test can pin it with
//!   `VDT_HOOK_BUDGET_MS`. Every event other than Stop gets a short budget.

use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::payload::{EventKind, Host, parse_payload, peek_context};
use crate::recorder::{HookReply, Options, record_event, record_unavailable};

/// The default work budget for a Stop hook when the policy does not name one.
pub const STOP_BUDGET: Duration = Duration::from_secs(20);
/// Work budget for every event that is not Stop.
pub const OTHER_BUDGET: Duration = Duration::from_secs(8);
/// The longest a Stop hook will wait for a configured `gate.stop_timeout`.
pub const MAX_STOP_BUDGET: Duration = Duration::from_secs(600);
/// Extra time allowed for writing the `check_unavailable` record after a failure.
pub const UNAVAILABLE_BUDGET: Duration = Duration::from_secs(2);

/// How long the hook waits for the "unavailable" record to be written. The default is
/// [`UNAVAILABLE_BUDGET`]; a test may raise it with `VDT_HOOK_RECORD_BUDGET_MS` so that an assertion
/// about the record is not a race with a loaded machine's filesystem. This never changes how long an
/// agent is held — the payload budget is separate and unaffected.
fn record_budget() -> Duration {
    std::env::var("VDT_HOOK_RECORD_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(UNAVAILABLE_BUDGET)
}
/// The most stdin is read; a larger payload is treated as unreadable.
pub const MAX_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Default)]
struct Context {
    cwd: Option<PathBuf>,
    session_id: Option<String>,
}

/// The budget for `event` when the payload is not available yet (the command line's default).
/// `run_hook` refines it from the policy once the payload's project is known.
pub fn budget_for(event: &str) -> Duration {
    if let Some(ms) = env_budget_ms() {
        return Duration::from_millis(ms).min(MAX_STOP_BUDGET);
    }
    if EventKind::parse(event) == Some(EventKind::Stop) {
        STOP_BUDGET
    } else {
        OTHER_BUDGET
    }
}

fn env_budget_ms() -> Option<u64> {
    std::env::var("VDT_HOOK_BUDGET_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// Entry point for `vdt hook ...`: `args` are the words after `hook`. It prints one JSON decision
/// when enforce mode holds the agent, prints nothing otherwise, and the caller exits 0.
pub fn run_hook_cli(args: &[String]) {
    std::panic::set_hook(Box::new(|_| {}));
    let host = args.first().map(String::as_str).unwrap_or("");
    let event = args.get(1).map(String::as_str).unwrap_or("");
    if let Some(text) = run_hook(
        host,
        event,
        Box::new(std::io::stdin()),
        Options::default(),
        budget_for(event),
    ) {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

/// The JSON a host reads, for a reply that holds the agent. Only hosts whose output shape is known
/// get one; the others keep recording in shadow and answer nothing.
fn reply_json(host: Host, kind: EventKind, reply: &HookReply) -> Option<String> {
    let shape = match reply {
        HookReply::Continue => return None,
        HookReply::Deny(reason) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": kind.as_str(),
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
        HookReply::SendBack(reason) => serde_json::json!({
            "decision": "block",
            "reason": reason,
        }),
    };
    match host {
        Host::Claude => Some(shape.to_string()),
        // The Codex hook docs do not state the output shape in the parts read for this build; the
        // same JSON is sent and the real-host behaviour is labeled unverified in docs/HOSTS.md.
        Host::Codex => Some(shape.to_string()),
        // Cursor, Antigravity and DSH record only: this build does not claim their reply shape.
        Host::Cursor | Host::Antigravity | Host::Dsh => None,
    }
}

/// The Stop budget from the policy in force for the project at `cwd`, clamped to what a hook may
/// wait. Anything unusable falls back to the caller's default.
fn policy_stop_budget(cwd: Option<&std::path::Path>, fallback: Duration) -> Duration {
    let Some(cwd) = cwd else {
        return fallback;
    };
    let root = verdict_core::find_project_root(cwd);
    let Ok((policy, _)) = verdict_core::verdict::load_policy(&root, None) else {
        return fallback;
    };
    let configured = verdict_engine::stop_timeout(&policy, fallback);
    if configured.is_zero() {
        fallback
    } else {
        configured.min(MAX_STOP_BUDGET)
    }
}

/// Run one hook. Returns the JSON to print, if any. See the module documentation for the contract.
pub fn run_hook(
    host_arg: &str,
    event_arg: &str,
    stdin: Box<dyn Read + Send>,
    opts: Options,
    budget: Duration,
) -> Option<String> {
    let host = Host::parse(host_arg);
    let kind = EventKind::parse(event_arg);

    let hook_start = std::time::Instant::now();
    // Read the payload first (it names the project, which is where the configured Stop timeout
    // comes from) but never longer than the budget: a host that never closes stdin must not hang
    // the agent.
    let (tx_read, rx_read) = mpsc::channel();
    let spawned = std::thread::Builder::new().spawn(move || {
        let mut stdin = stdin;
        let mut bytes = Vec::new();
        let read = stdin
            .by_ref()
            .take(MAX_PAYLOAD_BYTES + 1)
            .read_to_end(&mut bytes);
        let _ = tx_read.send((read, bytes));
    });
    if spawned.is_err() {
        return None;
    }
    let (read, bytes) = match rx_read.recv_timeout(budget) {
        Ok(v) => v,
        Err(_) => {
            let ctx = Arc::new(Mutex::new(Context::default()));
            return fail(
                host_arg,
                event_arg,
                host,
                kind,
                &ctx,
                format!(
                    "the payload did not arrive within {} ms, so the event was not read",
                    budget.as_millis()
                ),
            );
        }
    };
    let ctx = Arc::new(Mutex::new(Context::default()));
    let (cwd, sid) = peek_context(&bytes);
    if let Ok(mut c) = ctx.lock() {
        c.cwd = cwd.clone();
        c.session_id = sid;
    }
    let failure = match read {
        Err(e) => Some(format!("cannot read the payload: {e}")),
        Ok(_) if bytes.len() as u64 > MAX_PAYLOAD_BYTES => {
            Some("the payload was larger than 16 MB and was not read".into())
        }
        Ok(_) => None,
    };
    if let Some(reason) = failure {
        return fail(host_arg, event_arg, host, kind, &ctx, reason);
    }

    // The Stop hook uses the policy's own `gate.stop_timeout`; a test can pin it with
    // `VDT_HOOK_BUDGET_MS` (the `budget` argument). Other events keep their short budget. What was
    // already spent reading the payload comes out of it.
    let reading = hook_start.elapsed();
    let left = budget.saturating_sub(reading);
    if left.is_zero() {
        return fail(
            host_arg,
            event_arg,
            host,
            kind,
            &ctx,
            "the payload took the whole budget, so nothing else was attempted".into(),
        );
    }
    let work_budget = match kind {
        Some(EventKind::Stop) if env_budget_ms().is_none() => {
            policy_stop_budget(cwd.as_deref(), left).min(left)
        }
        _ => left,
    };

    // Keep a reserve for the failure record; the verification is bounded by this instant.
    let opts = Options {
        deadline: Some(hook_start + work_budget.saturating_sub(Duration::from_millis(750))),
        ..opts
    };
    let (tx, rx) = mpsc::channel();
    {
        let host_arg = host_arg.to_string();
        let event_arg = event_arg.to_string();
        // Builder::spawn returns an error instead of panicking when the OS refuses a thread,
        // so even that case ends quietly with exit 0.
        let spawned = std::thread::Builder::new().spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work(&host_arg, &event_arg, &bytes, &opts)
            }));
            let _ = tx.send(match result {
                Ok(r) => r,
                Err(_) => Err("the adapter crashed while handling the event".to_string()),
            });
        });
        if spawned.is_err() {
            return None;
        }
    }
    match rx.recv_timeout(work_budget) {
        Ok(Ok(reply)) => match (host, kind) {
            (Some(h), Some(k)) => reply_json(h, k, &reply),
            _ => None,
        },
        Ok(Err(reason)) => fail(host_arg, event_arg, host, kind, &ctx, reason),
        Err(mpsc::RecvTimeoutError::Timeout) => fail(
            host_arg,
            event_arg,
            host,
            kind,
            &ctx,
            format!(
                "the adapter did not finish within {} ms and was abandoned so the agent could continue",
                work_budget.as_millis()
            ),
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => fail(
            host_arg,
            event_arg,
            host,
            kind,
            &ctx,
            "the adapter stopped without a result".to_string(),
        ),
    }
}

/// Record why the event could not be handled, best effort, and answer nothing.
fn fail(
    host_arg: &str,
    event_arg: &str,
    _host: Option<Host>,
    _kind: Option<EventKind>,
    ctx: &Arc<Mutex<Context>>,
    failure: String,
) -> Option<String> {
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
    let spawned = std::thread::Builder::new().spawn(move || {
        let _ = tx2.send(record_unavailable(
            &label,
            cwd.as_deref(),
            sid.as_deref(),
            &reason,
        ));
    });
    if spawned.is_ok() {
        let _ = rx2.recv_timeout(record_budget());
    }
    None
}

fn work(
    host_arg: &str,
    event_arg: &str,
    bytes: &[u8],
    opts: &Options,
) -> Result<HookReply, String> {
    let host = Host::parse(host_arg).ok_or_else(|| format!("unknown host {host_arg:?}"))?;
    let kind = EventKind::parse(event_arg).ok_or_else(|| format!("unknown event {event_arg:?}"))?;
    let ev = parse_payload(host, kind, bytes)?;
    record_event(&ev, opts)
}
