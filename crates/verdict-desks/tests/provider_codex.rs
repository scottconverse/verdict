//! The Codex adapter speaks the generic provider protocol and records only observed identity.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn the_codex_adapter_returns_the_answer_and_only_observed_identity() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vdt-provider-codex"))
        .args([
            "--codex",
            env!("CARGO_BIN_EXE_vdt-codex-stub"),
            "--timeout-secs",
            "60",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::json!({
        "protocol": "verdict-desk/1",
        "model": "requested-1",
        "prompt": "rubric: answer with one JSON object"
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON answer");
    assert_eq!(answer["findings"], serde_json::json!([]));
    assert_eq!(answer["model"], "gpt-observed");
    assert_eq!(answer["served"]["model"], "gpt-observed");
    assert!(
        answer["served"].get("vendor").is_none(),
        "the vendor is never inferred: {answer}"
    );
    assert_eq!(answer["usage"]["input_tokens"], 3);
}
