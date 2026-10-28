//! `vdt-provider-codex`: an optional desk provider that asks the signed-in `codex` CLI.
//!
//! It is an adapter to the generic provider protocol (one JSON request on stdin, one JSON answer on
//! stdout); it is not a new provider and it does not change any Codex configuration. It runs
//! `codex exec` non-interactively in a read-only sandbox, then extracts the desk's JSON answer.
//!
//! Identity honesty: the requested model comes from the Verdict policy. Whatever `codex exec`
//! reports as the model or usage is recorded as **observed**; a vendor is never asserted unless the
//! CLI states one. Missing observed fields stay missing (the desk receipt then reads
//! `served vendor ... (not_stated)`), so a configured vendor/model can never masquerade as a served
//! identity.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_SECS: u64 = 240;
const MAX_ATTEMPTS: u32 = 2;

fn main() {
    match run() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("vdt-provider-codex: {e}");
            std::process::exit(1);
        }
    }
}

struct Args {
    codex: String,
    codex_args: Vec<String>,
    timeout: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut codex = std::env::var("VDT_CODEX").unwrap_or_else(|_| "codex".to_string());
    let mut codex_args = Vec::new();
    let mut timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--codex" => {
                let raw = it.next().ok_or("--codex needs a value")?;
                let mut parts = raw.split_whitespace();
                codex = parts.next().unwrap_or("codex").to_string();
                codex_args.extend(parts.map(str::to_string));
            }
            "--timeout-secs" => {
                let raw = it.next().ok_or("--timeout-secs needs a value")?;
                timeout = Duration::from_secs(
                    raw.parse()
                        .map_err(|_| format!("{raw:?} is not a whole number of seconds"))?,
                );
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(Args {
        codex,
        codex_args,
        timeout,
    })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let mut request = String::new();
    std::io::stdin()
        .read_to_string(&mut request)
        .map_err(|e| format!("cannot read the request: {e}"))?;
    let req: serde_json::Value =
        serde_json::from_str(&request).map_err(|e| format!("the request is not JSON: {e}"))?;
    let prompt = req
        .get("prompt")
        .and_then(serde_json::Value::as_str)
        .ok_or("the request has no prompt")?;
    let requested_model = req
        .get("model")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    let full_prompt = format!(
        "You are the model behind one Verdict desk. Answer the request below with exactly one JSON \
         object and no prose, no Markdown fence. The object must have \"findings\", \
         \"hypotheses\" and optionally \"proposals\".\n\n{prompt}"
    );

    let mut last_error = String::new();
    for attempt in 1..=MAX_ATTEMPTS {
        match ask(&args, &full_prompt, requested_model) {
            Ok(answer) => {
                println!("{answer}");
                return Ok(());
            }
            Err(e) => {
                last_error = e;
                if attempt < MAX_ATTEMPTS {
                    // Bounded backoff; a 429 is a quota answer, not provider evidence.
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        }
    }
    Err(last_error)
}

fn ask(args: &Args, prompt: &str, requested_model: &str) -> Result<String, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let last = std::env::temp_dir().join(format!(
        "vdt-codex-answer-{}-{stamp}.txt",
        std::process::id()
    ));

    let mut cmd = Command::new(&args.codex);
    cmd.args(&args.codex_args);
    cmd.args([
        "exec",
        "--json",
        "--sandbox",
        "read-only",
        "--skip-git-repo-check",
        "--ephemeral",
    ]);
    cmd.args(["-C", "."]);
    cmd.arg("-o").arg(&last);
    if !requested_model.trim().is_empty() {
        cmd.args(["-m", requested_model.trim()]);
    }
    // `-` reads the prompt from stdin.
    cmd.arg("-");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", args.codex))?;
    {
        let mut stdin = child.stdin.take().ok_or("codex stdin was not available")?;
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|e| format!("could not give the prompt to codex: {e}"))?;
    }
    // Drain both pipes while the process runs: waiting for exit first would deadlock a chatty
    // provider once a pipe buffer fills.
    let read = |pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let out_h = read(child.stdout.take());
    let err_h = {
        let pipe = child.stderr.take();
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if started.elapsed() > args.timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_h.join();
                    let _ = err_h.join();
                    let _ = std::fs::remove_file(&last);
                    return Err(format!(
                        "codex did not answer within {} seconds",
                        args.timeout.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_h.join();
                let _ = err_h.join();
                return Err(format!("could not wait for codex: {e}"));
            }
        }
    };
    let stdout = out_h.join().unwrap_or_default();
    let stderr = err_h.join().unwrap_or_default();

    if !status.success() {
        let _ = std::fs::remove_file(&last);
        let text = format!("{stdout}\n{stderr}");
        let tail: Vec<&str> = text
            .lines()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(format!(
            "codex exited {}: {}",
            status,
            tail.join(" | ").replace(['\n', '\r'], " ")
        ));
    }

    let answer_text = std::fs::read_to_string(&last).unwrap_or_default();
    let _ = std::fs::remove_file(&last);
    let mut answer = extract_json(&answer_text)?;
    let (observed_model, usage) = observed_from_events(&stdout);
    if let Some(obj) = answer.as_object_mut() {
        if obj.get("usage").is_none_or(serde_json::Value::is_null)
            && let Some(usage) = usage
        {
            obj.insert("usage".into(), usage);
        }
        if obj.get("model").is_none_or(serde_json::Value::is_null)
            && let Some(model) = &observed_model
        {
            obj.insert("model".into(), serde_json::Value::String(model.clone()));
        }
        // Only an observed model is reported as served; no vendor is ever inferred from config.
        if obj.get("served").is_none_or(serde_json::Value::is_null)
            && let Some(model) = observed_model
        {
            obj.insert("served".into(), serde_json::json!({ "model": model }));
        }
    }
    Ok(answer.to_string())
}

