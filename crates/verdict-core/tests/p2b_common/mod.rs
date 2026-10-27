#![allow(dead_code)]
//! Test support for the Part 2b tests: sessions, checks and findings the way a checker writes
//! them, a fake checker runner, and throwaway git repositories.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::gate::{CheckRunner, CheckSummary, RunOptions};
use verdict_core::lane::lane_name;
use verdict_core::report_head::Head;
use verdict_core::verdict::{Computed, VerdictInput, compute};
use verdict_core::*;

pub const SID: &str = "s-test000001";

static N: AtomicU64 = AtomicU64::new(0);

pub struct Tmp(pub PathBuf);
impl Tmp {
    pub fn new(label: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-p2b-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A clock that ticks one second per record, so ledgers and golden reports are reproducible.
pub fn clock() -> impl Fn() -> String + Send + 'static {
    let n = AtomicU64::new(0);
    move || {
        let s = n.fetch_add(1, Ordering::Relaxed);
        format!("2026-10-26T12:{:02}:{:02}.000Z", s / 60, s % 60)
    }
}

pub fn session_record(id: &str) -> Record {
    Record::Session {
        id: id.into(),
        started_at: "2026-10-26T12:00:00.000Z".into(),
        baseline: baseline_snapshot_stub(),
        agent: "claude".into(),
        model: "model-x".into(),
        vendor: "anthropic".into(),
        policy_version: 1,
        policy_hash: "p".repeat(64),
    }
}

fn baseline_snapshot_stub() -> Baseline {
    Baseline {
        hash: "b".repeat(64),
        is_git_repo: false,
        git: None,
        note: Some("not a git repository".into()),
        folder: None,
    }
}

/// A fresh file ledger with a fixed clock and one session, with a prompt and a claim.
pub fn seeded(root: &Path) -> Ledger {
    let mut l = Ledger::open(root).unwrap();
    l.set_clock(clock());
    l.append(Writer::Adapter, Some(SID), session_record(SID))
        .unwrap();
    l.append(
        Writer::Adapter,
        Some(SID),
        Record::Prompt {
            ordinal: 1,
            text: "add a login page".into(),
            agent: "claude".into(),
            model: "model-x".into(),
        },
    )
    .unwrap();
    l.append(
        Writer::Agent,
        Some(SID),
        Record::Claim {
            source: ClaimSource::FinalMessage,
            text: "login route exists in src/login.rs".into(),
            names: vec!["src/login.rs".into()],
            prompt_ordinal: Some(1),
        },
    )
    .unwrap();
    l.append(
        Writer::Adapter,
        Some(SID),
        Record::ToolCall {
            tool: "edit".into(),
            target: "src/login.rs".into(),
            output_hash: "0".repeat(64),
            duration_ms: 5,
        },
    )
    .unwrap();
    l
}

/// A check record, written by the core (the ledger lets only the core write checks).
pub fn check(l: &mut Ledger, name: &str, tier: Tier, outcome: CheckOutcome) -> u64 {
    l.append(
        Writer::Core,
        Some(SID),
        Record::Check {
            name: name.into(),
            tier,
            inputs_hash: "h".into(),
            started_at: "2026-10-26T12:00:00.000Z".into(),
            finished_at: "2026-10-26T12:00:01.000Z".into(),
            outcome,
            detail: match outcome {
                CheckOutcome::Ran => None,
                CheckOutcome::FailedToRun => Some("could not start".into()),
                CheckOutcome::SkippedByPolicy => Some("policy did not require it".into()),
            },
        },
    )
    .unwrap()
    .seq
}

pub fn ran(l: &mut Ledger, name: &str, tier: Tier) -> u64 {
    check(l, name, tier, CheckOutcome::Ran)
}

pub fn unavailable(l: &mut Ledger, component: &str) -> u64 {
    l.append(
        Writer::Core,
        Some(SID),
        Record::CheckUnavailable {
            component: component.into(),
            reason: "the tool is not installed".into(),
        },
    )
    .unwrap()
    .seq
}

pub fn finding_record(
    id: &str,
    severity: Severity,
    tier: Tier,
    judgment: bool,
    evidence: bool,
) -> Finding {
    Finding {
        id: id.into(),
        severity,
        title: format!("problem {id}"),
        claim_seq: None,
        evidence: if evidence {
            vec![EvidencePointer::command("cargo test", id.as_bytes())]
        } else {
            vec![]
        },
        tier,
        blast: None,
        judgment,
        fix_hint: None,
    }
}

/// A finding from a deterministic checker (writer `checker`).
pub fn find(l: &mut Ledger, id: &str, severity: Severity, tier: Tier) -> u64 {
    l.append(
        Writer::Checker,
        Some(SID),
        Record::Finding(finding_record(id, severity, tier, false, true)),
    )
    .unwrap()
    .seq
}

/// A judgment finding from a desk (writer `desk`).
pub fn desk_find(l: &mut Ledger, id: &str, severity: Severity) -> u64 {
    l.append(
        Writer::Desk,
        Some(SID),
        Record::Finding(finding_record(id, severity, Tier::Judgment, true, true)),
    )
    .unwrap()
    .seq
}

pub fn human(l: &mut Ledger, decision: DecisionKind, choice: &str, reason: Option<&str>) -> u64 {
    l.append(
        Writer::Human,
        Some(SID),
        Record::Decision {
            decision,
            choice: choice.into(),
            reason: reason.map(str::to_string),
            who: Writer::Human,
        },
    )
    .unwrap()
    .seq
}

/// The verdict input for `lane` under the built-in default policy.
pub fn input(lane: Lane) -> VerdictInput {
    let policy = Policy::builtin().unwrap();
    VerdictInput::from_policy(
        &policy,
        verdict_core::lane::Classification {
            lane,
            reasons: vec![format!("test: {} lane", lane_name(lane))],
            warnings: vec![],
        },
    )
}

pub fn verdict_of(l: &Ledger, lane: Lane) -> Computed {
    compute(l, SID, &input(lane)).unwrap()
}

/// Run every checker of `lane` at `tier` (static for all but exec/mutation, which run at `ran`;
/// firstrun at `browsed`), leaving the ledger with every required checker satisfied.
pub fn run_all_required(l: &mut Ledger, lane: Lane) {
    for name in input(lane).lane_checkers[&lane].clone() {
        let tier = match name.as_str() {
            "exec" | "mutation" => Tier::Ran,
            "firstrun" => Tier::Browsed,
            "release" => Tier::Asset,
            _ => Tier::Static,
        };
        ran(l, &name, tier);
    }
}

pub fn report_with(l: &Ledger, c: &Computed) -> String {
    render_report_with(l, Some(c.session.as_str()), Some(c)).unwrap()
}

pub fn head_from(l: &Ledger) -> Option<Head> {
    Head::from_stored(l, SID).unwrap()
}

// ----------------------------------------------------------------------- fake runner

/// What a fake checker does.
#[derive(Clone, Debug)]
pub struct Fake {
    pub outcome: CheckOutcome,
    pub tier: Tier,
    /// Findings to write as the checker: (id, severity).
    pub findings: Vec<(String, Severity)>,
}

/// A checker runner that writes check and finding records the way the real one will: the check
/// record as the core (the ledger only lets the core write checks), findings as the checker.
/// Checkers it has no script for are not run and not mentioned.
#[derive(Default)]
pub struct FakeRunner {
    pub script: std::sync::Mutex<BTreeMap<String, Fake>>,
    pub seen: std::sync::Mutex<Vec<(Vec<String>, Lane)>>,
    pub fail_with: Option<String>,
}

impl FakeRunner {
    pub fn new() -> FakeRunner {
        FakeRunner::default()
    }
    pub fn set(&self, name: &str, tier: Tier, findings: &[(&str, Severity)]) {
        self.script.lock().unwrap().insert(
            name.to_string(),
            Fake {
                outcome: CheckOutcome::Ran,
                tier,
                findings: findings.iter().map(|(i, s)| (i.to_string(), *s)).collect(),
            },
        );
    }
    pub fn set_all_static(&self, checkers: &[&str]) {
        for c in checkers {
            self.set(c, Tier::Static, &[]);
        }
    }
}

impl CheckRunner for FakeRunner {
    fn run_lane(
        &self,
        ledger: &mut Ledger,
        _root: &Path,
        session: &str,
        checkers: &[String],
        opts: &RunOptions,
    ) -> Result<Vec<CheckSummary>, String> {
        if let Some(e) = &self.fail_with {
            return Err(e.clone());
        }
        self.seen
            .lock()
            .unwrap()
            .push((checkers.to_vec(), opts.lane));
        let script = self.script.lock().unwrap().clone();
        let mut out = Vec::new();
        for name in checkers {
            let Some(f) = script.get(name) else { continue };
            let mut n = 0;
            for (id, sev) in &f.findings {
                ledger
                    .append(
                        Writer::Checker,
                        Some(session),
                        Record::Finding(Finding {
                            id: id.clone(),
                            severity: *sev,
                            title: format!("problem {id}"),
                            claim_seq: None,
                            evidence: vec![EvidencePointer::command(id, b"out")],
                            tier: f.tier,
                            blast: None,
                            judgment: false,
                            fix_hint: None,
                        }),
                    )
                    .map_err(|e| e.to_string())?;
                n += 1;
            }
            let seq = ledger
                .append(
                    Writer::Core,
                    Some(session),
                    Record::Check {
                        name: name.clone(),
                        tier: f.tier,
                        inputs_hash: "h".into(),
                        started_at: "2026-10-26T12:00:00.000Z".into(),
                        finished_at: "2026-10-26T12:00:01.000Z".into(),
                        outcome: f.outcome,
                        detail: None,
                    },
                )
                .map_err(|e| e.to_string())?
                .seq;
            out.push(CheckSummary {
                name: name.clone(),
                outcome: f.outcome,
                findings: n,
                check_seq: seq,
            });
        }
        Ok(out)
    }
}

// ----------------------------------------------------------------------- git

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A git repository in a temp folder with one commit.
pub fn git_repo(label: &str) -> Tmp {
    let t = Tmp::new(label);
    git(t.path(), &["init", "-q"]);
    std::fs::write(t.path().join("README.md"), "hello\n").unwrap();
    std::fs::write(t.path().join(".gitignore"), ".verdict/\n").unwrap();
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-q", "-m", "first"]);
    t
}

/// Open a ledger in the repo and start a session the way an adapter does.
pub fn repo_ledger(root: &Path, home: &Path) -> (Ledger, String) {
    let mut l = Ledger::open(root).unwrap();
    let s = start_session(
        &mut l,
        root,
        &SessionParams {
            agent: "claude".into(),
            model: "model-x".into(),
            vendor: "anthropic".into(),
            home: Some(home.to_path_buf()),
        },
    )
    .unwrap();
    (l, s.id)
}

pub fn finding_in(l: &mut Ledger, sid: &str, id: &str, severity: Severity) -> u64 {
    l.append(
        Writer::Checker,
        Some(sid),
        Record::Finding(finding_record(id, severity, Tier::Static, false, true)),
    )
    .unwrap()
    .seq
}

pub fn human_in(
    l: &mut Ledger,
    sid: &str,
    decision: DecisionKind,
    choice: &str,
    reason: Option<&str>,
) -> u64 {
    l.append(
        Writer::Human,
        Some(sid),
        Record::Decision {
            decision,
            choice: choice.into(),
            reason: reason.map(str::to_string),
            who: Writer::Human,
        },
    )
    .unwrap()
    .seq
}
