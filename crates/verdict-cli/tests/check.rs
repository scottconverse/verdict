//! `vdt check --checkers ...`: runs checkers on the newest session and prints one line each.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-check-test-{}-{}",
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

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn vdt(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vdt"))
        .args(args)
        .current_dir(dir)
        // Keep a developer's own global policy out of the test.
        .env("USERPROFILE", dir)
        .env("HOME", dir)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

#[test]
fn check_prints_one_line_per_checker_and_finds_a_new_problem() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.0.join("src")).unwrap();
    std::fs::write(t.0.join("src/app.js"), "export const a = 1;\n").unwrap();
    git(&t.0, &["init", "-q"]);
    git(&t.0, &["config", "user.email", "t@example.invalid"]);
    git(&t.0, &["config", "user.name", "t"]);
    git(&t.0, &["config", "commit.gpgsign", "false"]);
    git(&t.0, &["add", "-A"]);
    git(&t.0, &["commit", "-q", "-m", "base"]);

    // First run: starts the session, finds nothing.
    let first = vdt(&t.0, &["check", "--checkers", "regressions,unfinished"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let out = text(&first);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(lines[0].starts_with("regressions"), "{out}");
    assert!(lines[0].contains("ran, 0 finding(s)"), "{out}");
    assert!(lines[1].starts_with("unfinished"), "{out}");

    // The session's change, then run again on the newest session.
    std::fs::write(t.0.join("src/app.js"), "export const a = 1;\ndebugger;\n").unwrap();
    let second = vdt(
        &t.0,
        &[
            "check",
            "--checkers",
            "regressions",
            "--project",
            t.0.to_str().unwrap(),
        ],
    );
    assert!(second.status.success());
    assert!(
        text(&second).contains("ran, 1 finding(s)"),
        "{}",
        text(&second)
    );

    // The ledger holds what was printed, and still verifies.
    let verify = vdt(&t.0, &["ledger", "verify"]);
    assert!(verify.status.success(), "{}", text(&verify));
    let export = text(&vdt(&t.0, &["ledger", "export", "--jsonl"]));
    assert!(
        export.contains("\"kind\":\"finding\""),
        "no finding in the ledger"
    );
    assert!(export.contains("\"writer\":\"checker\""));
}

#[test]
fn unknown_names_and_lanes_are_reported_not_hidden() {
    let t = Tmp::new();
    std::fs::write(t.0.join("a.txt"), "a\n").unwrap();
    let o = vdt(&t.0, &["check", "--checkers", "mutation"]);
    assert!(o.status.success());
    assert!(
        text(&o).contains("could not run (check_unavailable)"),
        "{}",
        text(&o)
    );
    let bad = vdt(
        &t.0,
        &["check", "--checkers", "regressions", "--lane", "nope"],
    );
    assert_eq!(bad.status.code(), Some(1));
}
