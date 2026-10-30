//! `vdt gate`, `vdt verdict` and `vdt commit-anyway` as real processes, including a real
//! `git commit` through the installed hook. Every folder is a throwaway temp folder, and every child
//! gets a temp HOME so the real `~/.verdict` and `~/.claude` are never read or written.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::*;

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-gate-cli-{}-{}",
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

struct Repo {
    dir: Tmp,
    home: Tmp,
    sid: String,
}

fn vdt_exe() -> &'static str {
    env!("CARGO_BIN_EXE_vdt")
}

fn run(cmd: &mut Command, home: &Path) -> Output {
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // The person at the terminal types yes (debug builds read this for the test suite).
        .env("VDT_TEST_TYPED", "yes")
        .output()
        .expect("process runs")
}

fn vdt(dir: &Path, home: &Path, args: &[&str]) -> Output {
    run(Command::new(vdt_exe()).current_dir(dir).args(args), home)
}

fn git(dir: &Path, home: &Path, args: &[&str]) -> Output {
    run(
        Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args),
        home,
    )
}

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn repo() -> Repo {
    let dir = Tmp::new();
    let home = Tmp::new();
    assert!(git(&dir.0, &home.0, &["init", "-q"]).status.success());
    std::fs::write(dir.0.join("README.md"), "hello\n").unwrap();
    std::fs::write(dir.0.join(".gitignore"), ".verdict/\n").unwrap();
    assert!(git(&dir.0, &home.0, &["add", "."]).status.success());
    let c = git(&dir.0, &home.0, &["commit", "-q", "-m", "first"]);
    assert!(c.status.success(), "{}", out(&c));
    let mut l = Ledger::open(&dir.0).unwrap();
    // The same home as the vdt runs, so this handle and vdt use one seal key.
    l.set_seal_home(Some(home.0.clone()));
    let s = start_session(
        &mut l,
        &dir.0,
        &SessionParams {
            agent: "claude".into(),
            model: "m".into(),
            vendor: "anthropic".into(),
            home: Some(home.0.clone()),
        },
    )
    .unwrap();
    Repo {
        dir,
        home,
        sid: s.id,
    }
}

impl Repo {
    fn ledger(&self) -> Ledger {
        let mut l = Ledger::open(&self.dir.0).unwrap();
        l.set_seal_home(Some(self.home.0.clone()));
        l
    }
    fn vdt(&self, args: &[&str]) -> Output {
        vdt(&self.dir.0, &self.home.0, args)
    }
    fn git(&self, args: &[&str]) -> Output {
        git(&self.dir.0, &self.home.0, args)
    }
    fn failed(&self) {
        self.ledger()
            .append(
                Writer::Checker,
                Some(&self.sid),
                Record::Finding(Finding {
                    id: "wiring:src/a.ts:1".into(),
                    severity: Severity::Major,
                    title: "the button calls a route that does not exist".into(),
                    claim_seq: None,
                    evidence: vec![EvidencePointer::command("check", b"red")],
                    tier: Tier::Static,
                    blast: None,
                    judgment: false,
                    fix_hint: None,
                }),
            )
            .unwrap();
    }
    fn enforce(&self) {
        self.ledger()
            .append(
                Writer::Human,
                Some(&self.sid),
                Record::Decision {
                    decision: DecisionKind::ModeSwitch,
                    choice: "enforce".into(),
                    reason: None,
                    who: Writer::Human,
                },
            )
            .unwrap();
    }
    fn install_hook(&self) {
        let o = self.vdt(&[
            "gate",
            "install",
            "--vdt-path",
            vdt_exe(),
            "--i-am-the-human",
        ]);
        assert!(o.status.success(), "{}", out(&o));
    }
    fn stage(&self, rel: &str, body: &str) {
        let p = self.dir.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        assert!(self.git(&["add", rel]).status.success());
    }
    fn commits(&self) -> usize {
        let o = self.git(&["rev-list", "--count", "HEAD"]);
        String::from_utf8_lossy(&o.stdout).trim().parse().unwrap()
    }
}

