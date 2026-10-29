//! Verdict's LLM desks: seven rubrics, a provider interface, and the dispatch that turns a desk's
//! answer into ledger findings with citations.
//!
//! * The rubrics are files in this crate (`desks/<name>.md`, embedded with `include_str!`), so a
//!   build, an install and a run never depend on a skill folder on this machine.
//! * A desk is a worker over the ledger, never over the builder's notes. The prompt carries the
//!   rubric, the policy facts and a read-only view of the ledger.
//! * A provider is any command that reads a prompt as JSON on stdin and writes one JSON answer on
//!   stdout. A local or cloud model, a gateway or an existing routing skill can all be a provider;
//!   none of them is required.
//! * Above the policy's cross-vendor lane the desks are dispatched only to a provider whose vendor
//!   differs from the builder's. With none available the run is refused, unless the global policy
//!   downgraded the refusal to warn-only; the receipt then says `single_vendor`.
//! * A finding with no citation that resolves against the ledger or the tree is stored as a
//!   hypothesis, never as a finding.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use verdict_core::lane::{lane_desks, lane_name};
use verdict_core::model::{CheckOutcome, Finding, Record, Severity, Tier, Writer};
use verdict_core::pointer::EvidencePointer;
use verdict_core::{Lane, Ledger, Policy};

/// Every desk, in the order the design lists them.
pub const DESK_NAMES: [&str; 7] = [
    "engineer", "docs", "qa", "ux", "security", "legal", "refuter",
];

/// The packaged rubric for `desk`, if it is one of the seven.
pub fn rubric(desk: &str) -> Option<&'static str> {
    Some(match desk {
        "engineer" => include_str!("../desks/engineer.md"),
        "docs" => include_str!("../desks/docs.md"),
        "qa" => include_str!("../desks/qa.md"),
        "ux" => include_str!("../desks/ux.md"),
        "security" => include_str!("../desks/security.md"),
        "legal" => include_str!("../desks/legal.md"),
        "refuter" => include_str!("../desks/refuter.md"),
        _ => return None,
    })
}

/// What one desk run is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskOptions {
    /// The lane in force; it decides which desks run and whether cross-vendor is required.
    pub lane: Lane,
    /// How long one provider may take.
    pub timeout: Duration,
}

/// A provider the engine can call.
pub trait DeskProvider {
    /// Stable name, as the receipt records it.
    fn name(&self) -> &str;
    /// The vendor that serves the model. Never the host app or the transport.
    fn vendor(&self) -> &str;
    /// The model this provider asks for.
    fn model(&self) -> &str;
    /// The provider's own time limit, when it has one.
    fn timeout_secs(&self) -> Option<u64> {
        None
    }
    /// Ask it one question and return its raw answer text.
    fn ask(&self, prompt: &str, timeout: Duration) -> Result<String, String>;
}

/// A provider configured in the policy: a command that speaks the JSON protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProvider {
    /// Name.
    pub name: String,
    /// Vendor.
    pub vendor: String,
    /// The model to ask for.
    pub model: String,
    /// The program.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Its own time limit, if the policy gives one.
    pub timeout_secs: Option<u64>,
}

impl DeskProvider for CommandProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn vendor(&self) -> &str {
        &self.vendor
    }
    fn model(&self) -> &str {
        &self.model
    }
    fn timeout_secs(&self) -> Option<u64> {
        self.timeout_secs
    }
    fn ask(&self, prompt: &str, timeout: Duration) -> Result<String, String> {
        ask_command(self, prompt, timeout)
    }
}

/// A Windows Job Object holding the provider, so a timeout ends the provider and everything it
/// started. This is the same mechanism the checker exec path uses. Assignment happens right after
/// spawn and before the request is written, but a provider that starts a helper in its first
/// instants can predate job membership; the timeout path therefore also walks the tree with
/// `taskkill /T` while the provider is still alive (the early-helper regression covers that case).
/// If the job cannot be created or assigned, only the tree walk remains.
#[cfg(windows)]
struct ProviderJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProviderJob {
    fn new(child: &std::process::Child) -> Option<ProviderJob> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: plain Win32 calls on handles this function creates and owns; the structure is
        // zero-initialised and fully sized.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 || AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(ProviderJob(job))
        }
    }

    fn end(&self) {
        // SAFETY: a live job object this process created.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProviderJob {
    fn drop(&mut self) {
        // Closing the handle also ends whatever is left (KILL_ON_JOB_CLOSE); by then the provider
        // has ended or was terminated.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// End a provider's whole process tree, best effort. The provider is already killed; this stops
/// the helpers it started, which would otherwise keep the output pipes open past the deadline. The
/// checker exec path owns its own Job Object for the project's commands; a provider is not that
/// path, so this asks the OS to end the tree by the provider's pid.
fn kill_process_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // The provider was started in its own process group (see `ask_command`), whose id is its
        // pid, so this ends the provider and everything it started.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
    }
}

