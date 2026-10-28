//! firstrun: the project's configured first-run workflow, run for real.
//!
//! The design's first-run proof is a walk from an empty state on a machine, with the evidence
//! recorded. This checker runs the commands the project lists under `firstrun.commands` (each a
//! string, or an object with `run` and an optional `needs`), in order, under the same human
//! approval as every other project command.
//!
//! **A web first run is attested, not labelled.** A project that configures
//! `firstrun.server` (`{"command": "...", "url": "http://127.0.0.1:PORT/"}`) gets that server
//! started by Verdict, a readiness check on its URL, the first-run commands run while it is up,
//! and its stdout/stderr captured and hashed. A project that configures `firstrun.browser`
//! (`{"command": "...", "screenshot": "path"}`) gets that browser command run and the screenshot
//! hashed into the evidence. Only then is `browsed` reached. A page that answers without a
//! configured browser command is recorded (its status line and body hash) but does not earn
//! `browsed`.
//!
//! `firstrun.web: false` declares a project with no page to load (a library or a CLI); its
//! command evidence is `ran`, and the release condition accepts that (see `verdict-core`). The
//! checker itself never weakens the release bar: the verdict reads `firstrun.web` from the policy.
//!
//! A command that fails is a `Blocker` when the release lane is judging and a `Major` otherwise; a
//! first-run dead end (the server never answered, the browser evidence is missing) is a `Blocker`
//! at release and recorded as not established elsewhere. The clean-container walk itself is a
//! release activity (`docs/RELEASE.md`); this checker records what it actually ran.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use verdict_core::exec_approval::{Shell, policy_shell};
use verdict_core::{EvidencePointer, Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, Draft, Receipt, Unavailable, finish};
use crate::exec::{parse_duration, run_command_in};

/// The first-run checker.
pub struct FirstRun;

/// The most any one command may run.
const MAX_COMMAND: Duration = Duration::from_secs(1800);

