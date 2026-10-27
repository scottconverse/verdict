//! The nine defects the second-lineage review found in the checkers, each as behavior: a test
//! that failed before the fix and passes after.

mod common;

use std::time::{Duration, Instant};

use common::project::{Project, project};
use common::*;
use verdict_checkers::{Checker, RunOptions, run_lane_with_home};
use verdict_core::{
    CheckOutcome, ClaimSource, DecisionKind, Lane, Ledger, Record, RecordKind, Tier, Writer,
};

fn opts() -> RunOptions {
    RunOptions {
        lane: Lane::Standard,
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

fn run(p: &mut Project, checkers: &[&str]) -> Vec<verdict_checkers::CheckSummary> {
    let root = p.root().to_path_buf();
    let home = p.home.path().to_path_buf();
    let session = p.session.clone();
    run_lane_with_home(
        &mut p.ledger,
        &root,
        &session,
        &names(checkers),
        &opts(),
        Some(&home),
    )
    .unwrap()
}

fn unavailable_reasons(p: &Project) -> Vec<String> {
    records_of(&p.ledger, &p.session, RecordKind::CheckUnavailable)
        .into_iter()
        .map(|r| match r.record {
            Record::CheckUnavailable { reason, .. } => reason,
            _ => unreachable!(),
        })
        .collect()
}

fn finding_titles(p: &Project) -> Vec<String> {
    records_of(&p.ledger, &p.session, RecordKind::Finding)
        .into_iter()
        .map(|r| match r.record {
            Record::Finding(f) => format!("{} | {}", f.id, f.title),
            _ => unreachable!(),
        })
        .collect()
}

// ---------------------------------------------------------------- 1. approval bound to commands

const ONE_COMMAND: &str =
    r#"{"version":1,"exec":{"commands":[{"run":"echo ran> marker.txt","tier":"ran"}]}}"#;

#[test]
fn an_approval_covers_only_the_commands_it_was_given_for() {
    let files = [("verdict.json", ONE_COMMAND), ("a.txt", "a\n")];
    let mut p = project(&files, true);
    p.approve();
    let out = run(&mut p, &["exec"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
    assert!(p.root().join("marker.txt").exists());
    std::fs::remove_file(p.root().join("marker.txt")).unwrap();

    // The agent edits the policy: a new command appears. The old approval must not cover it.
    p.write(
        "verdict.json",
        r#"{"version":1,"exec":{"commands":[
            {"run":"echo ran> marker.txt","tier":"ran"},
            {"run":"echo evil> evil.txt","tier":"ran"}]}}"#,
    );
    let out = run(&mut p, &["exec"]);
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);
    let reasons = unavailable_reasons(&p);
    assert!(
        reasons
            .last()
            .unwrap()
            .contains("not approved: the commands changed since you approved them"),
        "{reasons:?}"
    );
    assert!(
        !p.root().join("evil.txt").exists(),
        "an unapproved command ran"
    );
    assert!(
        !p.root().join("marker.txt").exists(),
        "nothing may run after the commands changed"
    );

    // Changing the timeout or a tier counts as a change too.
    p.write(
        "verdict.json",
        r#"{"version":1,"exec":{"commands":[{"run":"echo ran> marker.txt","tier":"browsed"}]}}"#,
    );
    let out = run(&mut p, &["exec"]);
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);

    // Putting the commands back as approved makes the old approval valid again.
    p.write("verdict.json", ONE_COMMAND);
    let out = run(&mut p, &["exec"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
}

#[test]
fn an_approval_with_no_hash_does_not_count() {
    let files = [("verdict.json", ONE_COMMAND), ("a.txt", "a\n")];
    let mut p = project(&files, true);
    let session = p.session.clone();
    // What older ledgers hold: a bare "approved" / "approve".
    approve_exec_choice(&mut p.ledger, &session, "approved");
    approve_exec_choice(&mut p.ledger, &session, "approve");
    let out = run(&mut p, &["exec"]);
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);
    assert!(!p.root().join("marker.txt").exists());
    let reasons = unavailable_reasons(&p);
    assert!(reasons[0].contains("not approved"), "{reasons:?}");
}

// ------------------------------------------------------- 2. incomplete search is never clean

