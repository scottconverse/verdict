//! Fresh verification from the engine: the Stop/MCP path runs the lane's checkers and desks, and
//! every check in one run is stamped with the snapshot the context cached (F01, F03, F14).

use std::time::Duration;

use verdict_core::{
    CheckOutcome, DecisionKind, Lane, Ledger, Policy, Record, SessionParams, Tier, Writer,
    start_session,
};
use verdict_engine::{VerifyOptions, verify};

struct Tmp(std::path::PathBuf);
impl Tmp {
    fn new(label: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-engine-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project(label: &str, policy: &str) -> (Tmp, Tmp, Ledger, String) {
    let dir = Tmp::new(label);
    let home = Tmp::new(&format!("{label}-home"));
    std::fs::write(dir.path().join("verdict.json"), policy).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "f20@example.invalid"]);
    git(&["config", "user.name", "f20"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "baseline"]);
    let mut ledger = Ledger::open(dir.path()).unwrap();
    let s = start_session(
        &mut ledger,
        dir.path(),
        &SessionParams {
            agent: "claude".into(),
            model: "m".into(),
            vendor: "anthropic".into(),
            home: Some(home.path().to_path_buf()),
        },
    )
    .unwrap();
    (dir, home, ledger, s.id)
}

#[test]
fn verify_runs_the_checkers_and_records_the_verdict_and_the_gate_result() {
    let policy = r#"{"version":1,"lanes":{"standard":{"checkers":["unfinished"],"desks":[]}}}"#;
    let (dir, home, mut ledger, sid) = project("run", policy);
    let v = verify(
        dir.path(),
        &mut ledger,
        &sid,
        Some(home.path()),
        &VerifyOptions {
            budget: Some(Duration::from_secs(60)),
            reason: Some("test".into()),
        },
    )
    .unwrap();
    assert!(v.computed.label().contains("VERIFIED") || v.computed.label().contains("PARTIAL"));
    let records = ledger.records_for_session(&sid).unwrap();
    assert!(records.iter().any(|s| matches!(
        &s.record,
        Record::Check { name, outcome: CheckOutcome::Ran, .. } if name == "unfinished"
    )));
    assert!(
        records
            .iter()
            .any(|s| matches!(&s.record, Record::Verdict { .. }))
    );
    assert!(records.iter().any(|s| matches!(
        &s.record,
        Record::Check { name, .. } if name == "gate"
    )));
}

#[test]
fn every_check_in_one_run_carries_the_snapshot_stamp_taken_before_the_run() {
    // `exec` creates a file during the run. Every check in that run must still carry the stamp of
    // the snapshot the context cached before it started (never a later tree), so evidence about
    // the earlier snapshot cannot look current for the later one.
    let policy = r#"{"version":1,
        "lanes":{"low":{"checkers":["exec","unfinished"],"desks":[]}},
        "exec":{"commands":[{"run":"echo x > created_by_exec.txt","tier":"ran"}]}}"#;
    let (dir, home, mut ledger, sid) = project("stamp", policy);
    // The human approves the command, as on the page.
    verdict_core::human::apply(
        dir.path(),
        &mut ledger,
        &sid,
        &verdict_core::human::Action::ApproveCommands,
        Some(home.path()),
    )
    .unwrap();
    let before = verdict_core::baseline::tree_state(dir.path()).expect("a folder state");
    verify(
        dir.path(),
        &mut ledger,
        &sid,
        Some(home.path()),
        &VerifyOptions {
            budget: Some(Duration::from_secs(120)),
            reason: Some("stamp test".into()),
        },
    )
    .unwrap();
    let after = verdict_core::baseline::tree_state(dir.path()).expect("a folder state");
    assert_ne!(before, after, "the exec command changed the tree");
    let stamps: Vec<(String, Option<String>)> = ledger
        .records_for_session(&sid)
        .unwrap()
        .into_iter()
        .filter_map(|s| match &s.record {
            Record::Check {
                name, tree_state, ..
            } if name == "exec" || name == "unfinished" => Some((name.clone(), tree_state.clone())),
            _ => None,
        })
        .collect();
    assert!(stamps.len() >= 2, "{stamps:?}");
    let first = stamps[0].1.clone();
    for (name, stamp) in &stamps {
        assert_eq!(
            *stamp, first,
            "{name} was stamped with a different snapshot"
        );
        assert_ne!(
            stamp.as_deref(),
            Some(after.as_str()),
            "{name} is stamped with the later tree"
        );
    }
}