impl Checker for FirstRun {
    fn name(&self) -> &'static str {
        "firstrun"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Ran, Tier::Browsed]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let server_cfg = server_config(&ctx.policy);
        let browser_cfg = browser_config(&ctx.policy);
        let commands = commands(&ctx.policy);
        if commands.is_empty() && server_cfg.is_none() && browser_cfg.is_none() {
            return Err(Unavailable::new(
                "no first-run workflow is configured (set firstrun.commands, firstrun.server or \
                 firstrun.browser in verdict.json)",
            ));
        }
        if !ctx.exec_approved {
            return Err(Unavailable::new(
                "not approved: the human has not decided to run this repo's configured commands",
            ));
        }
        let shell = policy_shell(&ctx.policy).map_err(|e| {
            Unavailable::new(format!(
                "the project's declared shell cannot be used: {e}; nothing was run"
            ))
        })?;
        let timeout = policy_duration(&ctx.policy, "/firstrun/timeout")
            .unwrap_or(ctx.timeout)
            .min(MAX_COMMAND);
        let mut drafts: Vec<Draft> = Vec::new();
        let mut ran = 0usize;
        let mut server: Option<Server> = None;
        let mut page: Option<String> = None;

        if let Some(cfg) = &server_cfg {
            match Server::start(cfg, &ctx.root, shell) {
                Ok(s) => {
                    let ready_timeout = cfg
                        .ready_timeout
                        .unwrap_or(Duration::from_secs(60))
                        .min(MAX_COMMAND);
                    match wait_for(&cfg.url, ready_timeout) {
                        Ok((status, body_hash)) => {
                            page = Some(format!("{status}; body blake3 {body_hash}"));
                            ctx.note(format!(
                                "first-run server answered {}: {status} (body blake3 {body_hash})",
                                cfg.url
                            ));
                        }
                        Err(why) => {
                            ctx.note(format!(
                                "first-run server at {} did not answer: {why}",
                                cfg.url
                            ));
                            let (_, err) = s.logs();
                            let tail = String::from_utf8_lossy(&err);
                            let tail: String = tail
                                .chars()
                                .rev()
                                .take(600)
                                .collect::<String>()
                                .chars()
                                .rev()
                                .collect();
                            ctx.note(format!(
                                "first-run server stderr tail: {}",
                                tail.replace(['\n', '\r'], " | ")
                            ));
                            dead_end(
                                ctx,
                                &mut drafts,
                                format!(
                                    "the first-run server did not answer at {}: {why}",
                                    cfg.url
                                ),
                            );
                        }
                    }
                    server = Some(s);
                }
                Err(why) => {
                    ctx.note(format!("first-run server could not start: {why}"));
                    dead_end(
                        ctx,
                        &mut drafts,
                        format!("the first-run server could not start: {why}"),
                    );
                }
            }
        }

        for (i, (command, needs)) in commands.iter().enumerate() {
            if let Some(needs) = needs {
                let server_up = needs.eq_ignore_ascii_case("server") && server.is_some();
                if !server_up {
                    ctx.unestablished(format!(
                        "first-run command {} needs a {needs}, which this run did not start",
                        i + 1
                    ));
                    continue;
                }
            }
            let run = run_command_in(command, &ctx.root, timeout, shell);
            ran += 1;
            let problem = if let Some(e) = &run.spawn_error {
                Some(format!("it could not start: {e}"))
            } else if run.timed_out {
                Some(format!(
                    "it did not finish within {} seconds",
                    timeout.as_secs()
                ))
            } else if run.exit != Some(0) {
                Some(match run.exit {
                    Some(code) => format!("it exited {code}"),
                    None => "it ended without an exit code".to_string(),
                })
            } else {
                run.problem()
            };
            match problem {
                None => ctx.note(format!(
                    "first-run command {} ran and passed: {command}",
                    i + 1
                )),
                Some(why) => {
                    ctx.note(format!("first-run command {} failed: {command}", i + 1));
                    let severity = if ctx.lane == Lane::Release {
                        Severity::Blocker
                    } else {
                        Severity::Major
                    };
                    drafts.push(Draft {
                        rule: "command-failed",
                        severity,
                        title: format!(
                            "first-run command {} `{command}` did not pass: {why}",
                            i + 1
                        ),
                        claim_seq: None,
                        evidence: vec![verdict_core::EvidencePointer::command(
                            command,
                            run.tail.as_bytes(),
                        )],
                        tier: Tier::Ran,
                        fix_hint: Some(
                            "make the documented first-run path work from an empty state".into(),
                        ),
                        path: String::new(),
                        line: 0,
                    });
                }
            }
        }

        // Browser evidence: a configured browser command must succeed and its screenshot must
        // exist. Only then is the page load attested at the browsed tier.
        if let Some(cfg) = &browser_cfg {
            match run_command_in(&cfg.command, &ctx.root, timeout, shell) {
                run if run.success() => match screenshot(ctx, &cfg.screenshot) {
                    Ok((bytes, hash)) => {
                        ctx.note(format!(
                            "first-run browser attestation: command `{}` exited 0; screenshot {} \
                             ({} bytes, blake3 {}); page {}",
                            cfg.command,
                            cfg.screenshot,
                            bytes,
                            hash,
                            page.as_deref().unwrap_or("not fetched by Verdict")
                        ));
                        ctx.add_receipt(Receipt {
                            command: cfg.command.clone(),
                            tier: Tier::Browsed,
                            ended: "exit 0".into(),
                            output_hash: hash,
                            tail: String::new(),
                            problem: None,
                        });
                        ctx.reached(Tier::Browsed);
                    }
                    Err(why) => {
                        ctx.note(format!("first-run browser evidence is missing: {why}"));
                        dead_end(ctx, &mut drafts, why);
                    }
                },
                run => {
                    let why = run
                        .spawn_error
                        .clone()
                        .or_else(|| run.problem())
                        .unwrap_or_else(|| {
                            format!(
                                "it exited {}",
                                run.exit.map_or("abnormally".into(), |c| c.to_string())
                            )
                        });
                    ctx.note(format!("first-run browser command failed: {why}"));
                    dead_end(
                        ctx,
                        &mut drafts,
                        format!("the first-run browser command did not pass: {why}"),
                    );
                }
            }
        } else if server_cfg.is_some() {
            ctx.note(
                "firstrun.server answered but no firstrun.browser command is configured, so the \
                 page load is recorded without a screenshot: the browsed tier is not claimed",
            );
        }

        if let Some(mut s) = server.take() {
            s.stop();
            let (out, err) = s.logs();
            ctx.note(format!(
                "first-run server log hashes: stdout blake3 {}, stderr blake3 {} ({} and {} bytes)",
                blake3::hash(&out).to_hex(),
                blake3::hash(&err).to_hex(),
                out.len(),
                err.len()
            ));
        }

        if ran > 0 && ctx.reached_tier().is_none() {
            ctx.reached(Tier::Ran);
        }
        Ok(finish(self.name(), drafts))
    }
}