fn big_source_file_defining(symbol: &str) -> String {
    // Over the 1 MiB reading limit, so its text is never read.
    let mut s = format!("pub fn {symbol}() {{}}\n");
    s.push_str(&"// padding line to make the file large\n".repeat(30_000));
    s
}

#[test]
fn a_symbol_search_that_skipped_a_file_is_unavailable_not_clean() {
    let big = big_source_file_defining("hidden_helper");
    let files = [
        ("src/big.rs", big.as_str()),
        ("src/a.rs", "pub fn a() {}\n"),
    ];
    let mut p = project(&files, true);
    // The agent says it removed hidden_helper; the only place that still defines it was skipped.
    p.claim(
        ClaimSource::FinalMessage,
        "Removed hidden_helper from the code.",
    );
    let out = run(&mut p, &["claims_vs_code"]);
    assert_eq!(
        out[0].outcome,
        CheckOutcome::FailedToRun,
        "a search that skipped a file must not read as a clean check"
    );
    let reasons = unavailable_reasons(&p);
    assert!(
        reasons[0].contains("src/big.rs") && reasons[0].contains("not read"),
        "{reasons:?}"
    );
}

#[test]
fn a_claimed_symbol_is_not_called_missing_when_a_file_was_skipped() {
    let big = big_source_file_defining("hidden_helper");
    let files = [
        ("src/big.rs", big.as_str()),
        ("src/a.rs", "pub fn a() {}\n"),
    ];
    let mut p = project(&files, true);
    p.claim(
        ClaimSource::FinalMessage,
        "Added hidden_helper to the code.",
    );
    let out = run(&mut p, &["claims_vs_code"]);
    assert_eq!(out[0].outcome, CheckOutcome::FailedToRun);
    assert!(
        finding_titles(&p).is_empty(),
        "absence is not established, so no contradiction may be claimed: {:?}",
        finding_titles(&p)
    );
}

#[test]
fn a_missing_name_in_a_tree_read_completely_still_gives_the_finding() {
    let files = [("src/a.rs", "pub fn a() {}\n")];
    let mut p = project(&files, true);
    p.claim(
        ClaimSource::FinalMessage,
        "Added the function charge_card_now to src/a.rs.",
    );
    let out = run(&mut p, &["claims_vs_code"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
    assert!(
        finding_titles(&p)
            .iter()
            .any(|t| t.contains("missing-symbol") && t.contains("charge_card_now")),
        "{:?}",
        finding_titles(&p)
    );
}

// ------------------------------------------------------------- 3. an unusable baseline

#[test]
fn a_baseline_rerun_without_the_installed_dependencies_is_unknown_not_already_red() {
    let files = [
        (
            "verdict.json",
            r#"{"version":1,"exec":{"commands":[{"run":"exit 7","tier":"ran"}]}}"#,
        ),
        (".gitignore", "node_modules/\n"),
        ("a.txt", "a\n"),
    ];
    let mut p = project(&files, true);
    // Installed dependencies: in the working tree, ignored by git, so absent from the baseline copy.
    p.write("node_modules/dep/index.js", "module.exports = 1;\n");
    p.approve();
    p.write("a.txt", "changed\n");
    let ctx = p.context(Lane::Standard, Duration::from_secs(60));
    let findings = verdict_checkers::exec::Exec.run(&ctx).unwrap();
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0].id.starts_with("exec:failed-baseline-unknown."),
        "{}",
        findings[0].id
    );
    assert_eq!(findings[0].severity, verdict_core::Severity::Blocker);
    assert!(
        findings[0].title.contains("baseline unknown")
            && findings[0].title.contains("node_modules"),
        "{}",
        findings[0].title
    );
}

#[test]
fn already_red_says_the_baseline_ran_under_the_same_conditions() {
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
    assert!(findings[0].id.starts_with("exec:already-red."));
    assert_eq!(findings[0].severity, verdict_core::Severity::Major);
    assert!(
        findings[0].title.contains("under the same conditions"),
        "{}",
        findings[0].title
    );
}