#[test]
fn verify_records_a_lane_that_cannot_run_as_unavailable_not_clean() {
    let policy = r#"{"version":1,"lanes":{"standard":{"checkers":["unfinished"],"desks":[]}}}"#;
    let (dir, home, mut ledger, sid) = project("empty", policy);
    // A budget of 1ms leaves the checker no time: the run must say so.
    let v = verify(
        dir.path(),
        &mut ledger,
        &sid,
        Some(home.path()),
        &VerifyOptions {
            budget: Some(Duration::from_millis(1)),
            reason: Some("tiny budget".into()),
        },
    )
    .unwrap();
    assert_ne!(v.computed.word, verdict_core::VerdictWord::Verified);
    let checks = ledger.records_for_session(&sid).unwrap();
    assert!(checks.iter().any(|s| matches!(
        &s.record,
        Record::Check { name, outcome: CheckOutcome::FailedToRun, .. } if name == "unfinished"
    )));
    assert!(
        checks
            .iter()
            .any(|s| matches!(&s.record, Record::CheckUnavailable { .. }))
    );
    // The desk requirement is recorded too; a refused desk never reads as run.
    assert!(checks.iter().any(|s| matches!(
        &s.record,
        Record::Check { name, outcome: CheckOutcome::FailedToRun, .. } if name.starts_with("desk:") || name == "desks"
    )) || checks.iter().any(|s| matches!(&s.record, Record::CheckUnavailable { component, .. } if component == "desks")));
}