fn ask_command(p: &CommandProvider, prompt: &str, timeout: Duration) -> Result<String, String> {
    let started = Instant::now();
    let mut cmd = Command::new(&p.command);
    cmd.args(&p.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            // Own process group, so a timeout can end the provider and its helpers together.
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("provider {} could not start: {e}", p.name))?;
    // Assign the provider to a job object immediately after spawn, before the request is written,
    // so anything it starts after this point is in the job. A helper it starts in its first
    // instants can predate the assignment; the timeout path also walks the live process tree, and
    // the early-helper regression pins that. Without a job only the tree walk remains.
    #[cfg(windows)]
    let job = ProviderJob::new(&child);
    let mut stdin = child
        .stdin
        .take()
        .ok_or("provider stdin was not available")?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or("provider stdout was not available")?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or("provider stderr was not available")?;
    let request = json!({
        "protocol": "verdict-desk/1",
        "model": p.model,
        "prompt": prompt,
    })
    .to_string();
    // The request, both output streams and the wait all run at the same time. Waiting for exit
    // before reading the pipes is how a chatty provider deadlocks its parent.
    let (write_tx, write_rx) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(request.as_bytes());
        // Dropping stdin closes it, so a provider that reads to end sees EOF.
        let _ = write_tx.send(());
    });
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();
    let (err_tx, err_rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = out_tx.send(buf);
    });
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        let _ = err_tx.send(buf);
    });
    let mut timed_out = false;
    let mut status = None;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                status = Some(s);
                break;
            }
            Ok(None) => {}
            Err(e) => {
                // End the tree while the provider is still alive: a killed parent lets its
                // descendants reparent and escape a `/T`-style tree walk.
                #[cfg(windows)]
                if let Some(job) = &job {
                    job.end();
                }
                kill_process_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("provider {} could not be waited for: {e}", p.name));
            }
        }
        if started.elapsed() > timeout {
            timed_out = true;
            // End the whole tree first (job object, else a `/T` walk while the provider is still
            // alive), then the direct child, so a provider's helpers cannot keep the pipes open
            // past the deadline.
            #[cfg(windows)]
            if let Some(job) = &job {
                job.end();
            }
            kill_process_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if timed_out {
        // The tree has been ended, so the reader threads and the writer end on EOF/EPIPE. Do not
        // wait for them or for the pipe graces: the failure is already known and the run must stay
        // bounded by the deadline.
        return Err(format!(
            "provider {} did not answer within {} seconds",
            p.name,
            timeout.as_secs()
        ));
    }
    // The provider exited, but a helper it started can still hold the inherited stdin open, so the
    // writer thread would block forever. Wait only for the time left on this call's deadline; if
    // the request was never taken, end the tree and report the bounded failure.
    let remaining = timeout.saturating_sub(started.elapsed());
    if write_rx.recv_timeout(remaining).is_err() {
        #[cfg(windows)]
        if let Some(job) = &job {
            job.end();
        }
        kill_process_tree(child.id());
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!(
            "provider {} did not take its request within {} seconds",
            p.name,
            timeout.as_secs()
        ));
    }
    let _ = writer.join();
    // The pipes close once the provider (and anything it started) is gone. Keep that bounded too,
    // so a provider that leaves a helper behind cannot hold a successful call.
    let grace = Duration::from_secs(5);
    let out = out_rx.recv_timeout(grace);
    let err = err_rx.recv_timeout(grace);
    let out = out.map_err(|_| format!("provider {} output could not be read", p.name))?;
    let err = err.map_err(|_| format!("provider {} error output could not be read", p.name))?;
    let status = status.ok_or_else(|| format!("provider {} ended without a status", p.name))?;
    if !status.success() {
        let text = String::from_utf8_lossy(&err);
        return Err(format!(
            "provider {} exited with {}: {}",
            p.name,
            status,
            text.lines().next().unwrap_or("")
        ));
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// The providers a policy configures. A policy with none is valid: the desks then cannot run and
/// the run says so.
pub fn providers_from_policy(policy: &Policy) -> Result<Vec<CommandProvider>, String> {
    let Some(list) = policy.value.pointer("/vendors/providers") else {
        return Ok(Vec::new());
    };
    let Some(list) = list.as_array() else {
        return Err("vendors.providers must be a list".into());
    };
    let mut out = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let s = |k: &str| item.get(k).and_then(Value::as_str);
        let name = s("name").ok_or_else(|| format!("vendors.providers[{i}] has no name"))?;
        let vendor = s("vendor").ok_or_else(|| format!("provider {name} has no vendor"))?;
        let command = s("command").ok_or_else(|| format!("provider {name} has no command"))?;
        let model = s("model").unwrap_or("unspecified").to_string();
        let args = item
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        out.push(CommandProvider {
            name: name.to_string(),
            vendor: vendor.to_string(),
            model,
            command: command.to_string(),
            args,
            timeout_secs: item.get("timeout_secs").and_then(Value::as_u64),
        });
    }
    Ok(out)
}