#[test]
fn a_baseline_rerun_that_times_out_is_unknown() {
    // Fails at once in the working tree; on the baseline copy the tracked marker is absent, so it
    // sleeps past the limit. (cmd and sh both take `||`.)
    let sleeper = if cfg!(windows) {
        "ping -n 30 127.0.0.1 > nul"
    } else {
        "sleep 30"
    };
    let cmd = format!("(test -f ok.txt || exit 9) && exit 3 || {sleeper}");
    // `test` is not a cmd builtin, so on Windows build the same shape with `if exist`.
    let cmd = if cfg!(windows) {
        format!("if exist ok.txt (exit 3) else ({sleeper})")
    } else {
        cmd
    };
    let policy = serde_json::json!({
        "version": 1,
        "exec": {"timeout": "2s", "commands": [{"run": cmd, "tier": "ran"}]}
    })
    .to_string();
    let files = [("verdict.json", policy.as_str()), ("a.txt", "a\n")];
    let mut p = project(&files, true);
    p.approve();
    // The marker is untracked: present in the working tree, missing from the baseline copy.
    p.write("ok.txt", "x\n");
    let ctx = p.context(Lane::Standard, Duration::from_secs(60));
    let findings = verdict_checkers::exec::Exec.run(&ctx).unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(
        findings[0].id.starts_with("exec:failed-baseline-unknown."),
        "{} {}",
        findings[0].id,
        findings[0].title
    );
    assert!(
        findings[0].title.contains("timed out"),
        "{}",
        findings[0].title
    );
}

// -------------------------------------------------------------------- 4. the process tree

#[cfg(windows)]
const LEAVE_BEHIND_HOLDING_PIPES: &str = "start /b ping -n 30 127.0.0.1";
#[cfg(not(windows))]
const LEAVE_BEHIND_HOLDING_PIPES: &str = "sleep 30 &";

#[cfg(windows)]
const LEAVE_BEHIND_THAT_WRITES_LATER: &str =
    r#"start /b cmd /c "ping -n 4 127.0.0.1 > nul & echo alive> survivor.txt""#;
#[cfg(not(windows))]
const LEAVE_BEHIND_THAT_WRITES_LATER: &str =
    "(sleep 3; echo alive > survivor.txt) > /dev/null 2>&1 &";

#[test]
fn a_command_that_leaves_a_process_running_is_stopped_and_never_a_success() {
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let r = verdict_checkers::exec::run_command(
        LEAVE_BEHIND_HOLDING_PIPES,
        dir.path(),
        Duration::from_secs(60),
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "waited for the stray process instead of stopping it"
    );
    assert_eq!(r.exit, Some(0), "{r:?}");
    assert!(r.left_behind, "the stray process was not noticed: {r:?}");
    assert!(
        !r.success(),
        "a run that left processes behind counted as success"
    );
    assert!(
        r.output_complete,
        "the pipes should close once the tree is stopped: {r:?}"
    );
}

#[test]
fn a_stray_process_is_really_killed() {
    let dir = tempfile::tempdir().unwrap();
    let r = verdict_checkers::exec::run_command(
        LEAVE_BEHIND_THAT_WRITES_LATER,
        dir.path(),
        Duration::from_secs(60),
    );
    assert!(!r.success(), "{r:?}");
    // The stray would write this file about three seconds after the command ended.
    std::thread::sleep(Duration::from_secs(6));
    assert!(
        !dir.path().join("survivor.txt").exists(),
        "the stray process outlived the command"
    );
}

#[test]
fn a_timed_out_command_takes_its_children_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = if cfg!(windows) {
        r#"cmd /c "ping -n 5 127.0.0.1 > nul & echo alive> survivor.txt""#
    } else {
        "sh -c 'sleep 3; echo alive > survivor.txt'"
    };
    let r = verdict_checkers::exec::run_command(cmd, dir.path(), Duration::from_secs(1));
    assert!(r.timed_out, "{r:?}");
    assert!(!r.success());
    std::thread::sleep(Duration::from_secs(7));
    assert!(
        !dir.path().join("survivor.txt").exists(),
        "a child of the timed-out command kept running"
    );
}

