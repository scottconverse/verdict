//! `run_lane`: what it writes to the ledger, and the cases around it (unknown checkers, policy
//! skips, exec approval, a second run, baselines that are not git commits).

mod common;

use std::time::Duration;

use common::project::{Project, project};
use common::*;
use verdict_checkers::{Checker, RunOptions, expand_selection, lane_checkers, run_lane_with_home};
use verdict_core::{CheckOutcome, Lane, Ledger, Policy, Record, RecordKind, Tier, Writer};

fn opts(lane: Lane) -> RunOptions {
    RunOptions {
        lane,
        timeout: Duration::from_secs(60),
    }
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn records_of(l: &Ledger, session: &str, kind: RecordKind) -> Vec<verdict_core::Stored> {
    l.records_for_session(session)
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == kind)
        .collect()
}

#[test]
fn run_lane_writes_one_check_per_run_and_one_finding_per_problem() {
    let mut b = build("regressions", "red");
    let root = b.root().to_path_buf();
    let home = b.home.path().to_path_buf();
    let out = run_lane_with_home(
        &mut b.ledger,
        &root,
        &b.session,
        &names(&["regressions", "unfinished"]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    assert_eq!(out.len(), 2);
    let reg = &out[0];
    assert_eq!(reg.name, "regressions");
    assert_eq!(reg.outcome, CheckOutcome::Ran);
    assert!(reg.findings >= 6, "{reg:?}");

    let checks = records_of(&b.ledger, &b.session, RecordKind::Check);
    assert_eq!(checks.len(), 2);
    for c in &checks {
        // The ledger lets only the core write checks.
        assert_eq!(c.writer, Writer::Core);
    }
    let Record::Check {
        name,
        tier,
        inputs_hash,
        outcome,
        detail,
        ..
    } = &checks[0].record
    else {
        panic!()
    };
    assert_eq!(name, "regressions");
    assert_eq!(*tier, Tier::Static);
    assert_eq!(inputs_hash.len(), 64);
    assert_eq!(*outcome, CheckOutcome::Ran);
    assert!(detail.as_deref().unwrap().contains("finding(s)"));
    assert_eq!(checks[0].seq, reg.check_seq);

    let findings = records_of(&b.ledger, &b.session, RecordKind::Finding);
    assert!(findings.len() >= reg.findings);
    for f in &findings {
        assert_eq!(f.writer, Writer::Checker);
        let Record::Finding(f) = &f.record else {
            panic!()
        };
        assert!(!f.evidence.is_empty());
    }
    assert!(records_of(&b.ledger, &b.session, RecordKind::Hypothesis).is_empty());
    assert!(records_of(&b.ledger, &b.session, RecordKind::RefusedWrite).is_empty());
    assert!(b.ledger.verify().unwrap().is_ok());
}

#[test]
fn unknown_checkers_become_check_unavailable_not_errors() {
    let mut b = build("regressions", "green");
    let root = b.root().to_path_buf();
    let home = b.home.path().to_path_buf();
    let out = run_lane_with_home(
        &mut b.ledger,
        &root,
        &b.session,
        &names(&["mutation", "regressions"]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    let missing = out.iter().find(|s| s.name == "mutation").unwrap();
    assert_eq!(missing.outcome, CheckOutcome::FailedToRun);
    let un = records_of(&b.ledger, &b.session, RecordKind::CheckUnavailable);
    assert_eq!(un.len(), 1);
    let Record::CheckUnavailable { component, reason } = &un[0].record else {
        panic!()
    };
    assert_eq!(component, "mutation");
    assert!(reason.contains("no checker named mutation"));
    // And the failed check is not a clean one.
    let Record::Check { outcome, tier, .. } =
        &b.ledger.get(missing.check_seq).unwrap().unwrap().record
    else {
        panic!()
    };
    assert_eq!(*outcome, CheckOutcome::FailedToRun);
    assert_eq!(*tier, Tier::HonestRed);
    // The real checker still ran.
    assert_eq!(
        out.iter()
            .find(|s| s.name == "regressions")
            .unwrap()
            .outcome,
        CheckOutcome::Ran
    );
}

#[test]
fn checkers_run_in_the_fixed_order_whatever_order_they_are_given() {
    let mut b = build("claims_vs_code", "green");
    let root = b.root().to_path_buf();
    let home = b.home.path().to_path_buf();
    let out = run_lane_with_home(
        &mut b.ledger,
        &root,
        &b.session,
        &names(&[
            "claims_vs_code",
            "exec",
            "unfinished",
            "mystery",
            "regressions",
        ]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    let order: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        order,
        vec![
            "regressions",
            "unfinished",
            "exec",
            "mystery",
            "claims_vs_code"
        ]
    );
}

#[test]
fn a_checker_the_lane_does_not_list_is_skipped_by_policy() {
    // The built-in low lane lists regressions but not baseline_diff.
    let mut b = build("baseline_diff", "red");
    let root = b.root().to_path_buf();
    let home = b.home.path().to_path_buf();
    let out = run_lane_with_home(
        &mut b.ledger,
        &root,
        &b.session,
        &names(&["baseline_diff"]),
        &opts(Lane::Low),
        Some(&home),
    )
    .unwrap();
    assert_eq!(out[0].outcome, CheckOutcome::SkippedByPolicy);
    assert_eq!(out[0].findings, 0);
    assert!(records_of(&b.ledger, &b.session, RecordKind::Finding).is_empty());
}

#[test]
fn lane_lists_expand_star_and_minus_from_the_policy() {
    let policy = Policy::builtin().unwrap();
    assert_eq!(
        lane_checkers(&policy, Lane::Release),
        expand_selection(&names(&["*"]))
    );
    let high = lane_checkers(&policy, Lane::High);
    assert!(high.contains(&"regressions".to_string()));
    // high is ["*", "-firstrun", "-release"]: neither is installed, nothing else is removed.
    assert_eq!(high.len(), verdict_checkers::CHECKER_NAMES.len());
    // standard names wiring, which this build has.
    assert!(lane_checkers(&policy, Lane::Standard).contains(&"wiring".to_string()));
}

#[test]
fn exec_does_not_run_anything_until_the_human_has_decided() {
    let files = [
        (
            "verdict.json",
            r#"{"version":1,"exec":{"commands":[{"run":"echo ran> marker.txt","tier":"ran"}]}}"#,
        ),
        ("a.txt", "a\n"),
    ];
    let mut p = project(&files, true);
    let root = p.root().to_path_buf();
    let home = p.home.path().to_path_buf();
    let sess = p.session.clone();
    let out = run_lane_with_home(
        &mut p.ledger,
        &root,
        &sess,
        &names(&["exec"]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);
    assert!(
        !root.join("marker.txt").exists(),
        "exec ran without approval"
    );
    let un = records_of(&p.ledger, &p.session, RecordKind::CheckUnavailable);
    let Record::CheckUnavailable { reason, .. } = &un[0].record else {
        panic!()
    };
    assert!(reason.contains("not approved"), "{reason}");

    // A refusal is not an approval.
    p.ledger
        .append(
            Writer::Human,
            Some(&sess),
            Record::Decision {
                decision: verdict_core::DecisionKind::RunProjectCommands,
                choice: "deny".into(),
                reason: None,
                who: Writer::Human,
            },
        )
        .unwrap();
    let out = run_lane_with_home(
        &mut p.ledger,
        &root,
        &sess,
        &names(&["exec"]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);
    assert!(!root.join("marker.txt").exists());

    // After the human approves it runs, and the check carries the tier the command earns.
    p.approve();
    let out = run_lane_with_home(
        &mut p.ledger,
        &root,
        &sess,
        &names(&["exec"]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
    assert_eq!(out[0].findings, 0);
    assert!(
        root.join("marker.txt").exists(),
        "approved exec did not run"
    );
    let Record::Check { tier, .. } = &p.ledger.get(out[0].check_seq).unwrap().unwrap().record
    else {
        panic!()
    };
    assert_eq!(*tier, Tier::Ran);
}

#[test]
fn a_command_that_was_already_failing_is_reported_as_already_red() {
    let files = [
        (
            "verdict.json",
            r#"{"version":1,"exec":{"commands":[{"run":"exit 7","tier":"ran"}]}}"#,
        ),
        ("a.txt", "a\n"),
    ];
    let mut p = project(&files, true);
    p.approve();
    p.write("a.txt", "changed\n");
    let ctx = p.context(Lane::Standard, Duration::from_secs(60));
    let findings = verdict_checkers::exec::Exec.run(&ctx).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(rule_of(&findings[0].id), "already-red");
    assert_eq!(findings[0].severity, verdict_core::Severity::Major);
    assert!(
        findings[0].title.contains("code 7"),
        "{}",
        findings[0].title
    );
    // Two commands were recorded as evidence: the run and the baseline re-run.
    assert_eq!(findings[0].evidence.len(), 2);
}

#[test]
fn a_command_that_runs_too_long_is_killed_and_reported() {
    let sleeper = if cfg!(windows) {
        "ping -n 30 127.0.0.1 > nul"
    } else {
        "sleep 30"
    };
    let policy =
        format!(r#"{{"version":1,"exec":{{"commands":[{{"run":"{sleeper}","tier":"ran"}}]}}}}"#);
    let files = [("verdict.json", policy.as_str()), ("a.txt", "a\n")];
    let mut p = project(&files, true);
    p.approve();
    p.write("a.txt", "changed\n");
    let ctx = p.context(Lane::Standard, Duration::from_secs(1));
    let started = std::time::Instant::now();
    let findings = verdict_checkers::exec::Exec.run(&ctx).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the sleeper was not killed"
    );
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0].title.contains("timed out"),
        "{}",
        findings[0].title
    );
}

#[test]
fn a_command_that_needs_a_server_is_not_run_and_says_so() {
    let files = [
        (
            "verdict.json",
            r#"{"version":1,"exec":{"commands":[
                {"run":"echo up> marker.txt","tier":"ran"},
                {"run":"echo page> page.txt","tier":"browsed","needs":"server"}]}}"#,
        ),
        ("a.txt", "a\n"),
    ];
    let mut p = project(&files, true);
    p.approve();
    let ctx = p.context(Lane::Standard, Duration::from_secs(60));
    let findings = verdict_checkers::exec::Exec.run(&ctx).unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(rule_of(&findings[0].id), "needs-server");
    assert_eq!(findings[0].tier, Tier::HonestRed);
    assert!(!p.root().join("page.txt").exists());
    assert!(p.root().join("marker.txt").exists());
}

#[test]
fn a_second_run_in_one_session_keeps_ids_unique() {
    let mut b = build("regressions", "red");
    let root = b.root().to_path_buf();
    let home = b.home.path().to_path_buf();
    for _ in 0..2 {
        run_lane_with_home(
            &mut b.ledger,
            &root,
            &b.session,
            &names(&["regressions"]),
            &opts(Lane::Standard),
            Some(&home),
        )
        .unwrap();
    }
    let ids: Vec<String> = records_of(&b.ledger, &b.session, RecordKind::Finding)
        .into_iter()
        .map(|s| match s.record {
            Record::Finding(f) => f.id,
            _ => unreachable!(),
        })
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate finding ids: {ids:?}");
    assert!(ids.iter().any(|i| i.ends_with(".r2")));
}

#[test]
fn a_folder_baseline_says_it_could_not_diff_lines() {
    // No git here: the baseline is a content hash of the folder.
    let p = project(&[("src/a.js", "export const a = 1;\n")], false);
    assert!(
        p.ledger.records().unwrap().iter().any(
            |r| matches!(&r.record, Record::Session { baseline, .. } if !baseline.is_git_repo)
        ),
        "this test needs the temp folder to be outside any git repository"
    );
    let reg = verdict_checkers::real_registry();
    let run = |p: &Project, name: &str| {
        find(&reg, name).run(&p.context(Lane::Standard, Duration::from_secs(10)))
    };
    // Nothing changed yet: the folder hashes the same, so there is nothing to find.
    assert!(run(&p, "regressions").unwrap().is_empty());
    // A change: the whole file is scanned and the finding says so.
    p.write("src/a.js", "export const a = 1;\ndebugger;\n");
    let f = run(&p, "regressions").unwrap();
    assert_eq!(f.len(), 1);
    assert!(
        f[0].title.contains("line-level diff was not possible"),
        "{}",
        f[0].title
    );
    // Checkers that need to know what is new cannot answer from a hash.
    for name in ["baseline_diff", "tests_weakened"] {
        assert!(
            run(&p, name).is_err(),
            "{name} must be unavailable on a folder baseline"
        );
    }
}

#[test]
fn work_already_uncommitted_at_the_baseline_is_called_out() {
    let dir = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.js"), "1\n").unwrap();
    init_repo(dir.path());
    std::fs::write(dir.path().join("a.js"), "2\n").unwrap(); // dirty before the session
    let mut ledger = Ledger::open_in_memory().unwrap();
    let session = verdict_core::start_session(
        &mut ledger,
        dir.path(),
        &verdict_core::SessionParams {
            agent: "a".into(),
            model: "m".into(),
            vendor: "v".into(),
            home: Some(home.path().to_path_buf()),
        },
    )
    .unwrap()
    .id;
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let ctx = verdict_checkers::CheckContext::from_ledger(
        &ledger,
        dir.path(),
        &session,
        Lane::Standard,
        Duration::from_secs(5),
        policy,
    )
    .unwrap();
    let _ = verdict_checkers::regressions::Regressions
        .run(&ctx)
        .unwrap();
    assert!(
        ctx.notes()
            .iter()
            .any(|n| n.contains("already had 1 uncommitted")),
        "{:?}",
        ctx.notes()
    );
}

#[test]
fn a_claims_checker_with_no_claims_is_unavailable_not_clean() {
    let mut p = project(&[("src/a.rs", "pub fn a() {}\n")], true);
    let root = p.root().to_path_buf();
    let home = p.home.path().to_path_buf();
    let sess = p.session.clone();
    let out = run_lane_with_home(
        &mut p.ledger,
        &root,
        &sess,
        &names(&["claims_vs_code"]),
        &opts(Lane::Standard),
        Some(&home),
    )
    .unwrap();
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);
}

#[test]
fn rust_inline_tests_and_mod_imports_are_read_without_a_parser() {
    let mut p = project(
        &[
            (
                "src/lib.rs",
                "pub mod helper;\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert_eq!(1 + 1, 2);\n        assert_eq!(2 + 2, 4);\n    }\n}\n",
            ),
            ("src/helper.rs", "pub fn h() {}\n"),
        ],
        true,
    );
    // The helper is deleted (the import breaks) and an assertion is dropped from the inline tests.
    std::fs::remove_file(p.root().join("src/helper.rs")).unwrap();
    p.write(
        "src/lib.rs",
        "pub mod helper;\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        assert_eq!(1 + 1, 2);\n    }\n}\n",
    );
    p.prompt("tidy");
    let reg = verdict_checkers::real_registry();
    let ctx = p.context(Lane::Standard, Duration::from_secs(10));
    let r = find(&reg, "regressions").run(&ctx).unwrap();
    assert!(r.iter().any(|f| rule_of(&f.id) == "broken-import"), "{r:?}");
    let t = find(&reg, "tests_weakened").run(&ctx).unwrap();
    assert!(
        t.iter().any(|f| rule_of(&f.id) == "assertions-removed"),
        "{t:?}"
    );
}
