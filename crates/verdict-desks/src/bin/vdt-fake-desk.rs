//! A stub provider for Verdict's own tests only. It speaks the desk protocol: read one JSON
//! request from stdin, write one JSON answer to stdout. Nothing in the product calls it.
//!
//! Flags (each takes one value and may appear once):
//! * `--record N`    cite this ledger record number (default 1).
//! * `--file p:l`    also cite this file:line.
//! * `--severity s`  severity of the finding (default `major`).
//! * `--fail 1`      answer with nonsense; `--fail 2` exits 2 with a message.
//! * `--sleep-ms N`  sleep before answering.
//! * `--model M`     the model the answer says served it.
//! * `--uncited`     answer with a finding that cites nothing.

use std::io::{Read, Write};

struct Args {
    record: u64,
    file: Option<String>,
    severity: String,
    fail: u8,
    sleep_ms: u64,
    model: Option<String>,
    served_vendor: Option<String>,
    pad: usize,
    uncited: bool,
}

fn args() -> Args {
    let mut a = Args {
        record: 1,
        file: None,
        severity: "major".into(),
        fail: 0,
        sleep_ms: 0,
        model: None,
        served_vendor: None,
        pad: 0,
        uncited: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--record" => a.record = it.next().and_then(|v| v.parse().ok()).unwrap_or(1),
            "--file" => a.file = it.next(),
            "--severity" => a.severity = it.next().unwrap_or_else(|| "major".into()),
            "--fail" => a.fail = it.next().and_then(|v| v.parse().ok()).unwrap_or(1),
            "--sleep-ms" => a.sleep_ms = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--model" => a.model = it.next(),
            "--served-vendor" => a.served_vendor = it.next(),
            "--pad" => a.pad = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--uncited" => a.uncited = true,
            _ => {}
        }
    }
    a
}

fn main() {
    let a = args();
    let mut request = String::new();
    if std::io::stdin().read_to_string(&mut request).is_err() {
        std::process::exit(2);
    }
    let parsed: serde_json::Value = match serde_json::from_str(&request) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("the request was not JSON");
            std::process::exit(2);
        }
    };
    let prompt = parsed.get("prompt").and_then(|p| p.as_str()).unwrap_or("");
    if !prompt.contains("Rubric") && !prompt.contains("rubric") {
        eprintln!("no rubric in the request");
        std::process::exit(2);
    }
    if a.sleep_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(a.sleep_ms));
    }
    match a.fail {
        1 => {
            print!("this is not the JSON the protocol asks for");
            return;
        }
        2 => {
            eprintln!("the fake provider was told to fail");
            std::process::exit(2);
        }
        _ => {}
    }
    let mut evidence = vec![serde_json::json!({"record": a.record})];
    if let Some(path) = &a.file
        && let Some((file, line)) = path.rsplit_once(':')
        && let Ok(line) = line.parse::<u64>()
    {
        evidence.push(serde_json::json!({"file": file, "line": line}));
    }
    if a.pad > 0 {
        let padding = " ".repeat(a.pad);
        print!("{padding}");
    }
    let mut answer = serde_json::json!({
        "findings": [
            {
                "severity": a.severity,
                "title": if a.uncited {
                    "an uncited concern".to_string()
                } else {
                    "a cited concern".to_string()
                },
                "evidence": if a.uncited { serde_json::json!([]) } else { serde_json::json!(evidence) },
                "fix": "do the thing"
            }
        ],
        "hypotheses": ["a second opinion that cites nothing"],
        "model": a.model,
        "usage": {"input_tokens": 11, "output_tokens": 7}
    });
    if let Some(vendor) = &a.served_vendor {
        answer["served"] = serde_json::json!({
            "vendor": vendor,
            "model": a.model,
            "settings": {"temperature": 0}
        });
    }
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{answer}");
}