#[test]
fn the_same_output_always_gets_the_same_hash() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = if cfg!(windows) {
        "for /L %i in (1,1,300) do @echo line %i"
    } else {
        "i=0; while [ $i -lt 300 ]; do echo line $i; i=$((i+1)); done"
    };
    let a = verdict_checkers::exec::run_command(cmd, dir.path(), Duration::from_secs(60));
    let b = verdict_checkers::exec::run_command(cmd, dir.path(), Duration::from_secs(60));
    assert!(a.success() && b.success(), "{a:?} {b:?}");
    assert!(a.output_complete && b.output_complete);
    assert_eq!(a.output_hash, b.output_hash);
}

// --------------------------------------------------- 5. only the human's words authorize work

fn baseline_diff_titles(p: &mut Project, accept_claim: Option<u64>) -> Vec<String> {
    if let Some(seq) = accept_claim {
        p.ledger
            .append(
                Writer::Human,
                Some(&p.session),
                Record::Decision {
                    decision: DecisionKind::AcceptPromptTranslation,
                    choice: seq.to_string(),
                    reason: None,
                    who: Writer::Human,
                },
            )
            .unwrap();
    }
    let ctx = p.context(Lane::Standard, Duration::from_secs(60));
    verdict_checkers::baseline_diff::BaselineDiff
        .run(&ctx)
        .unwrap()
        .into_iter()
        .map(|f| f.title)
        .collect()
}

#[test]
fn the_agents_own_claim_does_not_authorize_unplanned_work() {
    let files = [("src/login.rs", "pub fn login() {}\n")];
    let mut p = project(&files, true);
    p.prompt("Fix the login bug in src/login.rs");
    p.write("src/login.rs", "pub fn login() { /* fixed */ }\n");
    p.write("src/admin_panel.rs", "pub fn admin() {}\n");
    let appended = p
        .ledger
        .append(
            Writer::Agent,
            Some(&p.session),
            Record::Claim {
                source: ClaimSource::FinalMessage,
                text: "Fixed login and also added src/admin_panel.rs".into(),
                names: vec!["src/admin_panel.rs".into()],
                prompt_ordinal: None,
            },
        )
        .unwrap();
    let titles = baseline_diff_titles(&mut p, None);
    assert!(
        titles.iter().any(|t| t.contains("src/admin_panel.rs")),
        "the agent's own mention suppressed the finding: {titles:?}"
    );
    // The human accepts that claim: now it stands for something asked for.
    let titles = baseline_diff_titles(&mut p, Some(appended.seq));
    assert!(
        !titles.iter().any(|t| t.contains("src/admin_panel.rs")),
        "{titles:?}"
    );
}

// ------------------------------------------------------------------- 6. an unreadable policy

#[test]
fn an_unreadable_policy_makes_every_checker_unavailable_and_runs_nothing() {
    let files = [("verdict.json", "{ this is not json"), ("a.txt", "a\n")];
    let mut p = project(&files, true);
    // An approval from before the policy broke.
    let session = p.session.clone();
    approve_exec_choice(&mut p.ledger, &session, "approved:0123456789abcdef");
    p.claim(ClaimSource::FinalMessage, "Updated a.txt");
    let all = names(&[
        "baseline_diff",
        "regressions",
        "tests_weakened",
        "unfinished",
        "exec",
        "claims_vs_code",
    ]);
    let root = p.root().to_path_buf();
    let home = p.home.path().to_path_buf();
    let out =
        run_lane_with_home(&mut p.ledger, &root, &session, &all, &opts(), Some(&home)).unwrap();
    assert_eq!(out.len(), 6);
    for s in &out {
        assert_eq!(s.outcome, CheckOutcome::FailedToRun, "{s:?}");
    }
    let reasons: Vec<String> = unavailable_reasons(&p)
        .into_iter()
        .filter(|r| !r.contains("the built-in default policy was used"))
        .collect();
    assert_eq!(reasons.len(), 6, "{reasons:?}");
    for r in &reasons {
        assert!(r.starts_with("policy could not be read: "), "{r}");
    }
    // No check was recorded as having run, and no finding was written.
    for c in records_of(&p.ledger, &p.session, RecordKind::Check) {
        let Record::Check { outcome, .. } = c.record else {
            panic!()
        };
        assert_eq!(outcome, CheckOutcome::FailedToRun);
    }
    assert!(finding_titles(&p).is_empty());
}