/// One desk's receipt: what was asked of which provider, what came back, and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    /// The desk.
    pub desk: String,
    /// The provider.
    pub provider: String,
    /// The vendor that served it.
    pub vendor: String,
    /// The vendor the answer says served it, when the provider says.
    pub served_vendor: Option<String>,
    /// The model asked for.
    pub requested_model: String,
    /// The model the answer says served it, when the provider says.
    pub returned_model: Option<String>,
    /// Effective settings the answer reports (temperature, effort, and so on).
    pub settings: Option<Value>,
    /// `confirmed` (a served vendor was stated), `not_stated`, or `mismatch` when a stated vendor
    /// was the builder's on a run that required another vendor.
    pub identity: String,
    /// Findings written.
    pub findings: usize,
    /// Hypotheses written.
    pub hypotheses: usize,
    /// `ran`, `refused` or `failed`.
    pub outcome: String,
    /// The provider's usage block, verbatim, when it sent one.
    pub usage: Option<Value>,
    /// The receipt's ledger sequence number.
    pub check_seq: u64,
    /// Why it failed, when it did.
    pub error: Option<String>,
}

/// What a desk run came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The desks ran (possibly none were required in this lane).
    Ran {
        /// One receipt per desk.
        receipts: Vec<Receipt>,
        /// True when the cross-vendor rule was downgraded to warn-only.
        single_vendor: bool,
    },
    /// The run was refused: the policy requires another vendor and none can serve.
    Refused {
        /// Plain-words reason.
        reason: String,
        /// The desks that could not run.
        desks: Vec<String>,
    },
}

#[derive(Debug, Deserialize, Clone)]
struct ServedInfo {
    #[serde(default)]
    vendor: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    settings: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    #[serde(default)]
    findings: Vec<RawFinding>,
    #[serde(default)]
    hypotheses: Vec<String>,
    /// A desk may propose a checker run (a mutation cell or a firstrun cell). The desk never runs
    /// code itself; the proposal is recorded and the core may run a configured checker for it.
    #[serde(default)]
    proposals: Vec<Value>,
    #[serde(default)]
    usage: Option<Value>,
    #[serde(default)]
    model: Option<String>,
    /// What the provider says actually served the answer (the design's served identity).
    #[serde(default)]
    served: Option<ServedInfo>,
    /// Effective settings observed from the invocation itself, when the adapter reports them.
    #[serde(default)]
    settings: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RawFinding {
    severity: Severity,
    title: String,
    #[serde(default)]
    evidence: Vec<RawEvidence>,
    #[serde(default)]
    fix: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawEvidence {
    Record { record: u64 },
    FileLine { file: String, line: u64 },
}

/// Parse a provider's answer: JSON, optionally inside one Markdown code fence.
fn parse_answer(text: &str) -> Result<RawAnswer, String> {
    let t = text.trim();
    let t = if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        rest.trim_start()
            .strip_suffix("```")
            .ok_or("the answer's code fence is not closed")?
            .trim()
    } else {
        t
    };
    serde_json::from_str(t)
        .map_err(|e| format!("the answer is not the JSON the protocol asks for: {e}"))
}

fn bounded(mut s: String, max: usize) -> String {
    if s.len() > max {
        let cut = s
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= max)
            .last()
            .unwrap_or(0);
        s.truncate(cut);
        s.push_str("...");
    }
    s
}

/// The read-only ledger view a desk is handed. Claims, checks, findings, decisions: not the
/// project's files (the rubric says which file pointers to ask about) and never a private note.
pub fn ledger_view(ledger: &Ledger, session: &str) -> Result<String, String> {
    let mut out = String::new();
    for s in ledger
        .records_for_session(session)
        .map_err(|e| e.to_string())?
    {
        match &s.record {
            Record::Prompt {
                ordinal,
                text,
                agent,
                model,
            } => out.push_str(&format!(
                "PROMPT {ordinal} ({agent}/{model}): {}\n",
                bounded(text.clone(), 2000)
            )),
            Record::Claim {
                source,
                text,
                names,
                ..
            } => out.push_str(&format!(
                "CLAIM {} [{}]: {} | names: {}\n",
                s.seq,
                source.as_str(),
                bounded(text.clone(), 2000),
                names.join(", ")
            )),
            Record::Check {
                name,
                tier,
                outcome,
                detail,
                tree_state,
                ..
            } => {
                let mut line = format!(
                    "CHECK {} {name}: {} at tier {}",
                    s.seq,
                    outcome_str(*outcome),
                    tier.as_str()
                );
                if let Some(d) = detail {
                    line.push_str(&format!(" | {}", bounded(d.clone(), 1200)));
                }
                if let Some(t) = tree_state {
                    line.push_str(&format!(" | tree {t}"));
                }
                out.push_str(&line);
                out.push('\n');
            }
            Record::Finding(f) | Record::Hypothesis(f) => out.push_str(&format!(
                "{} {} {} [{}]: {} | evidence: {}\n",
                if matches!(s.record, Record::Finding(_)) {
                    "FINDING"
                } else {
                    "HYPOTHESIS"
                },
                s.seq,
                severity_name(f.severity),
                f.tier.as_str(),
                f.title,
                f.evidence
                    .iter()
                    .map(EvidencePointer::describe)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Record::Decision {
                decision,
                choice,
                reason,
                ..
            } => out.push_str(&format!(
                "DECISION {} {}: {}{}\n",
                s.seq,
                decision.as_str(),
                choice,
                reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            )),
            Record::CheckUnavailable { component, reason } => out.push_str(&format!(
                "UNAVAILABLE {} {component}: {}\n",
                s.seq,
                bounded(reason.clone(), 800)
            )),
            Record::RefusedWrite {
                attempted_kind,
                attempted_decision,
                ..
            } => out.push_str(&format!(
                "REFUSED WRITE {}: agent tried to write {} {}\n",
                s.seq,
                attempted_kind.as_str(),
                attempted_decision.map(|d| d.as_str()).unwrap_or("")
            )),
            Record::Verdict { word, .. } => {
                out.push_str(&format!("VERDICT {} {}\n", s.seq, verdict_word(*word)))
            }
            _ => {}
        }
        if out.len() > 24_000 {
            out.push_str("... (the ledger view was cut here; ask about one record at a time)\n");
            break;
        }
    }
    Ok(out)
}

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Blocker => "blocker",
        Severity::Critical => "critical",
        Severity::Major => "major",
        Severity::Minor => "minor",
        Severity::Nit => "nit",
    }
}