/// A first-run dead end: at release it blocks; elsewhere it is recorded as not established.
fn dead_end(ctx: &CheckContext, drafts: &mut Vec<Draft>, why: String) {
    ctx.unestablished(why.clone());
    if ctx.lane == Lane::Release {
        drafts.push(Draft {
            rule: "dead-end",
            severity: Severity::Blocker,
            title: format!("first-run dead end: {why}"),
            claim_seq: None,
            evidence: vec![verdict_core::EvidencePointer::command(
                "firstrun",
                why.as_bytes(),
            )],
            tier: Tier::Browsed,
            fix_hint: Some("make the documented first-run path work from an empty state".into()),
            path: String::new(),
            line: 0,
        });
    }
}

fn screenshot(ctx: &CheckContext, path: &str) -> Result<(u64, String), String> {
    let p = ctx.root.join(path);
    let meta = std::fs::metadata(&p)
        .map_err(|e| format!("the browser screenshot {path} cannot be read: {e}"))?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(format!(
            "the browser screenshot {path} is not a non-empty file"
        ));
    }
    let asset = EvidencePointer::asset(&ctx.root, path)
        .map_err(|e| format!("the browser screenshot {path} cannot be hashed: {e}"))?;
    match asset {
        EvidencePointer::Asset { asset_hash, .. } => Ok((meta.len(), asset_hash)),
        _ => Err("the browser screenshot could not be hashed".into()),
    }
}

#[derive(Debug, Clone)]
struct ServerConfig {
    command: String,
    url: String,
    ready_timeout: Option<Duration>,
}

fn server_config(policy: &Policy) -> Option<ServerConfig> {
    let s = policy.value.pointer("/firstrun/server")?.as_object()?;
    let command = s.get("command")?.as_str()?.trim().to_string();
    let url = s.get("url")?.as_str()?.trim().to_string();
    if command.is_empty() || url.is_empty() {
        return None;
    }
    Some(ServerConfig {
        command,
        url,
        ready_timeout: s
            .get("ready_timeout")
            .and_then(Value::as_str)
            .and_then(parse_duration),
    })
}

#[derive(Debug, Clone)]
struct BrowserConfig {
    command: String,
    screenshot: String,
}

fn browser_config(policy: &Policy) -> Option<BrowserConfig> {
    let b = policy.value.pointer("/firstrun/browser")?.as_object()?;
    let command = b.get("command")?.as_str()?.trim().to_string();
    let screenshot = b.get("screenshot")?.as_str()?.trim().to_string();
    if command.is_empty() || screenshot.is_empty() {
        return None;
    }
    Some(BrowserConfig {
        command,
        screenshot,
    })
}

