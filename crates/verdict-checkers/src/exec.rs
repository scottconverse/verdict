//! exec: the project's own commands (build, lint, typecheck, test, proof), run for real.
//!
//! * Runs the policy's `exec.commands` only after the human has approved exactly those commands
//!   (a `run_project_commands` decision whose hash matches the commands, tiers, shell and timeout
//!   in force now). Without it the checker is unavailable ("not approved") and nothing runs.
//! * Native on every platform: `.ps1` scripts go to `pwsh` / `powershell`, other commands to
//!   `cmd /C` on Windows and `sh -c` elsewhere. Never bash on Windows, never WSL.
//! * Each command gets the policy timeout (never more than the caller's). Output is hashed in
//!   full; only a short tail is kept. Every command that ran keeps its own receipt in the ledger.
//! * A command runs inside a process tree the checker controls (a Job Object on Windows, a
//!   process group on Unix). When the command ends, or times out, everything it started is
//!   stopped. A run that left processes behind, or whose output could not be captured completely,
//!   never counts as a success.
//! * A failing command is run again on a temp copy of the baseline commit, so the finding says
//!   whether this session broke it (`broken-by-session`, blocks), it was `already-red` (Major:
//!   the baseline ran under the same conditions and failed the same way), or the baseline is
//!   unknown (`failed-baseline-unknown`, blocks: the copy could not be prepared like the working
//!   tree, or the re-run timed out or ended differently).
//! * A command that needs a server (`"needs": "server"`) is not run here; it is reported as an
//!   honest_red finding. A command that cannot be started is likewise honest_red.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use verdict_core::{EvidencePointer, Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::changes::git;
use crate::context::{CheckContext, Draft, ExecApproval, Receipt, Unavailable, finish};

pub use verdict_core::exec_approval::{ExecCommand, parse_duration, policy_commands};

/// The exec checker.
pub struct Exec;

/// How much of each output stream's end to keep.
const TAIL_BYTES: usize = 600;

/// How long to wait for the output pipes to close once the process tree is gone.
const PIPE_GRACE: Duration = Duration::from_secs(5);

/// What running one command produced.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// Exit code, if the process ended by itself.
    pub exit: Option<i32>,
    /// True when it was killed for running too long.
    pub timed_out: bool,
    /// Set when the command could not be started at all.
    pub spawn_error: Option<String>,
    /// blake3 of both output streams in full.
    pub output_hash: String,
    /// The last bytes of output, as text.
    pub tail: String,
    /// False when the output pipes did not close, so the hash covers only part of the output.
    pub output_complete: bool,
    /// True when processes the command started were still running after it ended (they were
    /// stopped by the checker).
    pub left_behind: bool,
    /// Set when the checker could not control the process tree.
    pub tree_error: Option<String>,
}

impl RunResult {
    /// Anything wrong with the run besides its exit code.
    pub fn problem(&self) -> Option<String> {
        let mut p = Vec::new();
        if self.left_behind {
            p.push("it left processes running after it ended (they were stopped)".to_string());
        }
        if !self.output_complete {
            p.push("its output could not be captured completely".to_string());
        }
        if let Some(e) = &self.tree_error {
            p.push(format!(
                "the processes it started could not be controlled: {e}"
            ));
        }
        if p.is_empty() {
            None
        } else {
            Some(p.join("; "))
        }
    }

    /// Ended cleanly: no stray processes, all output captured.
    pub fn clean(&self) -> bool {
        self.problem().is_none()
    }

    /// Exited 0 within the time limit, with nothing left behind and all output captured.
    pub fn success(&self) -> bool {
        self.spawn_error.is_none() && !self.timed_out && self.exit == Some(0) && self.clean()
    }

    fn not_started(err: String) -> RunResult {
        RunResult {
            exit: None,
            timed_out: false,
            spawn_error: Some(err),
            output_hash: blake3::hash(b"").to_hex().to_string(),
            tail: String::new(),
            output_complete: true,
            left_behind: false,
            tree_error: None,
        }
    }
}

#[derive(Default)]
struct Capture {
    hasher: blake3::Hasher,
    tail: Vec<u8>,
}