fn verdict_word(w: verdict_core::VerdictWord) -> &'static str {
    match w {
        verdict_core::VerdictWord::Failed => "FAILED",
        verdict_core::VerdictWord::NeedsYou => "NEEDS YOU",
        verdict_core::VerdictWord::Verified => "VERIFIED",
        verdict_core::VerdictWord::ClearToShip => "CLEAR TO SHIP",
        verdict_core::VerdictWord::Partial => "PARTIAL",
        verdict_core::VerdictWord::Unverified => "UNVERIFIED",
    }
}

fn outcome_str(o: CheckOutcome) -> &'static str {
    match o {
        CheckOutcome::Ran => "ran",
        CheckOutcome::FailedToRun => "failed to run",
        CheckOutcome::SkippedByPolicy => "skipped by policy",
    }
}

/// The prompt handed to a desk: its rubric, the policy facts that bound the answer, the ledger
/// view, and the artifacts the design assigns to that desk.
pub fn desk_prompt(desk: &str, rubric_text: &str, lane: Lane, view: &str, focus: &str) -> String {
    format!(
        "You are the {desk} desk.\n\n{rubric_text}\n\n## This run\n\nLane: {}. You may report at \
         most five findings in the report body; anything else becomes a hypothesis. Cite ledger \
         records as {{\"record\": N}} and code as {{\"file\": \"path\", \"line\": N}}. Answer with \
         one JSON object and nothing else.\n\n## What this desk reads\n\n{focus}\n\n## The ledger\n\n{view}\n",
        lane_name(lane)
    )
}