#[test]
fn shadow_mode_installs_holds_nothing_and_records_what_it_would_have_done() {
    let r = repo();
    // Without the flag, nothing is written.
    let o = r.vdt(&["gate", "install"]);
    assert!(o.status.success());
    assert!(out(&o).contains("Nothing was changed"), "{}", out(&o));
    assert!(!r.dir.0.join(".git/hooks/pre-commit").exists());

    r.install_hook();
    assert!(r.dir.0.join(".git/hooks/pre-commit").is_file());
    r.failed();
    r.stage("src/a.ts", "export const a = 1;\n");
    let c = r.git(&["commit", "-q", "-m", "second"]);
    assert!(
        c.status.success(),
        "shadow mode must let the commit through: {}",
        out(&c)
    );
    assert_eq!(r.commits(), 2);
    let text = out(&c);
    assert!(text.contains("VERDICT: FAILED"), "{text}");
    assert!(text.contains("would be held"), "{text}");
    let notes: Vec<String> = r
        .ledger()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::Check { name, detail, .. } if name == "gate" => detail,
            _ => None,
        })
        .collect();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("would have held"), "{notes:?}");
}

#[test]
fn enforce_mode_holds_a_failed_commit_until_the_human_chooses_commit_anyway_with_a_reason() {
    let r = repo();
    r.install_hook();
    r.failed();
    r.enforce();
    r.stage("src/a.ts", "export const a = 1;\n");

    // Held: git itself refuses the commit.
    let c = r.git(&["commit", "-q", "-m", "second"]);
    assert!(
        !c.status.success(),
        "enforce mode must hold a FAILED commit"
    );
    assert_eq!(r.commits(), 1);
    let text = out(&c);
    assert!(text.contains("HELD"), "{text}");
    assert!(text.contains("vdt commit-anyway"), "{text}");

    // Without --i-am-the-human nothing is recorded; an empty reason is refused.
    let o = r.vdt(&["commit-anyway", "--reason", "hotfix"]);
    assert!(o.status.success());
    assert!(out(&o).contains("Nothing was recorded"), "{}", out(&o));
    let o = r.vdt(&["commit-anyway", "--reason", "  ", "--i-am-the-human"]);
    assert!(!o.status.success());
    assert!(out(&o).contains("reason"), "{}", out(&o));
    let count = |r: &Repo| {
        r.ledger()
            .records()
            .unwrap()
            .iter()
            .filter(|s| {
                matches!(
                    s.record,
                    Record::Decision {
                        decision: DecisionKind::CommitAnyway,
                        ..
                    }
                )
            })
            .count()
    };
    assert_eq!(count(&r), 0);
    // The flag alone is not a person: with no terminal (an agent's shell) or no "yes", nothing
    // is recorded.
    for typed in [None, Some("no")] {
        let mut c = Command::new(vdt_exe());
        c.current_dir(&r.dir.0)
            .args(["commit-anyway", "--reason", "hotfix", "--i-am-the-human"])
            .env("HOME", &r.home.0)
            .env("USERPROFILE", &r.home.0)
            .env_remove("VDT_TEST_TYPED")
            .stdin(Stdio::null());
        if let Some(typed) = typed {
            c.env("VDT_TEST_TYPED", typed);
        }
        let o = c.output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{}", out(&o));
        let why = if typed.is_none() {
            "Refused"
        } else {
            "did not type yes"
        };
        assert!(out(&o).contains(why), "{}", out(&o));
    }
    assert_eq!(count(&r), 0);
    assert!(!r.git(&["commit", "-q", "-m", "second"]).status.success());

    // The human types a reason.
    let o = r.vdt(&[
        "commit-anyway",
        "--reason",
        "the outage is worse than this bug",
        "--i-am-the-human",
    ]);
    assert!(o.status.success(), "{}", out(&o));
    assert_eq!(count(&r), 1);

    // The commit goes through, and the verdict stays FAILED.
    let c = r.git(&["commit", "-q", "-m", "second"]);
    assert!(c.status.success(), "{}", out(&c));
    assert_eq!(r.commits(), 2);
    assert!(out(&c).contains("verdict stays FAILED"), "{}", out(&c));
    let report = out(&r.vdt(&["report"]));
    assert!(report.starts_with("VERDICT: FAILED"), "{report}");
    assert!(report.contains("COMMIT ANYWAY"), "{report}");
    assert!(report.contains("the outage is worse than this bug"));

    // A different staged tree needs its own decision.
    r.stage("src/b.ts", "export const b = 2;\n");
    assert!(!r.git(&["commit", "-q", "-m", "third"]).status.success());
    assert_eq!(r.commits(), 2);
    assert!(r.ledger().verify().unwrap().is_ok());
}