#[test]
fn a_desk_proposal_is_recorded_as_a_hypothesis_and_never_a_finding() {
    // The desk engine writes proposals as hypotheses (never verdict-moving). This checks the
    // record shape the engine reads; the fake provider is exercised in verdict-desks' tests.
    let (_dir, _home, mut ledger, sid) = project("proposal", r#"{"version":1}"#);
    let p = Policy::builtin().unwrap();
    assert!(p.value.get("mutation").is_some());
    ledger
        .append(
            Writer::Desk,
            Some(&sid),
            Record::Hypothesis(verdict_core::Finding {
                id: "desk:qa.proposal.0.x".into(),
                severity: verdict_core::Severity::Nit,
                title:
                    "PROPOSAL: mutation — the tax branch changed (the desk did not run any code)"
                        .into(),
                claim_seq: None,
                evidence: vec![],
                tier: Tier::Judgment,
                blast: Some(Lane::Standard),
                judgment: true,
                fix_hint: None,
            }),
        )
        .unwrap();
    let a = verdict_core::verdict::analyze(&ledger, &sid).unwrap();
    assert_eq!(a.hypotheses.len(), 1);
    assert!(a.findings.is_empty());
    assert!(
        a.hypotheses[0].1.title.contains("PROPOSAL: mutation"),
        "{}",
        a.hypotheses[0].1.title
    );
    let _ = DecisionKind::ModeSwitch;
}

/// F20: one connected deterministic path — a provider answer is processed by the real desk
/// engine into a proposal, the engine accepts it, and the proposed checker actually runs and
/// writes a new receipt after the proposal. No model call is involved.
#[test]
fn a_desk_proposal_is_dispatched_to_the_checker_and_creates_a_new_receipt() {
    // The answer cites record 2 (the prompt) so the desk finding is a real finding, and proposes
    // the mutation checker. The provider is `cmd /C type`/`cat`, so it speaks the protocol without
    // any model.
    let dir = Tmp::new("f20");
    let home = Tmp::new("f20-home");
    let answer = serde_json::json!({
        "findings": [{
            "severity": "minor",
            "title": "a cited concern",
            "evidence": [{"record": 2}],
            "fix": "look at it"
        }],
        "proposals": [{"checker": "mutation", "reason": "the tax branch changed"}]
    });
    let answer_path = dir.path().join("answer.json");
    std::fs::write(&answer_path, answer.to_string()).unwrap();
    std::fs::write(
        dir.path().join("mutation.json"),
        r#"{"survivors":[{"seed":7,"file":"src/a.rs","line":1,"description":"changed the constant"}],"killed":0,"total":1}"#,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "fn a() {}\n").unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "f20@example.invalid"]);
    git(&["config", "user.name", "f20"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "baseline"]);
    #[cfg(windows)]
    let (program, args) = (
        "cmd",
        vec![
            "/C".to_string(),
            "type".to_string(),
            answer_path.to_string_lossy().to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (program, args) = ("cat", vec![answer_path.to_string_lossy().to_string()]);
    let policy = serde_json::json!({
        "version": 1,
        "lanes": {"standard": {"checkers": ["unfinished"], "desks": ["engineer"]}},
        "mutation": {
            "require_when": ["test_sensitivity_uncertain"],
            "test_sensitivity_uncertain": true,
            "report": "mutation.json"
        },
        "desks": {"run_proposals": true},
        "vendors": {"providers": [{
            "name": "fake", "vendor": "openai",
            "command": program, "args": args, "model": "fake-1"
        }]}
    });
    std::fs::write(
        dir.path().join("verdict.json"),
        serde_json::to_string(&policy).unwrap(),
    )
    .unwrap();
    let mut ledger = Ledger::open(dir.path()).unwrap();
    let sid = start_session(
        &mut ledger,
        dir.path(),
        &SessionParams {
            agent: "claude".into(),
            model: "builder".into(),
            vendor: "anthropic".into(),
            home: Some(home.path().to_path_buf()),
        },
    )
    .unwrap()
    .id;
    ledger
        .append(
            Writer::Adapter,
            Some(&sid),
            Record::Prompt {
                ordinal: 1,
                text: "check the tax branch".into(),
                agent: "claude".into(),
                model: "builder".into(),
            },
        )
        .unwrap();

    std::fs::write(dir.path().join("src/a.rs"), "fn a() { let _ = 1; }\n").unwrap();
    verify(
        dir.path(),
        &mut ledger,
        &sid,
        Some(home.path()),
        &VerifyOptions {
            budget: Some(Duration::from_secs(120)),
            reason: Some("F20".into()),
        },
    )
    .unwrap();

    let records = ledger.records_for_session(&sid).unwrap();
    let proposal_seq = records
        .iter()
        .find_map(|s| match &s.record {
            Record::Hypothesis(f) if f.title.starts_with("PROPOSAL: mutation") => Some(s.seq),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no proposal hypothesis: {records:#?}"));
    let dispatch_seq = records
        .iter()
        .find_map(|s| match &s.record {
            Record::Check { name, .. } if name == "proposals" => Some(s.seq),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no proposals dispatch record: {records:#?}"));
    let mutation_checks: Vec<_> = records
        .iter()
        .filter_map(|s| match &s.record {
            Record::Check {
                name,
                outcome,
                tier,
                detail,
                ..
            } if name == "mutation" => Some((s.seq, *outcome, *tier, detail.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(mutation_checks.len(), 1, "{mutation_checks:#?}");
    let (mutation_seq, outcome, tier, detail) = &mutation_checks[0];
    assert!(
        proposal_seq < dispatch_seq,
        "proposal {proposal_seq}, dispatch {dispatch_seq}"
    );
    assert!(
        *mutation_seq > dispatch_seq,
        "the proposed checker must run after the dispatch (dispatch {dispatch_seq}, mutation {mutation_seq})"
    );
    assert_eq!(*outcome, CheckOutcome::Ran, "{detail:?}");
    assert_eq!(*tier, Tier::Asset);
    let detail = detail.clone().unwrap_or_default();
    assert!(detail.contains("survivor"), "{detail}");
    let survivor = records.iter().find_map(|s| match &s.record {
        Record::Finding(f) if f.id.starts_with("mutation:") => Some(f),
        _ => None,
    });
    let survivor = survivor.expect("the mutation survivor finding");
    assert!(survivor.title.contains("seed 7"), "{}", survivor.title);
    assert!(!survivor.evidence.is_empty());
}