/// The model and usage the CLI itself reported, if any. Never inferred.
fn observed_from_events(stdout: &str) -> (Option<String>, Option<serde_json::Value>) {
    let mut model = None;
    let mut usage = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(m) = v.get("model").and_then(serde_json::Value::as_str) {
            model = Some(m.to_string());
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            usage = Some(u.clone());
        }
        // Some events carry the fields one level down.
        if let Some(obj) = v.get("item").and_then(serde_json::Value::as_object) {
            if let Some(m) = obj.get("model").and_then(serde_json::Value::as_str) {
                model = Some(m.to_string());
            }
            if let Some(u) = obj.get("usage").filter(|u| u.is_object()) {
                usage = Some(u.clone());
            }
        }
    }
    (model, usage)
}

/// Extract the first JSON object from the answer, tolerating prose or one Markdown fence.
fn extract_json(text: &str) -> Result<serde_json::Value, String> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t)
        && v.is_object()
    {
        return Ok(v);
    }
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after).trim_start();
        if let Some(end) = after.find("```")
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(after[..end].trim())
            && v.is_object()
        {
            return Ok(v);
        }
    }
    for (i, _) in t.match_indices('{') {
        let mut it = serde_json::Deserializer::from_str(&t[i..]).into_iter::<serde_json::Value>();
        if let Some(Ok(v)) = it.next()
            && v.is_object()
            && (v.get("findings").is_some() || v.get("hypotheses").is_some())
        {
            return Ok(v);
        }
    }
    Err("the answer contains no JSON object".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_object_is_extracted_from_prose_or_a_fence() {
        let v = extract_json("here it is:\n{\"findings\":[],\"hypotheses\":[]}\nthanks").unwrap();
        assert!(v.get("findings").is_some());
        let v = extract_json("```json\n{\"findings\":[],\"hypotheses\":[]}\n```").unwrap();
        assert!(v.get("findings").is_some());
        assert!(extract_json("no object here").is_err());
    }

    #[test]
    fn observed_events_are_not_inferred() {
        let (model, usage) = observed_from_events("{\"type\":\"turn.completed\"}");
        assert!(model.is_none());
        assert!(usage.is_none());
        let (model, usage) = observed_from_events(
            "{\"type\":\"turn.completed\",\"model\":\"gpt-observed\",\"usage\":{\"input_tokens\":3}}",
        );
        assert_eq!(model.as_deref(), Some("gpt-observed"));
        assert!(usage.is_some());
    }
}