fn commands(policy: &Policy) -> Vec<(String, Option<String>)> {
    policy
        .value
        .pointer("/firstrun/commands")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| match v {
                    Value::String(s) if !s.trim().is_empty() => Some((s.trim().to_string(), None)),
                    Value::Object(o) => {
                        let run = o.get("run").and_then(Value::as_str)?.trim().to_string();
                        if run.is_empty() {
                            return None;
                        }
                        let needs = o.get("needs").and_then(Value::as_str).map(str::to_string);
                        Some((run, needs))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn policy_duration(policy: &Policy, pointer: &str) -> Option<Duration> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(parse_duration)
}

/// A server Verdict started for the first-run walk. Its output is drained as it runs (so a chatty
/// server cannot block on a full pipe) and hashed when it stops.
struct Server {
    child: Child,
    out: Arc<Mutex<Vec<u8>>>,
    err: Arc<Mutex<Vec<u8>>>,
    out_h: Option<std::thread::JoinHandle<()>>,
    err_h: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn start(cfg: &ServerConfig, root: &std::path::Path, shell: Shell) -> Result<Server, String> {
        let mut cmd = crate::exec::server_command(&cfg.command, root, shell);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("the first-run server could not be started: {e}"))?;
        let out = Arc::new(Mutex::new(Vec::new()));
        let err = Arc::new(Mutex::new(Vec::new()));
        let out_h = drain(child.stdout.take(), Arc::clone(&out));
        let err_h = drain(child.stderr.take(), Arc::clone(&err));
        Ok(Server {
            child,
            out,
            err,
            out_h: Some(out_h),
            err_h: Some(err_h),
        })
    }

    fn stop(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: the server was spawned as its own process group.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            // Stop the shell and what it started while the shell is still there to own the tree.
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &self.child.id().to_string(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let until = Instant::now() + Duration::from_secs(3);
        let done =
            |h: &Option<std::thread::JoinHandle<()>>| h.as_ref().is_none_or(|h| h.is_finished());
        while !(done(&self.out_h) && done(&self.err_h)) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        // Join only a stream that has closed. A process that still holds a pipe must not block
        // the checker for ever; the process is already dead, so the thread ends with it.
        let finished =
            |h: &Option<std::thread::JoinHandle<()>>| h.as_ref().is_some_and(|h| h.is_finished());
        if finished(&self.out_h)
            && let Some(h) = self.out_h.take()
        {
            let _ = h.join();
        }
        if finished(&self.err_h)
            && let Some(h) = self.err_h.take()
        {
            let _ = h.join();
        }
    }

    fn logs(&self) -> (Vec<u8>, Vec<u8>) {
        (
            self.out.lock().map(|v| v.clone()).unwrap_or_default(),
            self.err.lock().map(|v| v.clone()).unwrap_or_default(),
        )
    }
}

fn drain<R: Read + Send + 'static>(
    pipe: Option<R>,
    into: Arc<Mutex<Vec<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let Some(mut pipe) = pipe else { return };
        let mut buf = [0u8; 4096];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut v) = into.lock()
                        && v.len() < 4 * 1024 * 1024
                    {
                        v.extend_from_slice(&buf[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

/// Wait until `url` answers over plain HTTP, then return its status line and body hash.
fn wait_for(url: &str, timeout: Duration) -> Result<(String, String), String> {
    let until = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < until {
        match fetch(url) {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(if last.is_empty() {
        "it did not answer in time".into()
    } else {
        format!("it did not answer in time ({last})")
    })
}

/// A plain-HTTP GET of a local page, bounded. HTTPS needs a real browser command.
fn fetch(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "only plain http:// pages can be fetched by Verdict itself".to_string())?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let socket = addr
        .to_socket_addrs()
        .map_err(|e| format!("{addr}: {e}"))?
        .next()
        .ok_or_else(|| format!("{addr} did not resolve"))?;
    let mut stream = TcpStream::connect_timeout(&socket, Duration::from_secs(2))
        .map_err(|e| format!("connect to {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("send: {e}"))?;
    let mut body = Vec::new();
    (&mut stream)
        .take(2 * 1024 * 1024)
        .read_to_end(&mut body)
        .map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&body).to_string();
    let status = text
        .lines()
        .next()
        .unwrap_or("no status line")
        .trim()
        .to_string();
    if !status.starts_with("HTTP/1.") {
        return Err(format!("not an HTTP answer: {status}"));
    }
    Ok((status, blake3::hash(&body).to_hex().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_http_local_page_can_be_fetched() {
        assert!(fetch("https://example.com/").is_err());
        assert!(fetch("notaurl").is_err());
    }

    #[test]
    fn a_server_block_without_its_url_is_not_a_server() {
        let mut p = Policy::builtin().unwrap();
        p.value["firstrun"] = serde_json::json!({"server": {"command": "python -m http.server"}});
        assert!(server_config(&p).is_none());
    }
}