/// The desk-specific artifacts the design assigns, read from the ledger and the project. The
/// generic ledger view is the same for every desk; this is what makes each desk's input targeted.
pub fn desk_focus(
    desk: &str,
    ledger: &Ledger,
    session: &str,
    root: &Path,
) -> Result<String, String> {
    let records = ledger
        .records_for_session(session)
        .map_err(|e| e.to_string())?;
    let mut out = String::new();
    let findings_with = |prefixes: &[&str], out: &mut String| {
        for s in &records {
            if let Record::Finding(f) | Record::Hypothesis(f) = &s.record
                && prefixes.iter().any(|p| f.id.starts_with(p))
            {
                out.push_str(&format!(
                    "- {} {} [{}]: {} (record {})\n",
                    if f.evidence.is_empty() {
                        "HYPOTHESIS"
                    } else {
                        "FINDING"
                    },
                    severity_name(f.severity),
                    f.tier.as_str(),
                    f.title,
                    s.seq
                ));
            }
        }
    };
    let checks_named = |names: &[&str], out: &mut String| {
        for s in &records {
            if let Record::Check {
                name,
                outcome,
                tier,
                detail,
                ..
            } = &s.record
                && names.contains(&name.as_str())
            {
                out.push_str(&format!(
                    "- CHECK {name} ({}, {}): {} (record {})\n",
                    outcome_str(*outcome),
                    tier.as_str(),
                    detail.as_deref().unwrap_or("no detail"),
                    s.seq
                ));
            }
        }
    };
    match desk {
        "engineer" => {
            out.push_str("What you read: the tree, wiring findings and exec output hashes.\n");
            findings_with(&["wiring:"], &mut out);
            checks_named(&["exec"], &mut out);
            let manifests = manifests_in(root);
            out.push_str(&format!(
                "- Manifests and dependency files: {}\n",
                manifests.join(", ")
            ));
        }
        "docs" => {
            out.push_str("What you read: the README and other docs, claims_vs_code findings, release findings.\n");
            findings_with(&["claims_vs_code:", "release:"], &mut out);
            checks_named(&["release"], &mut out);
            out.push_str(&format!("- README excerpt:\n{}\n", readme_excerpt(root)));
        }
        "qa" => {
            out.push_str("What you read: exec and firstrun records, tests_weakened findings.\n");
            findings_with(&["tests_weakened:", "exec:", "firstrun:"], &mut out);
            checks_named(&["exec", "firstrun"], &mut out);
        }
        "ux" => {
            out.push_str("What you read: firstrun screenshot/log evidence and wiring paths.\n");
            findings_with(&["wiring:", "firstrun:"], &mut out);
            checks_named(&["firstrun"], &mut out);
        }
        "security" => {
            out.push_str("What you read: regressions findings, dependency files and the tree.\n");
            findings_with(&["regressions:", "unfinished:"], &mut out);
            out.push_str(&format!(
                "- Dependency and license files: {}\n",
                manifests_in(root).join(", ")
            ));
        }
        "legal" => {
            out.push_str("What you read: package manifests, LICENSE files and release findings.\n");
            for f in ["LICENSE", "NOTICE", "COPYING", "LICENSE.md"] {
                out.push_str(&format!(
                    "- {f}: {}\n",
                    if root.join(f).exists() {
                        "present"
                    } else {
                        "absent"
                    }
                ));
            }
            out.push_str(&format!(
                "- Dependency manifests: {}\n",
                manifests_in(root).join(", ")
            ));
            findings_with(&["release:"], &mut out);
        }
        "refuter" => {
            out.push_str(
                "What you read: the full ledger. Refute, do not confirm: try to break every \
                 CONNECTED wiring result and every clean exec result.\n",
            );
            for s in &records {
                if let Record::Check {
                    name,
                    tier,
                    outcome,
                    ..
                } = &s.record
                    && *outcome == CheckOutcome::Ran
                    && matches!(tier, Tier::Ran | Tier::Browsed | Tier::Static)
                {
                    out.push_str(&format!(
                        "- CLEAN RESULT TO REFUTE: {name} at {} (record {})\n",
                        tier.as_str(),
                        s.seq
                    ));
                }
            }
        }
        _ => out.push_str("What you read: the ledger.\n"),
    }
    if out.len() > 12_000 {
        let cut = out.char_indices().nth(12_000).map_or(out.len(), |(i, _)| i);
        out.truncate(cut);
        out.push_str("...\n");
    }
    Ok(out)
}

/// Dependency and license files in the project root (single folder, no walk).
fn manifests_in(root: &Path) -> Vec<String> {
    const NAMES: [&str; 8] = [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "pyproject.toml",
        "requirements.txt",
        "go.mod",
    ];
    let mut out: Vec<String> = NAMES
        .iter()
        .filter(|n| root.join(n).exists())
        .map(|n| n.to_string())
        .collect();
    if out.is_empty() {
        out.push("(none found in the project root)".into());
    }
    out
}

fn readme_excerpt(root: &Path) -> String {
    for name in ["README.md", "README", "readme.md"] {
        if let Ok(text) = std::fs::read_to_string(root.join(name)) {
            return bounded(text, 4000);
        }
    }
    "(no README in the project root)".into()
}

fn desk_hash(desk: &str, title: &str, evidence: &[EvidencePointer]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(desk.as_bytes());
    h.update(b"\0");
    h.update(title.as_bytes());
    for e in evidence {
        h.update(b"\0");
        h.update(e.describe().as_bytes());
    }
    h.finalize().to_hex().to_string()[..8].to_string()
}

fn record_evidence(ledger: &Ledger, seq: u64) -> Option<EvidencePointer> {
    let stored = ledger.get(seq).ok().flatten()?;
    let body = serde_json::to_vec(&stored.record).ok()?;
    Some(EvidencePointer::command(
        &format!("ledger record {seq}"),
        &body,
    ))
}