// ------------------------------------------- 7. each removal verb belongs to its own target

#[test]
fn a_removal_verb_only_excuses_the_file_it_is_aimed_at() {
    let files = [("src/a.rs", "pub fn a() {}\n")];
    let mut p = project(&files, true);
    p.claim(
        ClaimSource::FinalMessage,
        "Removed src/old.rs and added src/new.rs",
    );
    let out = run(&mut p, &["claims_vs_code"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
    let titles = finding_titles(&p);
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].contains("missing-file") && titles[0].contains("src/new.rs"));
}

#[test]
fn a_removal_verb_only_excuses_the_symbol_it_is_aimed_at() {
    let files = [("src/a.rs", "pub fn a() {}\n")];
    let mut p = project(&files, true);
    p.claim(
        ClaimSource::FinalMessage,
        "Removed old_helper and added new_helper",
    );
    let out = run(&mut p, &["claims_vs_code"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
    let titles = finding_titles(&p);
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].contains("missing-symbol") && titles[0].contains("new_helper"));
}

// -------------------------------------------------- 8. a deleted inline Rust test module

#[test]
fn deleting_a_whole_inline_test_module_is_seen() {
    let before = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adds() {\n        assert_eq!(add(1, 2), 3);\n    }\n}\n";
    let files = [("src/lib.rs", before)];
    let mut p = project(&files, true);
    p.write(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    );
    let out = run(&mut p, &["tests_weakened"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);
    assert!(
        finding_titles(&p)
            .iter()
            .any(|t| t.starts_with("tests_weakened:") && t.contains("src/lib.rs")),
        "{:?}",
        finding_titles(&p)
    );
}

// ------------------------------------------------------------ 9. a receipt for every command

#[test]
fn every_command_that_ran_keeps_its_own_receipt() {
    let policy = r#"{"version":1,"exec":{"commands":[
        {"run":"echo first-output","tier":"ran"},
        {"run":"echo second-output","tier":"ran"},
        {"run":"exit 5","tier":"ran"}]}}"#;
    let files = [("verdict.json", policy), ("a.txt", "a\n")];
    let mut p = project(&files, true);
    p.approve();
    p.write("a.txt", "changed\n");
    let out = run(&mut p, &["exec"]);
    assert_eq!(out[0].outcome, CheckOutcome::Ran);

    let details: Vec<String> = records_of(&p.ledger, &p.session, RecordKind::Check)
        .into_iter()
        .filter_map(|r| match r.record {
            Record::Check {
                name,
                outcome: CheckOutcome::Ran,
                tier,
                detail: Some(d),
                ..
            } if name == "exec" && d.starts_with("receipt:") => {
                assert_eq!(tier, Tier::Ran);
                Some(d)
            }
            _ => None,
        })
        .collect();
    // Three commands, plus the baseline re-run of the failing one.
    assert_eq!(details.len(), 4, "{details:#?}");
    for (cmd, how) in [
        ("echo first-output", "exited with code 0"),
        ("echo second-output", "exited with code 0"),
        ("exit 5", "exited with code 5"),
    ] {
        let d = details
            .iter()
            .find(|d| d.contains(&format!("`{cmd}`")))
            .unwrap_or_else(|| panic!("no receipt for {cmd}: {details:#?}"));
        assert!(d.contains(how), "{d}");
        assert!(d.contains("output hash "), "{d}");
    }
    assert!(
        details
            .iter()
            .any(|d| d.contains("first-output") && d.contains("output tail")),
        "the bounded tail is kept"
    );
    // Different commands, different output hashes.
    let hashes: std::collections::BTreeSet<&str> = details
        .iter()
        .filter(|d| d.contains("`echo first-output`") || d.contains("`echo second-output`"))
        .filter_map(|d| d.split("output hash ").nth(1)?.split(';').next())
        .collect();
    assert_eq!(hashes.len(), 2, "{details:#?}");
    // The checker's own record is still the last one about exec.
    let last = records_of(&p.ledger, &p.session, RecordKind::Check)
        .into_iter()
        .next_back()
        .unwrap();
    assert_eq!(last.seq, out[0].check_seq);
}