#[test]
fn the_hook_fails_open_when_vdt_cannot_be_found() {
    let r = repo();
    let o = r.vdt(&[
        "gate",
        "install",
        "--vdt-path",
        "/nowhere/at/all/vdt-missing",
        "--i-am-the-human",
    ]);
    assert!(o.status.success(), "{}", out(&o));
    r.failed();
    r.enforce();
    r.stage("src/a.ts", "x\n");
    // Take every folder that holds a `vdt` off this git's PATH, so the hook cannot call it. The
    // commit must still go through: that is failing open on the tool.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kept: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|d| !d.join("vdt.exe").exists() && !d.join("vdt").exists())
        .collect();
    let c = run(
        Command::new("git")
            .current_dir(&r.dir.0)
            .env("PATH", std::env::join_paths(kept).unwrap())
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["commit", "-q", "-m", "second"]),
        &r.home.0,
    );
    assert!(c.status.success(), "{}", out(&c));
    assert!(
        out(&c).contains("pre-commit gate was skipped"),
        "{}",
        out(&c)
    );
    assert_eq!(r.commits(), 2);
}

#[test]
fn a_pause_lets_the_commit_through_and_says_so() {
    let r = repo();
    r.install_hook();
    r.failed();
    r.enforce();
    r.ledger()
        .append(
            Writer::Human,
            Some(&r.sid),
            Record::Decision {
                decision: DecisionKind::Pause,
                choice: "1h".into(),
                reason: None,
                who: Writer::Human,
            },
        )
        .unwrap();
    r.stage("src/a.ts", "x\n");
    let c = r.git(&["commit", "-q", "-m", "second"]);
    assert!(c.status.success(), "{}", out(&c));
    assert!(out(&c).contains("paused by you"), "{}", out(&c));
    assert_eq!(r.commits(), 2);
}

#[test]
fn vdt_verdict_computes_and_writes_a_core_verdict_and_report_shows_it() {
    let r = repo();
    // Nothing checked yet.
    let o = r.vdt(&["verdict"]);
    assert!(o.status.success(), "{}", out(&o));
    let text = out(&o);
    assert!(text.starts_with("VERDICT: UNVERIFIED"), "{text}");
    assert!(text.contains("written by: core"));
    let verdicts: Vec<_> = r
        .ledger()
        .records()
        .unwrap()
        .into_iter()
        .filter(|s| matches!(s.record, Record::Verdict { .. }))
        .collect();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].writer, Writer::Core);
    assert!(!out(&r.vdt(&["report"])).contains("not in this build"));

    // With a blocking finding it says FAILED, for a named session.
    r.failed();
    let o = r.vdt(&["verdict", "--session", &r.sid]);
    assert!(out(&o).starts_with("VERDICT: FAILED"), "{}", out(&o));
    // An unknown session exits non-zero.
    assert!(!r.vdt(&["verdict", "--session", "s-nope"]).status.success());
    // Without a wired checker runner, --run records every required checker as unavailable.
    let r2 = repo();
    let o = r2.vdt(&["verdict", "--run"]);
    assert!(out(&o).starts_with("VERDICT: PARTIAL"), "{}", out(&o));
}

#[test]
fn a_tool_error_in_the_gate_never_holds_a_commit() {
    // A ledger file that is not a database: the gate cannot read it, so it lets the commit through
    // and says why.
    let r = repo();
    r.install_hook();
    r.stage("src/a.ts", "x\n");
    std::fs::write(
        r.dir.0.join(".verdict/ledger.db"),
        b"this is not sqlite at all",
    )
    .unwrap();
    let _ = std::fs::remove_file(r.dir.0.join(".verdict/ledger.db-wal"));
    let _ = std::fs::remove_file(r.dir.0.join(".verdict/ledger.db-shm"));
    let c = r.git(&["commit", "-q", "-m", "second"]);
    assert!(c.status.success(), "{}", out(&c));
    assert_eq!(r.commits(), 2);
}
