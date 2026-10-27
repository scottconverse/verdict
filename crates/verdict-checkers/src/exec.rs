//! exec: the project's own commands (build, lint, typecheck, test, proof), run for real.
//!
//! * Runs the policy's `exec.commands` only after the human has decided to run the project's
//!   commands (a `run_project_commands` decision in the ledger). Without it the checker is
//!   unavailable ("not approved") and nothing runs.
//! * Native on every platform: `.ps1` scripts go to `pwsh` / `powershell`, other commands to
//!   `cmd /C` on Windows and `sh -c` elsewhere. Never bash on Windows, never WSL.
//! * Each command gets the policy timeout (never more than the caller's). Output is hashed in
//!   full; only a short tail is kept, in the check's detail.
//! * A failing command is run again on a temp copy of the baseline commit, so the finding says
//!   whether this session broke it (`broken-by-session`, blocks) or it was `already-red`
//!   (Major: the session did not break it, but the human should know).
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
use crate::context::{CheckContext, Draft, Unavailable, finish};

/// The exec checker.
pub struct Exec;

/// How much of each output stream's end to keep.
const TAIL_BYTES: usize = 600;

/// One command from the policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecCommand {
    /// The command line.
    pub run: String,
    /// The tier it earns.
    pub tier: Tier,
    /// What it needs, for example `server`.
    pub needs: Option<String>,
}

/// Read the policy's `exec.commands`.
pub fn policy_commands(policy: &Policy) -> Vec<ExecCommand> {
    policy
        .value
        .pointer("/exec/commands")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let run = c.get("run")?.as_str()?.trim().to_string();
                    if run.is_empty() {
                        return None;
                    }
                    let tier = match c.get("tier").and_then(|t| t.as_str()).unwrap_or("ran") {
                        "static" => Tier::Static,
                        "browsed" => Tier::Browsed,
                        "asset" => Tier::Asset,
                        _ => Tier::Ran,
                    };
                    Some(ExecCommand {
                        run,
                        tier,
                        needs: c.get("needs").and_then(|n| n.as_str()).map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `3m`, `20s`, `500ms`, `1h`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let n: u64 = s[..split].parse().ok()?;
    match &s[split..] {
        "ms" => Some(Duration::from_millis(n)),
        "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n * 60)),
        "h" => Some(Duration::from_secs(n * 3600)),
        _ => None,
    }
}

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
}

impl RunResult {
    /// Exited 0 within the time limit.
    pub fn success(&self) -> bool {
        self.spawn_error.is_none() && !self.timed_out && self.exit == Some(0)
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
    let first = ws.first().map(String::as_str).unwrap_or("");
    let is_ps1 = first.to_ascii_lowercase().ends_with(".ps1");
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

fn kill_tree(pid: u32, child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
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
    let wants_ps = words(cmd)
        .first()
        .is_some_and(|w| w.to_ascii_lowercase().ends_with(".ps1"));
    let tries: Vec<&str> = if wants_ps {
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
        return RunResult {
            exit: None,
            timed_out: false,
            spawn_error: Some(last_err),
            output_hash: blake3::hash(b"").to_hex().to_string(),
            tail: String::new(),
        };
    };
    let pid = child.id();
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
                    kill_tree(pid, &mut child);
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break None,
        }
    };
    // A grandchild may still hold the pipes; wait briefly, then use what was read.
    let wait_until = Instant::now() + Duration::from_secs(2);
    while !(out_h.is_finished() && err_h.is_finished()) && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(10));
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

fn rank(t: Tier) -> u8 {
    match t {
        Tier::Browsed => 3,
        Tier::Ran => 2,
        Tier::Static => 1,
        _ => 0,
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
        if !ctx.exec_approved {
            return Err(Unavailable::new(
                "not approved: the human has not decided to run the project's commands \
                 (nothing was run)",
            ));
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
            if best.is_none_or(|b| rank(cmd.tier) > rank(b)) {
                best = Some(cmd.tier);
            }
            if res.success() {
                continue;
            }
            let how = if res.timed_out {
                format!("timed out after {}s", timeout.as_secs())
            } else {
                format!(
                    "exited with code {}",
                    res.exit.map_or("none".into(), |c| c.to_string())
                )
            };
            tails.push(format!("`{}` {how}: {}", cmd.run, res.tail.trim()));
            let mut evidence = vec![EvidencePointer::Command {
                command: cmd.run.clone(),
                output_hash: res.output_hash.clone(),
            }];
            let (rule, severity, verdict_text) = match &base {
                Some(commit) if ctx.baseline.is_git_repo => {
                    match baseline_copy(&ctx.root, commit) {
                        Ok(copy) => {
                            let again = run_command(&cmd.run, &copy.cwd, timeout);
                            evidence.push(EvidencePointer::Command {
                                command: format!("{} (on a copy of the baseline)", cmd.run),
                                output_hash: again.output_hash.clone(),
                            });
                            if again.spawn_error.is_some() {
                                ctx.note(format!(
                                    "baseline re-run of `{}` could not start",
                                    cmd.run
                                ));
                                (
                                    "failed-baseline-unknown",
                                    Severity::Blocker,
                                    "it could not be re-run on the baseline, so it is not known whether this session broke it",
                                )
                            } else if again.success() {
                                (
                                    "broken-by-session",
                                    Severity::Blocker,
                                    "it passes on a copy of the baseline: this session broke it",
                                )
                            } else {
                                ctx.note(
                                    "baseline copy holds tracked files only; a command that needs untracked files (installed packages, build output) can fail there for that reason",
                                );
                                (
                                    "already-red",
                                    Severity::Major,
                                    "it also fails on a copy of the baseline: it was already red before this session",
                                )
                            }
                        }
                        Err(e) => {
                            ctx.note(format!("baseline copy failed: {e}"));
                            (
                                "failed-baseline-unknown",
                                Severity::Blocker,
                                "the baseline copy could not be made, so it is not known whether this session broke it",
                            )
                        }
                    }
                }
                _ => (
                    "failed-baseline-unknown",
                    Severity::Blocker,
                    "the baseline is a folder hash and cannot be re-run, so it is not known whether this session broke it",
                ),
            };
            drafts.push(Draft {
                rule,
                severity,
                title: format!("`{}` {how}; {verdict_text}", cmd.run),
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
    }
}
