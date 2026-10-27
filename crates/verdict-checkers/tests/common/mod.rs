//! Test harness: builds a fixture (a tiny text project) into a temp git repository, one baseline
//! commit and then the session's change on top, and runs checkers against it. No nested `.git`
//! folder is ever committed; the repository exists only in the temp folder.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tempfile::TempDir;
use verdict_checkers::{CheckContext, Checker, Unavailable};
use verdict_core::{
    ClaimSource, DecisionKind, Finding, Lane, Ledger, Policy, Record, SessionParams, Writer,
    start_session,
};

pub mod project;

pub const NAMES: [&str; 6] = verdict_checkers::CHECKER_NAMES;

pub fn fixture_dir(name: &str, kind: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
        .join(kind)
}

pub fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git must be installed to run these tests");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn copy_dir(src: &Path, dst: &Path) {
    if !src.is_dir() {
        return;
    }
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(e.path(), &to).unwrap();
        }
    }
}

/// A built fixture: the repo, an in-memory ledger and the session in it.
pub struct Built {
    pub dir: TempDir,
    pub home: TempDir,
    pub ledger: Ledger,
    pub session: String,
}

impl Built {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }
}

pub fn init_repo(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "fixture@example.invalid"]);
    git(dir, &["config", "user.name", "fixture"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "core.autocrlf", "false"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "baseline"]);
}

fn claim_source(s: &str) -> ClaimSource {
    match s {
        "final_message" => ClaimSource::FinalMessage,
        "commit_message" => ClaimSource::CommitMessage,
        "notes_file" => ClaimSource::NotesFile,
        "plan_line" => ClaimSource::PlanLine,
        "done_when" => ClaimSource::DoneWhen,
        other => panic!("unknown claim source {other}"),
    }
}

/// Build a fixture: `base/` committed, then `change/` laid over it and `deleted.txt` removed,
/// then the prompts, claims and (if `approve_exec`) the human's decision from `session.json`.
pub fn build(name: &str, kind: &str) -> Built {
    let fx = fixture_dir(name, kind);
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    copy_dir(&fx.join("base"), dir.path());
    init_repo(dir.path());

    let mut ledger = Ledger::open_in_memory().unwrap();
    let started = start_session(
        &mut ledger,
        dir.path(),
        &SessionParams {
            agent: "claude".into(),
            model: "fixture-model".into(),
            vendor: "anthropic".into(),
            home: Some(home.path().to_path_buf()),
        },
    )
    .unwrap();
    let session = started.id;

    copy_dir(&fx.join("change"), dir.path());
    if let Ok(list) = std::fs::read_to_string(fx.join("deleted.txt")) {
        for p in list.lines().map(str::trim).filter(|p| !p.is_empty()) {
            std::fs::remove_file(dir.path().join(p)).unwrap();
        }
    }

    if let Ok(text) = std::fs::read_to_string(fx.join("session.json")) {
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        for (i, p) in v["prompts"].as_array().into_iter().flatten().enumerate() {
            ledger
                .append(
                    Writer::Adapter,
                    Some(&session),
                    Record::Prompt {
                        ordinal: i as u32 + 1,
                        text: p.as_str().unwrap().to_string(),
                        agent: "claude".into(),
                        model: "fixture-model".into(),
                    },
                )
                .unwrap();
        }
        for c in v["claims"].as_array().into_iter().flatten() {
            let text = c["text"].as_str().unwrap().to_string();
            let names = verdict_core::part1c::extract_names(&text);
            ledger
                .append(
                    Writer::Agent,
                    Some(&session),
                    Record::Claim {
                        source: claim_source(c["source"].as_str().unwrap()),
                        text,
                        names,
                        prompt_ordinal: None,
                    },
                )
                .unwrap();
        }
        if v["approve_exec"].as_bool() == Some(true) {
            approve_exec(&mut ledger, &session);
        }
    }
    Built {
        dir,
        home,
        ledger,
        session,
    }
}

pub fn approve_exec(ledger: &mut Ledger, session: &str) {
    ledger
        .append(
            Writer::Human,
            Some(session),
            Record::Decision {
                decision: DecisionKind::RunProjectCommands,
                choice: "approve".into(),
                reason: None,
                who: Writer::Human,
            },
        )
        .unwrap();
}

pub fn context(b: &Built) -> CheckContext {
    let policy = Policy::load(b.root(), Some(b.home.path())).unwrap();
    CheckContext::from_ledger(
        &b.ledger,
        b.root(),
        &b.session,
        Lane::Standard,
        Duration::from_secs(60),
        policy,
    )
    .unwrap()
}

pub fn find<'a>(registry: &'a [Box<dyn Checker>], name: &str) -> &'a dyn Checker {
    registry
        .iter()
        .find(|c| c.name() == name)
        .unwrap_or_else(|| panic!("no checker {name}"))
        .as_ref()
}

pub fn run_checker(
    registry: &[Box<dyn Checker>],
    name: &str,
    b: &Built,
) -> Result<Vec<Finding>, Unavailable> {
    find(registry, name).run(&context(b))
}

/// The rule names the red fixture must produce (one per line of `expect.txt`).
pub fn expected(name: &str) -> Vec<String> {
    std::fs::read_to_string(fixture_dir(name, "red").join("expect.txt"))
        .unwrap_or_else(|_| panic!("fixtures/{name}/red/expect.txt is missing"))
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The rule inside a finding id (`checker.rule.hash`).
pub fn rule_of(id: &str) -> &str {
    // `<checker>:<rule>.<hash>`
    id.split_once(':')
        .and_then(|(_, rest)| rest.split('.').next())
        .unwrap_or("")
}

/// The red fixture must produce a finding for every expected rule, each with evidence.
pub fn check_red(registry: &[Box<dyn Checker>], name: &str) -> Result<Vec<Finding>, String> {
    let b = build(name, "red");
    let findings = run_checker(registry, name, &b)
        .map_err(|u| format!("{name} red fixture: checker unavailable: {}", u.reason))?;
    if findings.is_empty() {
        return Err(format!("{name} red fixture produced no findings"));
    }
    let rules: Vec<&str> = findings.iter().map(|f| rule_of(&f.id)).collect();
    for want in expected(name) {
        if !rules.contains(&want.as_str()) {
            return Err(format!(
                "{name} red fixture: no `{want}` finding (got {rules:?})"
            ));
        }
    }
    for f in &findings {
        if f.evidence.is_empty() {
            return Err(format!("{name}: finding {} has no evidence pointer", f.id));
        }
    }
    Ok(findings)
}

/// The green fixture must run and produce nothing.
pub fn check_green(registry: &[Box<dyn Checker>], name: &str) -> Result<(), String> {
    let b = build(name, "green");
    let findings = run_checker(registry, name, &b)
        .map_err(|u| format!("{name} green fixture: checker unavailable: {}", u.reason))?;
    if findings.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{name} green fixture produced findings: {:#?}",
            findings.iter().map(|f| &f.title).collect::<Vec<_>>()
        ))
    }
}
