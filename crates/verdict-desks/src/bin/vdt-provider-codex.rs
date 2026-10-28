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
    // "unspecified" is the provider interface's "the policy named no model". Passing it to the CLI
    // would ask for a model that does not exist; let Codex use its own configured default and
    // record the observed fields separately.
    let requested = requested_model.trim();
    if !requested.is_empty() && !requested.eq_ignore_ascii_case("unspecified") {
        cmd.args(["-m", requested]);
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
    let answer = normalize_answer(extract_json(&answer_text)?);
    let (observed_model, usage) = observed_from_events(&stdout);
    let mut answer = apply_observation(answer, observed_model, usage);
    // Effective settings are observable from the invocation itself: what was run, with which
    // model flag and deadline, where. They are never taken from the answer's words.
    if let Some(obj) = answer.as_object_mut() {
        obj.insert(
            "settings".into(),
            serde_json::json!({
                "source": "invocation",
                "requested_model": requested_model,
                "timeout_secs": args.timeout.as_secs(),
                "cwd": ".",
                "args": args.codex_args,
            }),
        );
    }
    Ok(answer.to_string())
}

/// Observation outranks the answer's own words. `model` and `served` hold what the CLI reported;
/// anything the answer claimed about its identity is kept only under `claimed`, and when nothing
/// was observed no served identity is asserted at all (the receipt then reads `not_stated`).
fn apply_observation(
    mut answer: serde_json::Value,
    observed_model: Option<String>,
    usage: Option<serde_json::Value>,
) -> serde_json::Value {
    if let Some(obj) = answer.as_object_mut() {
        let claimed_served = obj.get("served").filter(|v| v.is_object()).cloned();
        let claimed_model = obj
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        if claimed_served.is_some() || claimed_model.is_some() {
            let mut claimed = serde_json::Map::new();
            if let Some(m) = claimed_model {
                claimed.insert("model".into(), serde_json::Value::String(m));
            }
            if let Some(s) = claimed_served {
                claimed.insert("served".into(), s);
            }
            obj.insert("claimed".into(), serde_json::Value::Object(claimed));
        }
        obj.remove("served");
        match &observed_model {
            // Only an observed model is reported as served; no vendor is ever inferred.
            Some(model) => {
                obj.insert("model".into(), serde_json::Value::String(model.clone()));
                obj.insert("served".into(), serde_json::json!({ "model": model }));
            }
            None => {
                obj.remove("model");
            }
        }
        if let Some(usage) = usage {
            obj.insert("usage".into(), usage);
        }
    }
    answer
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

/// Turn a mostly-right model answer into the exact answer shape the desk protocol reads. It never
/// invents a finding: entries that are not objects, a missing title, or evidence that is not an
/// object are dropped or reduced to a hypothesis, and an unknown severity becomes `minor`.
fn normalize_answer(v: serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let mut findings = Vec::new();
    if let Some(list) = v.get("findings").and_then(serde_json::Value::as_array) {
        for f in list {
            let Some(obj) = f.as_object() else { continue };
            let severity = obj
                .get("severity")
                .and_then(serde_json::Value::as_str)
                .map(|s| s.to_ascii_lowercase())
                .filter(|s| {
                    matches!(
                        s.as_str(),
                        "blocker" | "critical" | "major" | "minor" | "nit"
                    )
                })
                .unwrap_or_else(|| "minor".into());
            let title = obj
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("a desk concern");
            let mut finding = serde_json::json!({"severity": severity, "title": title});
            if let Some(evidence) = obj.get("evidence").and_then(serde_json::Value::as_array) {
                let evidence: Vec<_> = evidence.iter().filter(|e| e.is_object()).cloned().collect();
                if !evidence.is_empty() {
                    finding["evidence"] = serde_json::Value::Array(evidence);
                }
            }
            if let Some(fix) = obj.get("fix").and_then(serde_json::Value::as_str) {
                finding["fix"] = serde_json::Value::String(fix.to_string());
            }
            findings.push(finding);
        }
    }
    out.insert("findings".into(), serde_json::Value::Array(findings));
    if let Some(list) = v.get("hypotheses").and_then(serde_json::Value::as_array) {
        let list: Vec<_> = list
            .iter()
            .filter_map(|h| match h {
                serde_json::Value::String(s) => Some(serde_json::Value::String(s.clone())),
                serde_json::Value::Object(o) => o
                    .get("title")
                    .or_else(|| o.get("text"))
                    .or_else(|| o.get("description"))
                    .and_then(serde_json::Value::as_str)
                    .map(|s| serde_json::Value::String(s.to_string())),
                _ => None,
            })
            .collect();
        out.insert("hypotheses".into(), serde_json::Value::Array(list));
    }
    if let Some(list) = v.get("proposals").and_then(serde_json::Value::as_array) {
        let list: Vec<_> = list
            .iter()
            .filter(|p| p.is_string() || p.is_object())
            .cloned()
            .collect();
        out.insert("proposals".into(), serde_json::Value::Array(list));
    }
    for key in ["usage", "served"] {
        if let Some(o) = v.get(key).filter(|x| x.is_object()) {
            out.insert(key.into(), o.clone());
        }
    }
    if let Some(m) = v.get("model").and_then(serde_json::Value::as_str) {
        out.insert("model".into(), serde_json::Value::String(m.to_string()));
    }
    serde_json::Value::Object(out)
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
    fn a_model_answer_is_normalized_without_inventing_findings() {
        let v = serde_json::json!({
            "findings": [
                {"severity": "MAJOR", "title": "a real concern", "evidence": [{"record": 3}]},
                {"severity": "weird", "title": "still reported", "evidence": ["not an object"]},
                "not a finding"
            ],
            "hypotheses": ["a string", {"text": "an object wrapped"}, {"nope": 1}],
            "proposals": [{"checker": "mutation", "reason": "because"}]
        });
        let n = normalize_answer(v);
        assert_eq!(n["findings"].as_array().unwrap().len(), 2);
        assert_eq!(n["findings"][0]["severity"], "major");
        assert_eq!(n["findings"][1]["severity"], "minor");
        assert!(n["findings"][1].get("evidence").is_none());
        assert_eq!(n["hypotheses"].as_array().unwrap().len(), 2);
        assert_eq!(n["proposals"][0]["checker"], "mutation");
    }

    /// Regression for a Critical desk finding losing its severity in normalization. The desk
    /// consumer deserializes `findings[].severity` straight into `Severity`
    /// (crates/verdict-desks/src/lib.rs, `RawFinding`), and the core blocks it in the release lane,
    /// so a downgrade to `minor` silently changes a release decision.
    #[test]
    fn a_critical_finding_keeps_its_severity_and_still_blocks_the_release_lane() {
        use verdict_core::lane::Bar;
        use verdict_core::model::{Finding, Lane, Severity, Tier};
        use verdict_core::verdict::{FindingStatus, FindingView, is_blocking_with};

        let n = normalize_answer(serde_json::json!({
            "findings": [{
                "severity": "CRITICAL",
                "title": "the release artifact does not match its manifest",
                "evidence": [{"record": 2}]
            }]
        }));
        // The normalizer keeps the desk's severity instead of rewriting it to `minor`.
        assert_eq!(n["findings"][0]["severity"], "critical");
        // The consumer's exact deserialization: `RawFinding.severity: Severity`.
        let severity: Severity =
            serde_json::from_value(n["findings"][0]["severity"].clone()).unwrap();
        assert_eq!(severity, Severity::Critical);
        // The stored finding the consumer writes (`Record::Finding`, judgment tier).
        let view = FindingView {
            seq: 3,
            finding: Finding {
                id: "desk:engineer.critical".into(),
                severity,
                title: n["findings"][0]["title"].as_str().unwrap().to_string(),
                claim_seq: None,
                evidence: Vec::new(),
                tier: Tier::Judgment,
                blast: Some(Lane::Release),
                judgment: true,
                fix_hint: None,
            },
            status: FindingStatus::Open,
        };
        assert!(
            is_blocking_with(&view, Lane::Release, Bar::ZeroAboveNit),
            "a Critical desk finding must block the release lane"
        );
        // The same finding downgraded to minor would not block: the bug really changed a decision.
        let mut minor = view.clone();
        minor.finding.severity = Severity::Minor;
        assert!(!is_blocking_with(&minor, Lane::Release, Bar::ZeroAboveNit));
        // Unknown severities still fall back to `minor`; the fix widened the whitelist only.
        let n = normalize_answer(serde_json::json!({
            "findings": [{"severity": "catastrophic", "title": "t", "evidence": [{"record": 2}]}]
        }));
        assert_eq!(n["findings"][0]["severity"], "minor");
    }

    #[test]
    fn an_answer_cannot_override_the_observed_identity() {
        // The answer claims a model and a served vendor that the CLI's events do not report.
        let answer = serde_json::json!({
            "findings": [],
            "model": "claimed-model",
            "served": {"vendor": "someone-else", "model": "claimed-model"},
            "usage": {"input_tokens": 1}
        });
        let out = apply_observation(
            answer,
            Some("gpt-observed".into()),
            Some(serde_json::json!({"input_tokens": 9})),
        );
        assert_eq!(out["model"], "gpt-observed", "observation wins");
        assert_eq!(out["served"]["model"], "gpt-observed");
        assert!(
            out["served"].get("vendor").is_none(),
            "no vendor is observed"
        );
        assert_eq!(
            out["claimed"]["model"], "claimed-model",
            "the claim is kept, labelled"
        );
        assert_eq!(out["claimed"]["served"]["vendor"], "someone-else");
        assert_eq!(out["usage"]["input_tokens"], 9, "observed usage wins");

        // With nothing observed, the answer asserts no served identity at all.
        let answer = serde_json::json!({"findings": [], "model": "claimed-model", "served": {"vendor": "x"}});
        let out = apply_observation(answer, None, None);
        assert!(
            out.get("served").is_none(),
            "nothing was served as far as we know"
        );
        assert!(out.get("model").is_none());
        assert_eq!(out["claimed"]["model"], "claimed-model");
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
