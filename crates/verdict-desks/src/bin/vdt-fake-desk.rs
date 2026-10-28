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
    spawn_child: Option<String>,
    spawn_early_child: Option<String>,
    helper: bool,
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
        spawn_child: None,
        spawn_early_child: None,
        helper: false,
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
            "--spawn-child" => a.spawn_child = it.next(),
            "--spawn-early-child" => a.spawn_early_child = it.next(),
            "--helper" => a.helper = true,
            _ => {}
        }
    }
    a
}

/// Start a long-lived copy of this program that holds the inherited output pipes, and record its
/// pid so a test can prove it was ended too.
fn spawn_helper(path: &str) {
    let exe = std::env::current_exe().unwrap_or_default();
    match std::process::Command::new(exe)
        .args(["--helper"])
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            if let Err(e) = std::fs::write(path, child.id().to_string()) {
                eprintln!("cannot write the helper pid: {e}");
            }
        }
        Err(e) => eprintln!("cannot start the helper: {e}"),
    }
}

fn main() {
    let a = args();
    // The pipe-holding helper: it never reads the request, it just stays alive holding the
    // inherited output pipes until the runner ends the tree.
    if a.helper {
        std::thread::sleep(std::time::Duration::from_secs(600));
        return;
    }
    // A provider that starts a helper before it reads its request: on Windows the job object is
    // assigned after spawn, so this helper can predate job membership. It must still be ended.
    if let Some(path) = &a.spawn_early_child {
        spawn_helper(path);
    }
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
    // A helper that outlives this process unless the runner ends the whole tree, holding the
    // inherited output pipes exactly as a real provider's subprocess would. It is started after
    // the request is read, like a real provider's helper would be.
    if let Some(path) = &a.spawn_child {
        spawn_helper(path);
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
