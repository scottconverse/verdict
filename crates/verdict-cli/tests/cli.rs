use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::*;

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-cli-test-{}-{}",
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

fn vdt(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vdt"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn project() -> (Tmp, String) {
    let t = Tmp::new();
    let mut l = Ledger::open(&t.0).unwrap();
    let home = Tmp::new();
    let s = start_session(
        &mut l,
        &t.0,
        &SessionParams {
            agent: "claude".into(),
            model: "m".into(),
            vendor: "anthropic".into(),
            home: Some(home.0.clone()),
        },
    )
    .unwrap();
    l.append(
        Writer::Adapter,
        Some(&s.id),
        Record::Prompt {
            ordinal: 1,
            text: "do the thing".into(),
            agent: "claude".into(),
            model: "m".into(),
        },
    )
    .unwrap();
    l.append(
        Writer::Agent,
        Some(&s.id),
        Record::Claim {
            source: ClaimSource::FinalMessage,
            text: "thing done".into(),
            names: vec![],
            prompt_ordinal: Some(1),
        },
    )
    .unwrap();
    (t, s.id)
}

#[test]
fn version_still_works() {
    let t = Tmp::new();
    let o = vdt(&t.0, &["--version"]);
    assert!(o.status.success());
    assert!(text(&o).starts_with("vdt "));
}

#[test]
fn commands_work_from_a_subfolder_and_exit_zero() {
    let (t, id) = project();
    let sub = t.0.join("deep").join("er");
    std::fs::create_dir_all(&sub).unwrap();

    let o = vdt(&sub, &["ledger", "verify"]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("ledger OK: 3 records"));

    let o = vdt(&sub, &["sessions"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o).contains(&id));

    let o = vdt(&sub, &["report"]);
    assert_eq!(o.status.code(), Some(0));
    let r = text(&o);
    assert!(r.starts_with("VERDICT: UNVERIFIED - nothing was checked"));
    assert!(
        r.trim_end().ends_with(
            "- whether your build and tests pass, or your real app works: nothing was run"
        )
    );

    let o = vdt(&sub, &["report", "--session", &id]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(text(&o), r);

    let o = vdt(&sub, &["ledger", "export", "--jsonl"]);
    assert_eq!(o.status.code(), Some(0));
    let out = text(&o);
    assert_eq!(out.lines().count(), 3);
    for l in out.lines() {
        serde_json::from_str::<serde_json::Value>(l).unwrap();
    }
}

#[test]
fn unknown_session_exits_one() {
    let (t, _) = project();
    assert_eq!(
        vdt(&t.0, &["report", "--session", "s-nope"]).status.code(),
        Some(1)
    );
}

#[test]
fn tampering_exits_two_and_names_the_record() {
    let (t, _) = project();
    let c = rusqlite::Connection::open(t.0.join(".verdict").join("ledger.db")).unwrap();
    c.execute_batch("DROP TRIGGER records_no_update").unwrap();
    c.execute(
        "UPDATE records SET body = replace(body, 'do the thing', 'nothing') WHERE seq = 2",
        [],
    )
    .unwrap();
    drop(c);
    let o = vdt(&t.0, &["ledger", "verify"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        text(&o).contains("LEDGER BROKEN at record 2"),
        "{}",
        text(&o)
    );
}

#[test]
fn no_ledger_is_fine_and_creates_nothing() {
    let t = Tmp::new();
    for args in [
        &["ledger", "verify"][..],
        &["sessions"],
        &["report"],
        &["ledger", "export", "--jsonl"],
    ] {
        assert_eq!(vdt(&t.0, args).status.code(), Some(0), "{args:?}");
    }
    assert!(!t.0.join(".verdict").exists());
}