fn drain<R: Read + Send + 'static>(mut r: R) -> (Arc<Mutex<Capture>>, std::thread::JoinHandle<()>) {
    let cap = Arc::new(Mutex::new(Capture::default()));
    let c2 = Arc::clone(&cap);
    let h = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Ok(mut c) = c2.lock() {
                c.hasher.update(&buf[..n]);
                c.tail.extend_from_slice(&buf[..n]);
                if c.tail.len() > TAIL_BYTES * 2 {
                    let cut = c.tail.len() - TAIL_BYTES;
                    c.tail.drain(..cut);
                }
            }
        }
    });
    (cap, h)
}

/// Split a command line into words, keeping double-quoted text together.
fn words(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut any = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                any = true;
            }
            c if c.is_whitespace() && !in_q => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Build the process for `cmd`, natively for this platform.
fn build_command(cmd: &str, cwd: &Path, powershell: &str) -> Command {
    let ws = words(cmd);
    let is_ps1 = verdict_core::exec_approval::is_powershell_script(cmd);
    let mut c;
    if is_ps1 {
        c = Command::new(powershell);
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .args(&ws);
    } else {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c = Command::new("cmd");
            c.args(["/D", "/S", "/C"]).raw_arg(format!("\"{cmd}\""));
        }
        #[cfg(not(windows))]
        {
            c = Command::new("sh");
            c.arg("-c").arg(cmd);
        }
    }
    c.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c
}

/// The process tree one command runs in. On Windows a Job Object with kill-on-close holds the
/// command and everything it starts; on Unix the command leads its own process group.
struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    pgid: i32,
    #[cfg(windows)]
    pid: u32,
    error: Option<String>,
}

#[cfg(windows)]
impl Tree {
    fn attach(child: &std::process::Child) -> Tree {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        let pid = child.id();
        let fail = |what: &str| format!("{what} failed: {}", std::io::Error::last_os_error());
        // SAFETY: plain Win32 calls on a handle this function creates and owns; the structure
        // passed to SetInformationJobObject is zero-initialised and fully sized.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Tree {
                    job: std::ptr::null_mut(),
                    pid,
                    error: Some(fail("CreateJobObject")),
                };
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let e = fail("SetInformationJobObject");
                CloseHandle(job);
                return Tree {
                    job: std::ptr::null_mut(),
                    pid,
                    error: Some(e),
                };
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                let e = fail("AssignProcessToJobObject");
                CloseHandle(job);
                return Tree {
                    job: std::ptr::null_mut(),
                    pid,
                    error: Some(e),
                };
            }
            Tree {
                job,
                pid,
                error: None,
            }
        }
    }

    /// How many processes of the tree are still running (`None` if that cannot be read).
    fn active(&mut self) -> Option<u32> {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };
        if self.job.is_null() {
            return None;
        }
        // SAFETY: the handle is a live job object; the buffer is a zeroed struct of the size
        // passed.
        unsafe {
            let mut acct: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            let ok = QueryInformationJobObject(
                self.job,
                JobObjectBasicAccountingInformation,
                &mut acct as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            );
            if ok == 0 {
                self.error = Some(format!(
                    "QueryInformationJobObject failed: {}",
                    std::io::Error::last_os_error()
                ));
                None
            } else {
                Some(acct.ActiveProcesses)
            }
        }
    }

    fn kill(&mut self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        if self.job.is_null() {
            // No job: the best available is taskkill on the process tree.
            let _ = Command::new("taskkill")
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        } else {
            // SAFETY: the handle is a live job object.
            unsafe {
                TerminateJobObject(self.job, 1);
            }
        }
    }
}

#[cfg(windows)]
impl Drop for Tree {
    fn drop(&mut self) {
        if !self.job.is_null() {
            // SAFETY: closing the handle this struct owns; kill-on-close stops anything left.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.job);
            }
        }
    }
}

#[cfg(unix)]
impl Tree {
    fn attach(child: &std::process::Child) -> Tree {
        Tree {
            pgid: child.id() as i32,
            error: None,
        }
    }

    /// How many processes of the group still exist: 0, or 1 meaning "at least one".
    fn active(&mut self) -> Option<u32> {
        // SAFETY: signal 0 only checks that the group exists.
        let r = unsafe { libc::kill(-self.pgid, 0) };
        if r == 0 {
            return Some(1);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Some(0),
            // EPERM: it exists but is not ours to signal; it is still there.
            Some(libc::EPERM) => Some(1),
            _ => None,
        }
    }

    fn kill(&mut self) {
        // SAFETY: SIGKILL to the process group this tree created.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
    }
}

