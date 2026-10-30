//! The `vdt hook` and `vdt hooks` commands as the hosts and the human run them: real processes.

use proptest::prelude::*;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-hooks-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CLAUDE: &str = include_str!("../../verdict-adapters/tests/fixtures/claude-session.json");
const CODEX: &str = include_str!("../../verdict-adapters/tests/fixtures/codex-session.json");

fn command(dir: &Path, home: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_vdt"));
    c.args(args)
        .current_dir(dir)
        .env("USERPROFILE", home)
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        // The person at the terminal types yes (debug builds read this for the test suite).
        .env("VDT_TEST_TYPED", "yes");
    c
}

fn hook(dir: &Path, home: &Path, host: &str, event: &str, stdin: &[u8]) -> Output {
    let mut child = command(dir, home, &["hook", host, event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut si = child.stdin.take().unwrap();
    let _ = si.write_all(stdin);
    drop(si);
    child.wait_with_output().unwrap()
}

fn assert_silent_success(o: &Output) {
    assert_eq!(
        o.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        o.stdout.is_empty(),
        "stdout must stay empty: {:?}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(
        o.stderr.is_empty(),
        "stderr: {:?}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn play(dir: &Path, home: &Path, host: &str, fixture: &str) {
    let escaped = serde_json::to_string(&dir.to_string_lossy().to_string()).unwrap();
    let escaped = escaped.trim_matches('"');
    let steps: serde_json::Value = serde_json::from_str(fixture).unwrap();
    for step in steps.as_array().unwrap() {
        let payload = serde_json::to_string(&step["payload"])
            .unwrap()
            .replace("__CWD__", escaped);
        let o = hook(
            dir,
            home,
            host,
            step["event"].as_str().unwrap(),
            payload.as_bytes(),
        );
        assert_silent_success(&o);
    }
}

fn vdt_text(dir: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    let o = command(dir, home, args).output().unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8_lossy(&o.stdout).to_string(),
    )
}

#[test]
fn a_hook_exits_zero_and_prints_nothing_for_every_kind_of_bad_call() {
    let (t, h) = (Tmp::new(), Tmp::new());
    for (args, stdin) in [
        (vec!["hook"], &b""[..]),
        (vec!["hook", "claude"], &b"{}"[..]),
        (vec!["hook", "claude", "Stop"], &b"not json at all"[..]),
        (vec!["hook", "codex", "PostToolUse"], &b"\xff\xfe\x00"[..]),
        (
            vec!["hook", "nope", "nothing", "--flag", "--version"],
            &b"{}"[..],
        ),
        (vec!["hook", "--help"], &b""[..]),
        (
            vec!["hook", "codex", "Stop"],
            &br#"{"decision":"block"}"#[..],
        ),
    ] {
        let mut child = command(&t.0, &h.0, &args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut si = child.stdin.take().unwrap();
        let _ = si.write_all(stdin);
        drop(si);
        assert_silent_success(&child.wait_with_output().unwrap());
    }
}

#[test]
fn a_stdin_that_never_closes_ends_at_the_budget_with_exit_zero_and_a_record() {
    let (t, h) = (Tmp::new(), Tmp::new());
    let start = Instant::now();
    let mut child = command(&t.0, &h.0, &["hook", "codex", "Stop"])
        .env("VDT_HOOK_BUDGET_MS", "400")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _keep_open = child.stdin.take().unwrap(); // never written, never closed
    let out = loop {
        if child.try_wait().unwrap().is_some() {
            break child.wait_with_output().unwrap();
        }
        assert!(start.elapsed() < Duration::from_secs(15), "the hook hung");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_silent_success(&out);
    assert!(start.elapsed() < Duration::from_secs(10));
    // The hook's own wait for writing the record is the unchanged UNAVAILABLE_BUDGET; poll the
    // ledger with a bounded deadline instead of racing it, so a loaded machine cannot make this
    // assertion flaky. The assertion itself is unchanged.
    let deadline = Duration::from_secs(10);
    let mut text = String::new();
    let polled = Instant::now();
    while polled.elapsed() < deadline {
        let (_, t) = vdt_text(&t.0, &h.0, &["ledger", "export", "--jsonl"]);
        text = t;
        if text.contains("check_unavailable") && text.contains("did not arrive") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(text.contains("check_unavailable"), "{text}");
    assert!(text.contains("did not arrive"), "{text}");
}

#[test]
fn a_recorded_claude_session_shows_in_vdt_report_as_unverified_and_not_established() {
    let (t, h) = (Tmp::new(), Tmp::new());
    play(&t.0, &h.0, "claude", CLAUDE);
    let (code, report) = vdt_text(&t.0, &h.0, &["report"]);
    assert_eq!(code, 0, "{report}");
    println!("{report}");
    // The Stop hook ran fresh verification, so this is PARTIAL (a required desk could not run),
    // never clean. The point of the test is the same: nothing was proven.
    assert!(report.contains("VERDICT: PARTIAL"), "{report}");
    assert!(
        report.contains("builder: claude/claude-sonnet-5-5 [anthropic]"),
        "{report}"
    );
    assert!(report.contains("add a login page"), "{report}");
    assert!(report.contains("cargo test"), "{report}");
    assert!(report.contains("src/login.rs"), "{report}");
    assert!(report.contains("Login page added; tests pass."), "{report}");
    assert!(report.contains("NOT ESTABLISHED"), "{report}");
    assert!(
        !report.to_lowercase().contains("verified (") && !report.contains("CLEAR"),
        "{report}"
    );
    let (code, v) = vdt_text(&t.0, &h.0, &["ledger", "verify"]);
    assert_eq!(code, 0, "{v}");
    let (_, sessions) = vdt_text(&t.0, &h.0, &["sessions"]);
    assert_eq!(
        sessions.lines().count(),
        1,
        "one ledger session: {sessions}"
    );
}

#[test]
fn a_recorded_codex_session_reports_the_same_way() {
    let (t, h) = (Tmp::new(), Tmp::new());
    play(&t.0, &h.0, "codex", CODEX);
    let (code, report) = vdt_text(&t.0, &h.0, &["report"]);
    assert_eq!(code, 0, "{report}");
    assert!(
        report.contains("builder: codex/gpt-5.5-codex [openai]"),
        "{report}"
    );
    assert!(report.contains("add a login page"), "{report}");
    assert!(report.contains("NOT ESTABLISHED"), "{report}");
}

#[test]
fn hooks_install_prints_without_the_flag_and_installs_with_it() {
    let (t, h) = (Tmp::new(), Tmp::new());
    let (code, text) = vdt_text(&t.0, &h.0, &["hooks", "install", "claude"]);
    assert_eq!(code, 0);
    assert!(
        text.contains("Nothing was changed") && text.contains("vdt hook claude Stop"),
        "{text}"
    );
    assert!(!t.0.join(".claude").exists());

    // The flag alone is not a person: with no terminal (an agent's shell) or no "yes", nothing
    // is written.
    for typed in [None, Some("no")] {
        let mut c = command(
            &t.0,
            &h.0,
            &["hooks", "install", "claude", "--i-am-the-human"],
        );
        c.stdin(Stdio::null()).env_remove("VDT_TEST_TYPED");
        if let Some(typed) = typed {
            c.env("VDT_TEST_TYPED", typed);
        }
        let o = c.output().unwrap();
        let text = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(1), "{text}");
        let why = if typed.is_none() {
            "Refused"
        } else {
            "did not type yes"
        };
        assert!(text.contains(why), "{text}");
        assert!(!t.0.join(".claude").exists(), "{text}");
    }

    let (code, text) = vdt_text(
        &t.0,
        &h.0,
        &["hooks", "install", "claude", "--i-am-the-human"],
    );
    assert_eq!(code, 0, "{text}");
    assert!(t.0.join(".claude").join("settings.json").is_file());

    let (code, text) = vdt_text(
        &t.0,
        &h.0,
        &["hooks", "install", "codex", "--i-am-the-human"],
    );
    assert_eq!(code, 0, "{text}");
    assert!(
        h.0.join(".codex").join("hooks.json").is_file(),
        "CODEX_HOME is honored: {text}"
    );

    let (code, text) = vdt_text(
        &t.0,
        &h.0,
        &["hooks", "uninstall", "claude", "--i-am-the-human"],
    );
    assert_eq!(code, 0, "{text}");
    let (_, export) = vdt_text(&t.0, &h.0, &["ledger", "export", "--jsonl"]);
    assert!(
        export.contains("install_hooks") && export.contains("\"human\""),
        "{export}"
    );

    let (code, _) = vdt_text(&t.0, &h.0, &["hooks", "install", "gemini"]);
    assert_eq!(
        code, 1,
        "an unknown host is a plain error for the human command"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, failure_persistence: None, .. ProptestConfig::default() })]

    /// Whatever arrives on stdin, the real process exits 0, prints nothing, and does not hang.
    #[test]
    fn no_stdin_makes_the_hook_process_fail_or_speak(
        bytes in prop::collection::vec(any::<u8>(), 0..300),
        event in prop::sample::select(vec!["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "Bogus"]),
        host in prop::sample::select(vec!["claude", "codex", "bogus"]),
    ) {
        let (t, h) = (Tmp::new(), Tmp::new());
        let start = Instant::now();
        let o = hook(&t.0, &h.0, host, event, &bytes);
        prop_assert_eq!(o.status.code(), Some(0));
        prop_assert!(o.stdout.is_empty());
        prop_assert!(o.stderr.is_empty());
        prop_assert!(start.elapsed() < Duration::from_secs(15));
    }
}
