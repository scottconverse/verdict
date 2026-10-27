//! Golden reports: one stored ledger per verdict word, expected output in `snapshots/`, so a change
//! in wording is a deliberate diff, not drift.

mod p2b_common;

use p2b_common::*;
use verdict_core::verdict::{CommitAnywayChoice, record_verdict};
use verdict_core::*;

fn render(l: &Ledger, lane: Lane) -> String {
    let c = verdict_of(l, lane);
    report_with(l, &c)
}

fn fresh(label: &str) -> (Tmp, Ledger) {
    let t = Tmp::new(label);
    let l = seeded(t.path());
    (t, l)
}

#[test]
fn golden_unverified() {
    let (_t, l) = fresh("g-unverified");
    insta::assert_snapshot!(render(&l, Lane::Standard));
}

#[test]
fn golden_failed_with_a_commit_anyway_override() {
    let (_t, mut l) = fresh("g-failed");
    run_all_required(&mut l, Lane::Standard);
    find(
        &mut l,
        "wiring:src/login.rs:12",
        Severity::Major,
        Tier::Static,
    );
    human(
        &mut l,
        DecisionKind::CommitAnyway,
        &CommitAnywayChoice {
            tree: "tree1".into(),
            finding_ids: vec!["wiring:src/login.rs:12".into()],
        }
        .encode(),
        Some("hotfix, will fix Monday"),
    );
    insta::assert_snapshot!(render(&l, Lane::Standard));
}

#[test]
fn golden_needs_you() {
    let (_t, mut l) = fresh("g-needs");
    run_all_required(&mut l, Lane::Standard);
    find(
        &mut l,
        "baseline_diff:route-POST-/api/receipts",
        Severity::Major,
        Tier::Static,
    );
    desk_find(&mut l, "engineer:refund-total", Severity::Minor);
    insta::assert_snapshot!(render(&l, Lane::Standard));
}

#[test]
fn golden_verified_ran() {
    let (_t, mut l) = fresh("g-verified");
    run_all_required(&mut l, Lane::Standard);
    find(&mut l, "wiring:old", Severity::Major, Tier::Static);
    human(
        &mut l,
        DecisionKind::AcceptFindingNotReal,
        "wiring:old",
        None,
    );
    insta::assert_snapshot!(render(&l, Lane::Standard));
}

#[test]
fn golden_partial() {
    let (_t, mut l) = fresh("g-partial");
    for n in [
        "wiring",
        "baseline_diff",
        "regressions",
        "tests_weakened",
        "claims_vs_code",
    ] {
        ran(&mut l, n, Tier::Static);
    }
    unavailable(&mut l, "exec");
    insta::assert_snapshot!(render(&l, Lane::Standard));
}

#[test]
fn golden_clear_to_ship() {
    let (_t, mut l) = fresh("g-clear");
    run_all_required(&mut l, Lane::Release);
    insta::assert_snapshot!(render(&l, Lane::Release));
}

#[test]
fn the_stored_verdict_renders_the_same_headline_as_the_computed_one() {
    let (_t, mut l) = fresh("g-stored");
    run_all_required(&mut l, Lane::Standard);
    let c = verdict_of(&l, Lane::Standard);
    record_verdict(&mut l, &c).unwrap();
    let stored = render_report(&l, Some(SID)).unwrap();
    assert!(stored.starts_with("VERDICT: VERIFIED (ran)"), "{stored}");
    assert!(stored.contains("lane: standard"));
    assert!(!stored.contains("not in this build"));
    // The first three lines (verdict, three-state, session) agree.
    let live = report_with(&l, &c);
    let head = |s: &str| s.lines().take(2).collect::<Vec<_>>().join("\n");
    assert_eq!(head(&stored), head(&live));
}

#[test]
fn a_session_with_no_verdict_still_reads_unverified_in_the_stored_report() {
    let (_t, l) = fresh("g-none");
    let text = render_report(&l, Some(SID)).unwrap();
    assert!(text.starts_with("VERDICT: UNVERIFIED - nothing was checked"));
}