/// Run the lane's desks. `providers` is the caller's list (from the policy, or a test's fakes).
pub fn run(
    ledger: &mut Ledger,
    root: &Path,
    session: &str,
    policy: &Policy,
    opts: &DeskOptions,
    providers: &[Box<dyn DeskProvider>],
) -> Result<Outcome, String> {
    let listed = lane_desks(policy, opts.lane);
    let desks: Vec<String> = if listed.iter().any(|d| d == "*") {
        DESK_NAMES.iter().map(|s| s.to_string()).collect()
    } else {
        listed
    };
    if desks.is_empty() {
        return Ok(Outcome::Ran {
            receipts: Vec::new(),
            single_vendor: false,
        });
    }
    let policy_version = policy.version;
    let threshold = policy
        .value
        .pointer("/vendors/cross_vendor_required_from_lane")
        .and_then(Value::as_str)
        .and_then(verdict_core::lane::parse_lane)
        .unwrap_or(Lane::Standard);
    let builder_vendor = ledger
        .records_for_session(session)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find_map(|s| match s.record {
            Record::Session { vendor, .. } => Some(vendor),
            _ => None,
        })
        .unwrap_or_default();
    let cross_required = opts.lane >= threshold;

    // A project may not set the global warn-only downgrade for itself. Setting it to `refuse` (the
    // default) is not a downgrade and is allowed.
    for src in &policy.sources {
        if let verdict_core::policy::PolicySource::Project(path) = src {
            let tries_warn = std::fs::read_to_string(path)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| {
                    v.pointer("/vendors/single_vendor")
                        .and_then(Value::as_str)
                        .map(|s| s == "warn")
                })
                .unwrap_or(false);
            if tries_warn {
                let reason =
                    "single_vendor may only be set in the global policy, never in a project's \
                     verdict.json"
                        .to_string();
                for desk in &desks {
                    ledger
                        .append(
                            Writer::Desk,
                            Some(session),
                            Record::CheckUnavailable {
                                component: format!("desk:{desk}"),
                                reason: reason.clone(),
                            },
                        )
                        .map_err(|e| e.to_string())?;
                }
                return Ok(Outcome::Refused { reason, desks });
            }
        }
    }
    let warn_only = policy
        .value
        .pointer("/vendors/single_vendor")
        .and_then(Value::as_str)
        == Some("warn");

    let mut order: Vec<&Box<dyn DeskProvider>> = providers.iter().collect();
    // The policy's `vendors.verifier_order` is a preference list of vendors (or provider names);
    // then a different vendor, then the provider name, for determinism.
    let preferred: Vec<String> = policy
        .value
        .pointer("/vendors/verifier_order")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let preference = |vendor: &str, name: &str| -> usize {
        let vendor = vendor.to_ascii_lowercase();
        let name = name.to_ascii_lowercase();
        preferred
            .iter()
            .position(|n| *n == vendor || *n == name)
            .unwrap_or(usize::MAX)
    };
    order.sort_by_key(|p| {
        (
            preference(p.vendor(), p.name()),
            p.vendor() != builder_vendor,
            p.name().to_string(),
        )
    });
    let candidates: Vec<&Box<dyn DeskProvider>> = if cross_required {
        order
            .iter()
            .copied()
            .filter(|p| p.vendor() != builder_vendor)
            .collect()
    } else {
        order.clone()
    };
    let mut single_vendor = false;
    if candidates.is_empty() && cross_required {
        let reason = format!(
            "cross-vendor verification is required from the {} lane and up, and the builder's \
             vendor is {builder_vendor:?}; no configured provider serves another vendor",
            lane_name(threshold)
        );
        if warn_only {
            single_vendor = true;
        } else {
            for desk in &desks {
                ledger
                    .append(
                        Writer::Desk,
                        Some(session),
                        Record::CheckUnavailable {
                            component: format!("desk:{desk}"),
                            reason: reason.clone(),
                        },
                    )
                    .map_err(|e| e.to_string())?;
            }
            return Ok(Outcome::Refused { reason, desks });
        }
    }
    let pool: Vec<&Box<dyn DeskProvider>> = if candidates.is_empty() {
        order
    } else {
        candidates
    };
    let view = ledger_view(ledger, session)?;
    let mut receipts = Vec::new();
    for (i, desk) in desks.iter().enumerate() {
        let Some(rubric_text) = rubric(desk) else {
            continue;
        };
        let Some(provider) = pool.get(i % pool.len().max(1)).copied() else {
            let reason = "no provider is configured, so this desk could not run".to_string();
            ledger
                .append(
                    Writer::Desk,
                    Some(session),
                    Record::CheckUnavailable {
                        component: format!("desk:{desk}"),
                        reason: reason.clone(),
                    },
                )
                .map_err(|e| e.to_string())?;
            receipts.push(Receipt {
                desk: desk.clone(),
                provider: String::new(),
                vendor: String::new(),
                served_vendor: None,
                requested_model: String::new(),
                returned_model: None,
                settings: None,
                identity: "not_stated".into(),
                findings: 0,
                hypotheses: 0,
                outcome: "failed".into(),
                usage: None,
                check_seq: 0,
                error: Some(reason),
            });
            continue;
        };
        let focus = desk_focus(desk, ledger, session, root)?;
        let prompt = desk_prompt(desk, rubric_text, opts.lane, &view, &focus);
        let started_at = verdict_core::timefmt::now_rfc3339();
        let started = Instant::now();
        let timeout = provider
            .timeout_secs()
            .map(Duration::from_secs)
            .unwrap_or(opts.timeout);
        let answer = provider.ask(&prompt, timeout);
        let finished_at = verdict_core::timefmt::now_rfc3339();
        let mut findings = 0;
        let mut hypotheses = 0;
        let mut usage = None;
        let mut returned_model = None;
        let mut served_vendor: Option<String> = None;
        let mut settings: Option<Value> = None;
        let mut identity = "not_stated";
        let (outcome, error) = match answer.and_then(|t| parse_answer(&t)) {
            Err(e) => {
                ledger
                    .append(
                        Writer::Desk,
                        Some(session),
                        Record::CheckUnavailable {
                            component: format!("desk:{desk}"),
                            reason: e.clone(),
                        },
                    )
                    .map_err(|e| e.to_string())?;
                ("failed".to_string(), Some(e))
            }
            Ok(raw) => {
                usage = raw.usage.clone();
                returned_model = raw.model.clone();
                if let Some(s) = &raw.served {
                    served_vendor = s.vendor.clone();
                    settings = s.settings.clone();
                    if s.model.is_some() {
                        returned_model = s.model.clone();
                    }
                }
                // Effective settings can be observed without any served vendor being stated (the
                // provider adapter records the invocation itself).
                if settings.is_none() {
                    settings = raw.settings.clone();
                }
                identity = if served_vendor.is_some() {
                    "confirmed"
                } else {
                    "not_stated"
                };
                // A stated served vendor that is the builder's cannot be independent verification
                // on a run that requires another vendor. Refuse that answer's findings.
                if cross_required
                    && served_vendor
                        .as_deref()
                        .is_some_and(|v| v.eq_ignore_ascii_case(&builder_vendor))
                {
                    identity = "mismatch";
                    let why = format!(
                        "the provider says the answer was served by {served_vendor:?}, which is the \
                         builder's vendor; this run requires another vendor, so the answer was not \
                         counted"
                    );
                    ledger
                        .append(
                            Writer::Desk,
                            Some(session),
                            Record::CheckUnavailable {
                                component: format!("desk:{desk}"),
                                reason: why.clone(),
                            },
                        )
                        .map_err(|e| e.to_string())?;
                    ("failed".to_string(), Some(why))
                } else {
                    for h in &raw.hypotheses {
                        hypotheses += 1;
                        ledger
                            .append(
                                Writer::Desk,
                                Some(session),
                                Record::Hypothesis(Finding {
                                    id: format!(
                                        "desk:{desk}.h{hypotheses}.{}",
                                        desk_hash(desk, h, &[])
                                    ),
                                    severity: Severity::Minor,
                                    title: h.clone(),
                                    claim_seq: None,
                                    evidence: Vec::new(),
                                    tier: Tier::Judgment,
                                    blast: Some(opts.lane),
                                    judgment: true,
                                    fix_hint: None,
                                }),
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    for f in &raw.findings {
                        // Every citation must resolve; an answer whose citations do not is a
                        // hypothesis, not a finding.
                        let mut evidence = Vec::new();
                        let mut bad = None;
                        for e in &f.evidence {
                            match e {
                                RawEvidence::Record { record } => {
                                    match record_evidence(ledger, *record) {
                                        Some(p) => evidence.push(p),
                                        None => {
                                            bad = Some(format!(
                                                "it cites ledger record {record}, which is not in this ledger"
                                            ))
                                        }
                                    }
                                }
                                RawEvidence::FileLine { file, line } => {
                                    match EvidencePointer::file_line(root, file, *line) {
                                        Ok(p) => evidence.push(p),
                                        Err(e) => {
                                            bad = Some(format!("it cites {file}:{line}: {e}"))
                                        }
                                    }
                                }
                            }
                        }
                        if evidence.is_empty() {
                            let why = bad.unwrap_or_else(|| "it cites nothing".into());
                            hypotheses += 1;
                            ledger
                                .append(
                                    Writer::Desk,
                                    Some(session),
                                    Record::Hypothesis(Finding {
                                        id: format!(
                                            "desk:{desk}.h{hypotheses}.{}",
                                            desk_hash(desk, &f.title, &[])
                                        ),
                                        severity: f.severity,
                                        title: format!("{} ({why})", f.title),
                                        claim_seq: None,
                                        evidence: Vec::new(),
                                        tier: Tier::Judgment,
                                        blast: Some(opts.lane),
                                        judgment: true,
                                        fix_hint: f.fix.clone(),
                                    }),
                                )
                                .map_err(|e| e.to_string())?;
                            continue;
                        }
                        findings += 1;
                        let id = format!("desk:{desk}.{}", desk_hash(desk, &f.title, &evidence));
                        ledger
                            .append(
                                Writer::Desk,
                                Some(session),
                                Record::Finding(Finding {
                                    id,
                                    severity: f.severity,
                                    title: f.title.clone(),
                                    claim_seq: None,
                                    evidence,
                                    tier: Tier::Judgment,
                                    blast: Some(opts.lane),
                                    judgment: true,
                                    fix_hint: f.fix.clone(),
                                }),
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    // Proposals: a desk may ask the core to run a configured mutation or firstrun
                    // cell. It never runs code itself; the proposal is recorded as a hypothesis (it
                    // never moves the verdict) and `verdict-engine` may run the configured checker.
                    for (pi, p) in raw.proposals.iter().enumerate() {
                        let (checker, reason) = match p {
                            Value::String(s) => (s.trim().to_string(), String::new()),
                            Value::Object(o) => (
                                o.get("checker")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .trim()
                                    .to_string(),
                                o.get("reason")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                            ),
                            _ => continue,
                        };
                        if checker.is_empty() {
                            continue;
                        }
                        hypotheses += 1;
                        let title = format!(
                            "PROPOSAL: {checker} — {reason} (the desk did not run any code)"
                        );
                        ledger
                            .append(
                                Writer::Desk,
                                Some(session),
                                Record::Hypothesis(Finding {
                                    id: format!(
                                        "desk:{desk}.proposal.{pi}.{}",
                                        desk_hash(desk, &title, &[])
                                    ),
                                    severity: Severity::Nit,
                                    title,
                                    claim_seq: None,
                                    evidence: Vec::new(),
                                    tier: Tier::Judgment,
                                    blast: Some(opts.lane),
                                    judgment: true,
                                    fix_hint: None,
                                }),
                            )
                            .map_err(|e| e.to_string())?;
                    }
                    ("ran".to_string(), None)
                }
            }
        };
        let detail = format!(
            "desk {desk}; provider {:?} (vendor {}, requested model {:?}, returned model {:?}); \
             served vendor {:?} ({identity}); settings {:?}; usage {:?}; lane {}; policy v{policy_version}; \
             {} finding(s), {} hypothesis(ies); {} ms",
            provider.name(),
            provider.vendor(),
            provider.model(),
            returned_model,
            served_vendor,
            settings,
            usage,
            lane_name(opts.lane),
            findings,
            hypotheses,
            started.elapsed().as_millis()
        );
        let check_seq = ledger
            .append(
                Writer::Core,
                Some(session),
                Record::Check {
                    name: format!("desk:{desk}"),
                    tier: Tier::Judgment,
                    inputs_hash: desk_hash(desk, &prompt, &[]),
                    started_at,
                    finished_at,
                    outcome: if error.is_some() {
                        CheckOutcome::FailedToRun
                    } else {
                        CheckOutcome::Ran
                    },
                    detail: Some(detail),
                    tree_state: verdict_core::baseline::tree_state_of_ledger(ledger),
                },
            )
            .map_err(|e| e.to_string())?
            .seq;
        receipts.push(Receipt {
            desk: desk.clone(),
            provider: provider.name().to_string(),
            vendor: provider.vendor().to_string(),
            served_vendor,
            requested_model: provider.model().to_string(),
            returned_model,
            settings,
            identity: identity.to_string(),
            findings,
            hypotheses,
            outcome,
            usage,
            check_seq,
            error,
        });
    }
    Ok(Outcome::Ran {
        receipts,
        single_vendor,
    })
}

/// Render a receipt as one line, for `vdt desks` and the report's checks section.
pub fn receipt_line(r: &Receipt) -> String {
    let mut s = format!(
        "desk {:<8} {:<7} provider {} [{}] model {}",
        r.desk, r.outcome, r.provider, r.vendor, r.requested_model
    );
    if let Some(m) = &r.returned_model
        && m != &r.requested_model
    {
        s.push_str(&format!(" (served {m})"));
    }
    if let Some(v) = &r.served_vendor {
        s.push_str(&format!("; served by {v} ({})", r.identity));
    } else {
        s.push_str(&format!(
            "; served vendor {} ({})",
            "not stated", r.identity
        ));
    }
    if let Some(settings) = &r.settings {
        s.push_str(&format!("; settings {settings}"));
    }
    s.push_str(&format!(
        ": {} finding(s), {} hypothesis(ies)",
        r.findings, r.hypotheses
    ));
    if let Some(u) = &r.usage {
        s.push_str(&format!("; usage {u}"));
    }
    if let Some(e) = &r.error {
        s.push_str(&format!("; {e}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_desk_has_a_rubric_that_says_it_may_not_write_a_verdict() {
        for d in DESK_NAMES {
            let r = rubric(d).unwrap_or_else(|| panic!("no rubric for {d}"));
            assert!(r.contains("verdict"), "{d}");
            assert!(
                r.contains("hypothesis") || r.contains("Hypothesis"),
                "{d} does not mention hypotheses"
            );
        }
        assert!(rubric("mystery").is_none());
    }

    #[test]
    fn an_answer_in_a_code_fence_parses_and_a_broken_one_does_not() {
        let a = parse_answer("```json\n{\"findings\":[],\"hypotheses\":[\"x\"]}\n```").unwrap();
        assert_eq!(a.hypotheses, vec!["x".to_string()]);
        assert!(parse_answer("not json").is_err());
        assert!(parse_answer("```json\n{\"findings\":[]}").is_err());
    }

    #[test]
    fn a_prompt_carries_the_rubric_the_lane_and_the_ledger_view() {
        let p = desk_prompt(
            "qa",
            "RUBRIC TEXT",
            Lane::High,
            "CLAIM 1 [plan_line]: x",
            "FOCUS TEXT",
        );
        assert!(p.contains("RUBRIC TEXT"));
        assert!(p.contains("Lane: high"));
        assert!(p.contains("CLAIM 1"));
        assert!(p.contains("FOCUS TEXT"));
        assert!(p.contains("{\"record\": N}"));
    }

    #[test]
    fn the_desk_id_is_stable_for_the_same_title_and_evidence() {
        let e = vec![EvidencePointer::command("ledger record 3", b"x")];
        assert_eq!(desk_hash("qa", "t", &e), desk_hash("qa", "t", &e));
        assert_ne!(desk_hash("qa", "t", &e), desk_hash("docs", "t", &e));
    }
}