impl Tree {
    /// Wait until the tree is empty, up to `limit`. True when it is.
    fn wait_empty(&mut self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        loop {
            match self.active() {
                Some(0) => return true,
                None => return false,
                Some(_) => {}
            }
            if Instant::now() >= until {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Run one command line in `cwd` with a time limit.
pub fn run_command(cmd: &str, cwd: &Path, timeout: Duration) -> RunResult {
    let mut spawned = None;
    let mut last_err = String::new();
    let candidates: &[&str] = if cfg!(windows) {
        &["pwsh", "powershell"]
    } else {
        &["pwsh"]
    };
    let tries: Vec<&str> = if verdict_core::exec_approval::is_powershell_script(cmd) {
        candidates.to_vec()
    } else {
        vec![""]
    };
    for ps in tries {
        match build_command(cmd, cwd, ps).spawn() {
            Ok(c) => {
                spawned = Some(c);
                break;
            }
            Err(e) => last_err = format!("could not start the command: {e}"),
        }
    }
    let Some(mut child) = spawned else {
        return RunResult::not_started(last_err);
    };
    let mut tree = Tree::attach(&child);
    let (out_cap, out_h) = drain(child.stdout.take().expect("piped"));
    let (err_cap, err_h) = drain(child.stderr.take().expect("piped"));
    let start = Instant::now();
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    tree.kill();
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                tree.kill();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // The command has ended. Anything it started that is still running is a stray: give it a
    // moment to finish on its own (a process that just exited may not be gone yet), then stop it.
    let mut left_behind = false;
    if !timed_out && tree.error.is_none() && !tree.wait_empty(Duration::from_millis(300)) {
        // Still running (and not just unreadable): the command left processes behind.
        left_behind = tree.error.is_none();
    }
    if timed_out || left_behind || tree.error.is_some() {
        tree.kill();
        tree.wait_empty(Duration::from_secs(3));
    }
    // Every holder of the pipes is gone, so the readers reach end of file. If they do not, the
    // output is incomplete and the result says so.
    let wait_until = Instant::now() + PIPE_GRACE;
    while !(out_h.is_finished() && err_h.is_finished()) && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(10));
    }
    let output_complete = out_h.is_finished() && err_h.is_finished();
    if output_complete {
        let _ = out_h.join();
        let _ = err_h.join();
    }
    let take = |cap: &Arc<Mutex<Capture>>| -> (String, Vec<u8>) {
        let c = cap.lock().expect("capture lock");
        (c.hasher.finalize().to_hex().to_string(), c.tail.clone())
    };
    let (oh, ot) = take(&out_cap);
    let (eh, et) = take(&err_cap);
    let mut h = blake3::Hasher::new();
    h.update(b"stdout:");
    h.update(oh.as_bytes());
    h.update(b"stderr:");
    h.update(eh.as_bytes());
    let tail = format!("{}{}", tail_text(&et), tail_text(&ot));
    RunResult {
        exit,
        timed_out,
        spawn_error: None,
        output_hash: h.finalize().to_hex().to_string(),
        tail,
        output_complete,
        left_behind,
        tree_error: tree.error.clone(),
    }
}

fn tail_text(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(TAIL_BYTES);
    let s = String::from_utf8_lossy(&bytes[start..]).to_string();
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect::<String>()
        .replace('\n', " | ")
}

/// A temp copy of the baseline commit's tracked files, deleted on drop.
struct BaselineCopy {
    dir: PathBuf,
    index: PathBuf,
    /// Where the project root sits inside the copy.
    cwd: PathBuf,
}

impl Drop for BaselineCopy {
    fn drop(&mut self) {
        for _ in 0..3 {
            if std::fs::remove_dir_all(&self.dir).is_ok() || !self.dir.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(&self.index);
    }
}

fn baseline_copy(root: &Path, commit: &str) -> Result<BaselineCopy, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!("{}-{nanos}", std::process::id());
    let dir = std::env::temp_dir().join(format!("verdict-baseline-{id}"));
    let index = std::env::temp_dir().join(format!("verdict-baseline-{id}.idx"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make a temp folder: {e}"))?;
    let guard = BaselineCopy {
        dir: dir.clone(),
        index: index.clone(),
        cwd: dir.clone(),
    };
    let run = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .output()
            .map_err(|e| format!("git could not be run: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    };
    run(&["read-tree", commit])?;
    let prefix = format!("--prefix={}/", dir.to_string_lossy().replace('\\', "/"));
    run(&["checkout-index", "-a", "-f", &prefix])?;
    let sub = String::from_utf8_lossy(&git(root, &["rev-parse", "--show-prefix"])?)
        .trim()
        .to_string();
    let mut guard = guard;
    guard.cwd = if sub.is_empty() {
        dir
    } else {
        dir.join(sub.trim_end_matches('/'))
    };
    Ok(guard)
}

/// Ignored folders that are only build output or caches: a baseline copy without them is not
/// missing anything a command needs to run.
const REBUILDABLE: &[&str] = &[
    "target",
    "dist",
    "build",
    "out",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".cache",
    "coverage",
    ".nyc_output",
    ".next",
    ".turbo",
    ".verdict",
    ".idea",
    ".vscode",
];

/// What the working tree has that the baseline copy cannot have: ignored files and folders that
/// are not plain build output (installed dependencies such as `node_modules`, a `.venv`, a `.env`
/// file). While any exist, a failure on the copy proves nothing about the baseline.
fn environment_gap(root: &Path) -> Result<Vec<String>, String> {
    let raw = git(
        root,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
    )?;
    let mut gap = Vec::new();
    for item in String::from_utf8_lossy(&raw).split('\0') {
        let item = item.trim_end_matches('/');
        if item.is_empty() {
            continue;
        }
        let top = item.split('/').next().unwrap_or(item);
        let name = item.rsplit('/').next().unwrap_or(item);
        if REBUILDABLE.contains(&top) || REBUILDABLE.contains(&name) || name.ends_with(".log") {
            continue;
        }
        gap.push(item.to_string());
    }
    gap.sort();
    gap.dedup();
    Ok(gap)
}

/// The baseline question for one failing command, answered.
struct BaselineAnswer {
    rule: &'static str,
    severity: Severity,
    text: String,
}

fn unknown(why: impl Into<String>) -> BaselineAnswer {
    BaselineAnswer {
        rule: "failed-baseline-unknown",
        severity: Severity::Blocker,
        text: format!("baseline unknown: {}", why.into()),
    }
}

fn rank(t: Tier) -> u8 {
    match t {
        Tier::Browsed => 3,
        Tier::Ran => 2,
        Tier::Static => 1,
        _ => 0,
    }
}

fn ended_text(res: &RunResult, timeout: Duration) -> String {
    if res.timed_out {
        format!("timed out after {}s", timeout.as_secs())
    } else {
        format!(
            "exited with code {}",
            res.exit.map_or("none".into(), |c| c.to_string())
        )
    }
}

fn receipt(cmd: &str, tier: Tier, res: &RunResult, timeout: Duration) -> Receipt {
    Receipt {
        command: cmd.to_string(),
        tier,
        ended: ended_text(res, timeout),
        output_hash: res.output_hash.clone(),
        tail: res.tail.trim().to_string(),
        problem: res.problem(),
    }
}

impl Exec {
    /// Run the failing command again on the baseline and say what that shows.
    fn ask_baseline(
        &self,
        ctx: &CheckContext,
        cmd: &ExecCommand,
        first: &RunResult,
        timeout: Duration,
        base: &Option<String>,
        evidence: &mut Vec<EvidencePointer>,
    ) -> BaselineAnswer {
        let Some(commit) = base.as_ref().filter(|_| ctx.baseline.is_git_repo) else {
            return unknown(
                "the baseline is a folder hash and cannot be re-run, so it is not known whether \
                 this session broke it",
            );
        };
        let copy = match baseline_copy(&ctx.root, commit) {
            Ok(c) => c,
            Err(e) => {
                ctx.note(format!("baseline copy failed: {e}"));
                return unknown(
                    "the baseline copy could not be made, so it is not known whether this \
                     session broke it",
                );
            }
        };
        let again = run_command(&cmd.run, &copy.cwd, timeout);
        evidence.push(EvidencePointer::Command {
            command: format!("{} (on a copy of the baseline)", cmd.run),
            output_hash: again.output_hash.clone(),
        });
        if again.spawn_error.is_none() {
            ctx.add_receipt(receipt(
                &format!("{} (on a copy of the baseline)", cmd.run),
                cmd.tier,
                &again,
                timeout,
            ));
        }
        if let Some(e) = &again.spawn_error {
            ctx.note(format!(
                "baseline re-run of `{}` could not start: {e}",
                cmd.run
            ));
            return unknown(
                "the re-run on the baseline could not start, so it is not known whether this session broke it",
            );
        }
        if again.success() {
            return BaselineAnswer {
                rule: "broken-by-session",
                severity: Severity::Blocker,
                text: "it passes on a copy of the baseline: this session broke it".into(),
            };
        }
        // It failed on the copy. That only means "already red" if the copy ran under the same
        // conditions and failed the same way.
        if again.timed_out {
            return unknown(format!(
                "the re-run on the baseline timed out after {}s, which says nothing about whether \
                 it was already failing",
                timeout.as_secs()
            ));
        }
        if let Some(p) = again.problem() {
            return unknown(format!("the re-run on the baseline was not clean ({p})"));
        }
        if first.timed_out || first.problem().is_some() || first.exit.is_none() {
            return unknown(
                "the run in the working tree did not end in a plain failure, so the two runs \
                 cannot be compared",
            );
        }
        match environment_gap(&ctx.root) {
            Err(e) => {
                return unknown(format!(
                    "it could not be checked whether the baseline copy matches the working \
                     tree's environment ({e})"
                ));
            }
            Ok(gap) if !gap.is_empty() => {
                let shown: Vec<&str> = gap.iter().take(3).map(String::as_str).collect();
                ctx.note(format!(
                    "the baseline copy holds tracked files only and lacks {}",
                    shown.join(", ")
                ));
                return unknown(format!(
                    "the baseline copy could not be prepared like the working tree (it lacks \
                     {}{}), so its failure may be only that",
                    shown.join(", "),
                    if gap.len() > 3 { " and more" } else { "" }
                ));
            }
            Ok(_) => {}
        }
        if again.exit != first.exit {
            return unknown(format!(
                "it failed differently on the baseline (exit {} there, {} here)",
                again.exit.map_or("none".into(), |c| c.to_string()),
                first.exit.map_or("none".into(), |c| c.to_string())
            ));
        }
        BaselineAnswer {
            rule: "already-red",
            severity: Severity::Major,
            text: format!(
                "already red: the same command also failed with exit {} on a copy of the \
                 baseline under the same conditions, so it was failing before this session",
                again.exit.map_or("none".into(), |c| c.to_string())
            ),
        }
    }
}

impl Checker for Exec {
    fn name(&self) -> &'static str {
        "exec"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Ran, Tier::Browsed]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        match ctx.exec_approval {
            ExecApproval::Current => {}
            ExecApproval::NotDecided => {
                return Err(Unavailable::new(
                    "not approved: the human has not decided to run the project's commands \
                     (nothing was run)",
                ));
            }
            ExecApproval::Refused => {
                return Err(Unavailable::new(
                    "not approved: the human said no to running the project's commands \
                     (nothing was run)",
                ));
            }
            ExecApproval::Unbound => {
                return Err(Unavailable::new(
                    "not approved: the earlier approval does not say which commands it covered, \
                     so it counts for none (nothing was run); approve this repo's commands again",
                ));
            }
            ExecApproval::Changed => {
                return Err(Unavailable::new(
                    "not approved: the commands changed since you approved them (nothing was run)",
                ));
            }
        }
        let commands = policy_commands(&ctx.policy);
        if commands.is_empty() {
            return Err(Unavailable::new("the policy lists no exec.commands"));
        }
        let timeout = ctx
            .policy
            .value
            .pointer("/exec/timeout")
            .and_then(|v| v.as_str())
            .and_then(parse_duration)
            .map_or(ctx.timeout, |p| p.min(ctx.timeout));
        let base = ctx.base_commit();
        let mut drafts: Vec<Draft> = Vec::new();
        let mut ran = 0usize;
        let mut best: Option<Tier> = None;
        let mut tails: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();

        for (i, cmd) in commands.iter().enumerate() {
            let order = format!("{i:04}");
            if let Some(needs) = &cmd.needs {
                skipped.push(format!("`{}` needs a {needs}", cmd.run));
                drafts.push(Draft {
                    rule: "needs-server",
                    severity: Severity::Major,
                    title: format!(
                        "`{}` was not run: it needs a {needs}, which this checker does not start",
                        cmd.run
                    ),
                    claim_seq: None,
                    evidence: vec![EvidencePointer::command(&cmd.run, b"")],
                    tier: Tier::HonestRed,
                    fix_hint: Some("run it by hand and attach the result".into()),
                    path: order,
                    line: 0,
                });
                continue;
            }
            let res = run_command(&cmd.run, &ctx.root, timeout);
            if let Some(e) = &res.spawn_error {
                skipped.push(format!("`{}`: {e}", cmd.run));
                drafts.push(Draft {
                    rule: "not-run",
                    severity: Severity::Major,
                    title: format!("`{}` could not be started: {e}", cmd.run),
                    claim_seq: None,
                    evidence: vec![EvidencePointer::command(&cmd.run, b"")],
                    tier: Tier::HonestRed,
                    fix_hint: Some(
                        "install what the command needs, then run the check again".into(),
                    ),
                    path: order,
                    line: 0,
                });
                continue;
            }
            ran += 1;
            ctx.add_receipt(receipt(&cmd.run, cmd.tier, &res, timeout));
            // A run that left processes behind or lost output earns no tier.
            if res.clean() && best.is_none_or(|b| rank(cmd.tier) > rank(b)) {
                best = Some(cmd.tier);
            }
            if res.success() {
                continue;
            }
            let how = ended_text(&res, timeout);
            let problem = res.problem();
            tails.push(format!(
                "`{}` {how}{}: {}",
                cmd.run,
                problem
                    .as_ref()
                    .map_or(String::new(), |p| format!(" ({p})")),
                res.tail.trim()
            ));
            let mut evidence = vec![EvidencePointer::Command {
                command: cmd.run.clone(),
                output_hash: res.output_hash.clone(),
            }];
            if res.exit == Some(0) && !res.timed_out {
                // It passed, but not cleanly. Nothing to compare with the baseline.
                drafts.push(Draft {
                    rule: "unclean-run",
                    severity: Severity::Major,
                    title: format!(
                        "`{}` {how} but did not end cleanly: {}",
                        cmd.run,
                        problem.unwrap_or_default()
                    ),
                    claim_seq: None,
                    evidence,
                    tier: cmd.tier,
                    fix_hint: Some(
                        "make the command stop everything it starts, then run the check again"
                            .into(),
                    ),
                    path: order,
                    line: 0,
                });
                continue;
            }
            let answer = self.ask_baseline(ctx, cmd, &res, timeout, &base, &mut evidence);
            drafts.push(Draft {
                rule: answer.rule,
                severity: answer.severity,
                title: format!(
                    "`{}` {how}{}; {}",
                    cmd.run,
                    problem.map_or(String::new(), |p| format!(" ({p})")),
                    answer.text
                ),
                claim_seq: None,
                evidence,
                tier: cmd.tier,
                fix_hint: Some(
                    "fix the failure; the output's last lines are in the check's detail".into(),
                ),
                path: order,
                line: 0,
            });
        }
        if ran == 0 {
            return Err(Unavailable::new(format!(
                "no command could be run here: {}",
                skipped.join("; ")
            )));
        }
        if let Some(t) = best {
            ctx.reached(t);
        }
        if !tails.is_empty() {
            let mut joined = tails.join(" || ");
            if joined.len() > 1800 {
                let cut = joined
                    .char_indices()
                    .nth(1800)
                    .map_or(joined.len(), |(i, _)| i);
                joined.truncate(cut);
            }
            ctx.set_exec_tail(joined);
        }
        Ok(finish(self.name(), drafts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("3m"), Some(Duration::from_secs(180)));
        assert_eq!(parse_duration("20s"), Some(Duration::from_secs(20)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("x"), None);
        assert_eq!(parse_duration("5"), None);
    }

    #[test]
    fn words_keep_quotes_together() {
        assert_eq!(
            words(r#".\scripts\check.ps1 -Name "a b" -Fast"#),
            vec![r".\scripts\check.ps1", "-Name", "a b", "-Fast"]
        );
    }

    #[test]
    fn a_passing_and_a_failing_command() {
        let dir = std::env::temp_dir();
        let ok = run_command("exit 0", &dir, Duration::from_secs(20));
        assert!(ok.success(), "{ok:?}");
        let bad = run_command("exit 3", &dir, Duration::from_secs(20));
        assert_eq!(bad.exit, Some(3));
        assert!(!bad.success());
    }

    #[test]
    fn output_is_hashed_and_only_the_tail_is_kept() {
        let dir = std::env::temp_dir();
        let r = run_command("echo hello-from-exec", &dir, Duration::from_secs(20));
        assert!(r.success());
        assert!(r.tail.contains("hello-from-exec"));
        assert_eq!(r.output_hash.len(), 64);
        assert!(r.output_complete);
    }
}
