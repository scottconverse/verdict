mod p2b_common;

use p2b_common::*;
use verdict_core::lane::Bar;
use verdict_core::verdict::{CommitAnywayChoice, FindingStatus, analyze, compute, record_verdict};
use verdict_core::*;

fn l() -> (Tmp, Ledger) {
    let t = Tmp::new("word");
    let l = seeded(t.path());
    (t, l)
}

fn word(l: &Ledger, lane: Lane) -> (VerdictWord, Option<Tier>) {
    let c = verdict_of(l, lane);
    (c.word, c.highest_tier)
}

// ------------------------------------------------------------------ the six words

#[test]
fn nothing_checked_is_unverified() {
    let (_t, l) = l();
    assert_eq!(word(&l, Lane::Low).0, VerdictWord::Unverified);
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::Unverified);
}

#[test]
fn a_blocking_finding_is_failed_even_when_other_checkers_are_clean() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    find(
        &mut l,
        "wiring:src/login.rs:3",
        Severity::Major,
        Tier::Static,
    );
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(c.word, VerdictWord::Failed);
    assert_eq!(c.fix_first.len(), 1);
    assert_eq!(c.label(), "FAILED");
}

#[test]
fn clean_runs_are_verified_with_the_highest_tier_as_a_mandatory_suffix() {
    let (_t, mut l) = l();
    for n in ["wiring", "regressions", "claims_vs_code", "unfinished"] {
        ran(&mut l, n, Tier::Static);
    }
    let c = verdict_of(&l, Lane::Low);
    assert_eq!(
        (c.word, c.highest_tier),
        (VerdictWord::Verified, Some(Tier::Static))
    );
    assert_eq!(c.label(), "VERIFIED (static)");

    // Add a build-and-test run in the standard lane: the suffix rises to ran.
    let (_t2, mut l2) = self::l();
    run_all_required(&mut l2, Lane::Standard);
    let c = verdict_of(&l2, Lane::Standard);
    assert_eq!(c.label(), "VERIFIED (ran)");

    // And a browsed check lifts it to browsed.
    ran(&mut l2, "wiring", Tier::Browsed);
    assert_eq!(
        verdict_of(&l2, Lane::Standard).label(),
        "VERIFIED (browsed)"
    );
}

#[test]
fn a_static_only_run_can_never_read_as_a_tested_one() {
    let (_t, mut l) = l();
    for n in ["wiring", "regressions", "claims_vs_code", "unfinished"] {
        ran(&mut l, n, Tier::Static);
    }
    let text = report_with(&l, &verdict_of(&l, Lane::Low));
    assert!(text.starts_with("VERDICT: VERIFIED (static)"), "{text}");
    assert!(text.contains("your build and tests: not run"));
    assert!(text.contains("your real app: not run"));
    assert!(!text.contains("VERIFIED (ran)") && !text.contains("VERIFIED (browsed)"));
}

#[test]
fn something_for_the_human_is_needs_you_not_verified() {
    // A desk finding (judgment) above nit.
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    desk_find(&mut l, "engineer:refund-total", Severity::Major);
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(c.word, VerdictWord::NeedsYou);
    assert!(c.fix_first.is_empty());
    assert_eq!(c.yours_to_settle.len(), 1);

    // A baseline_diff finding never blocks, even at major; it forces NEEDS YOU.
    let (_t, mut l) = self::l();
    run_all_required(&mut l, Lane::Standard);
    find(
        &mut l,
        "baseline_diff:route-POST-/api/receipts",
        Severity::Major,
        Tier::Static,
    );
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::NeedsYou);

    // tests_weakened too.
    let (_t, mut l) = self::l();
    run_all_required(&mut l, Lane::Standard);
    find(
        &mut l,
        "tests_weakened:login.test.ts",
        Severity::Critical,
        Tier::Static,
    );
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::NeedsYou);

    // A minor finding from a blocking checker is the human's to settle, a nit is not.
    let (_t, mut l) = self::l();
    run_all_required(&mut l, Lane::Standard);
    find(&mut l, "unfinished:todo", Severity::Minor, Tier::Static);
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::NeedsYou);
    let (_t, mut l) = self::l();
    run_all_required(&mut l, Lane::Standard);
    find(&mut l, "unfinished:todo", Severity::Nit, Tier::Static);
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::Verified);
}

#[test]
fn a_required_checker_that_did_not_run_is_partial() {
    let (_t, mut l) = l();
    for n in [
        "wiring",
        "baseline_diff",
        "regressions",
        "tests_weakened",
        "claims_vs_code",
        "unfinished",
    ] {
        ran(&mut l, n, Tier::Static);
    }
    // exec is required in the standard lane and never ran.
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(c.word, VerdictWord::Partial);
    assert!(
        c.not_established
            .iter()
            .any(|s| s.starts_with("exec: did not run"))
    );
    // The same ledger is fine for the low lane, which does not require exec.
    let low = verdict_of(&l, Lane::Low);
    assert_eq!(low.word, VerdictWord::Verified);
}

#[test]
fn check_unavailable_for_a_required_checker_is_never_clean() {
    // Alone: nothing else ran, but the required checker said it could not run.
    let (_t, mut l) = l();
    unavailable(&mut l, "wiring");
    let c = verdict_of(&l, Lane::Low);
    assert!(
        matches!(c.word, VerdictWord::Partial | VerdictWord::NeedsYou),
        "{:?}",
        c.word
    );
    // With the rest clean it is PARTIAL, and says why.
    for n in ["regressions", "claims_vs_code", "unfinished"] {
        ran(&mut l, n, Tier::Static);
    }
    let c = verdict_of(&l, Lane::Low);
    assert_eq!(c.word, VerdictWord::Partial);
    assert!(
        c.not_established
            .iter()
            .any(|s| s.contains("wiring: could not run"))
    );
    // A later successful run of that checker clears it.
    ran(&mut l, "wiring", Tier::Static);
    assert_eq!(verdict_of(&l, Lane::Low).word, VerdictWord::Verified);

    // A check that failed to start is the same thing as unavailable.
    let (_t, mut l) = self::l();
    for n in ["regressions", "claims_vs_code", "unfinished"] {
        ran(&mut l, n, Tier::Static);
    }
    check(&mut l, "wiring", Tier::HonestRed, CheckOutcome::FailedToRun);
    assert_eq!(word(&l, Lane::Low).0, VerdictWord::Partial);
}

#[test]
fn unavailable_plumbing_makes_a_clean_run_needs_you() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    l.append(
        Writer::Adapter,
        Some(SID),
        Record::CheckUnavailable {
            component: "core".into(),
            reason: "timed out after 20s on Stop".into(),
        },
    )
    .unwrap();
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::NeedsYou);
}

#[test]
fn skipped_by_policy_satisfies_a_requirement_but_is_listed_as_not_established() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::High);
    check(
        &mut l,
        "mutation",
        Tier::Unverified,
        CheckOutcome::SkippedByPolicy,
    );
    let c = verdict_of(&l, Lane::High);
    assert_eq!(c.word, VerdictWord::Verified);
    assert!(
        c.not_established
            .iter()
            .any(|s| s.contains("mutation: skipped by policy"))
    );
}

// ------------------------------------------------------------------ hypotheses

#[test]
fn hypotheses_never_count_toward_or_against_a_verdict() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    // A desk finding with no evidence pointer is stored as a hypothesis.
    let a = l
        .append(
            Writer::Desk,
            Some(SID),
            Record::Finding(finding_record(
                "engineer:x",
                Severity::Blocker,
                Tier::Judgment,
                true,
                false,
            )),
        )
        .unwrap();
    assert!(a.stored_as_hypothesis);
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(
        c.word,
        VerdictWord::Verified,
        "a hypothesis must not fail or block"
    );
    assert!(c.not_established.iter().any(|s| s.contains("hypothesis")));
    // And a hypothesis cannot make an unchecked session look checked.
    let (_t, mut l) = self::l();
    l.append(
        Writer::Desk,
        Some(SID),
        Record::Finding(finding_record(
            "engineer:x",
            Severity::Blocker,
            Tier::Judgment,
            true,
            false,
        )),
    )
    .unwrap();
    assert_eq!(word(&l, Lane::Low).0, VerdictWord::Unverified);
}

// ------------------------------------------------------------------ closing findings

#[test]
fn accepting_a_finding_as_not_real_closes_exactly_that_one() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    find(&mut l, "wiring:a", Severity::Major, Tier::Static);
    find(&mut l, "regressions:b", Severity::Critical, Tier::Static);
    assert_eq!(verdict_of(&l, Lane::Standard).fix_first.len(), 2);

    human(&mut l, DecisionKind::AcceptFindingNotReal, "wiring:a", None);
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(
        c.word,
        VerdictWord::Failed,
        "the other finding is still open"
    );
    assert_eq!(c.fix_first.len(), 1);
    assert_eq!(c.fix_first[0].id, "regressions:b");

    human(
        &mut l,
        DecisionKind::AcceptFindingNotReal,
        "regressions:b",
        None,
    );
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);

    // The closed ones are still in the report, with who closed them.
    let text = report_with(&l, &verdict_of(&l, Lane::Standard));
    assert!(text.contains("CLOSED FINDINGS"), "{text}");
    assert!(text.contains("accepted as not real by you"), "{text}");
}

#[test]
fn an_acceptance_of_something_else_closes_nothing() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    find(&mut l, "wiring:a", Severity::Major, Tier::Static);
    human(
        &mut l,
        DecisionKind::AcceptFindingNotReal,
        "wiring:other",
        None,
    );
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::Failed);
}

#[test]
fn only_the_checker_or_the_core_can_resolve_a_finding_never_a_desk_or_agent() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    let seq = find(&mut l, "wiring:a", Severity::Major, Tier::Static);
    // A desk and an agent cannot withdraw a finding: the ledger refuses a correction of a record
    // they did not write, and even a correction of their own is not a resolution.
    for w in [Writer::Desk, Writer::Agent] {
        let r = l.append(
            w,
            Some(SID),
            Record::Correction {
                target_seq: seq,
                reason: "not a problem".into(),
                replacement: None,
            },
        );
        assert!(r.is_err());
    }
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::Failed);
    // The core can: a re-run found it gone.
    l.append(
        Writer::Core,
        Some(SID),
        Record::Correction {
            target_seq: seq,
            reason: "wiring re-ran and no longer reports it".into(),
            replacement: None,
        },
    )
    .unwrap();
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::Verified);
    let a = analyze(&l, SID).unwrap();
    assert!(matches!(
        a.findings[0].status,
        FindingStatus::Resolved { .. }
    ));
}

// ------------------------------------------------------------------ CLEAR TO SHIP

fn release_ledger() -> (Tmp, Ledger) {
    let (t, mut l) = l();
    run_all_required(&mut l, Lane::Release);
    (t, l)
}

#[test]
fn clear_to_ship_needs_the_release_lane_and_every_condition() {
    let (_t, l) = release_ledger();
    let c = verdict_of(&l, Lane::Release);
    assert_eq!(c.word, VerdictWord::ClearToShip, "{:#?}", c.not_established);
    // The same ledger in the high lane (which does not require firstrun or release) is only
    // VERIFIED: CLEAR TO SHIP belongs to the release lane alone.
    let high = verdict_of(&l, Lane::High);
    assert_eq!(high.word, VerdictWord::Verified);
}

#[test]
fn clear_to_ship_withheld_without_a_browsed_firstrun_or_release_coherence() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Release);
    // firstrun re-ran, but only at the static tier: not an attestation.
    ran(&mut l, "firstrun", Tier::Static);
    let c = verdict_of(&l, Lane::Release);
    assert_eq!(c.word, VerdictWord::Verified);
    assert!(
        c.not_established
            .iter()
            .any(|s| s.contains("CLEAR TO SHIP withheld: firstrun"))
    );

    let (_t, mut l) = self::l();
    for name in input(Lane::Release).lane_checkers[&Lane::Release].clone() {
        if name == "release" {
            continue;
        }
        let tier = if name == "firstrun" {
            Tier::Browsed
        } else {
            Tier::Static
        };
        ran(&mut l, &name, tier);
    }
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::Partial);
}

#[test]
fn any_open_finding_above_nit_blocks_clear_to_ship_and_above_minor_fails_the_release() {
    let (_t, mut l) = release_ledger();
    find(&mut l, "release:note", Severity::Nit, Tier::Static);
    assert_eq!(
        word(&l, Lane::Release).0,
        VerdictWord::ClearToShip,
        "a nit does not block"
    );

    let (_t, mut l) = release_ledger();
    find(&mut l, "unfinished:todo", Severity::Minor, Tier::Static);
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::NeedsYou);

    // In the release lane a desk's major finding fails it (in other lanes it only needs you).
    let (_t, mut l) = release_ledger();
    desk_find(&mut l, "security:token", Severity::Major);
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::Failed);
    let (_t, mut l) = l_standard();
    desk_find(&mut l, "security:token", Severity::Major);
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::NeedsYou);
}

fn l_standard() -> (Tmp, Ledger) {
    let (t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    (t, l)
}

// ------------------------------------------------------------------ commit anyway

fn anyway(l: &mut Ledger, tree: &str, ids: &[&str], reason: &str) -> u64 {
    human(
        l,
        DecisionKind::CommitAnyway,
        &CommitAnywayChoice {
            tree: tree.into(),
            finding_ids: ids.iter().map(|s| s.to_string()).collect(),
        }
        .encode(),
        Some(reason),
    )
}

#[test]
fn commit_anyway_leaves_the_verdict_failed() {
    let (_t, mut l) = l_standard();
    find(&mut l, "wiring:a", Severity::Major, Tier::Static);
    assert_eq!(word(&l, Lane::Standard).0, VerdictWord::Failed);
    anyway(
        &mut l,
        "tree1",
        &["wiring:a"],
        "shipping a hotfix, will fix Monday",
    );
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(
        c.word,
        VerdictWord::Failed,
        "an override never changes a verdict"
    );
    assert!(c.override_open);
    let text = report_with(&l, &c);
    assert!(text.starts_with("VERDICT: FAILED"));
    assert!(text.contains("COMMIT ANYWAY"), "{text}");
    assert!(text.contains("shipping a hotfix, will fix Monday"));
}

#[test]
fn an_empty_reason_is_refused_and_a_non_human_cannot_commit_anyway() {
    let (_t, mut l) = l_standard();
    for reason in [None, Some(""), Some("   ")] {
        let r = l.append(
            Writer::Human,
            Some(SID),
            Record::Decision {
                decision: DecisionKind::CommitAnyway,
                choice: "x".into(),
                reason: reason.map(str::to_string),
                who: Writer::Human,
            },
        );
        assert!(matches!(r, Err(LedgerError::Invalid(_))), "{reason:?}");
    }
    for w in [
        Writer::Agent,
        Writer::Adapter,
        Writer::Checker,
        Writer::Desk,
        Writer::Core,
    ] {
        let r = l.append(
            w,
            Some(SID),
            Record::Decision {
                decision: DecisionKind::CommitAnyway,
                choice: "x".into(),
                reason: Some("because".into()),
                who: Writer::Human,
            },
        );
        match r {
            Err(LedgerError::Refused { refused_seq, .. }) => {
                let rec = l.get(refused_seq).unwrap().unwrap();
                assert!(matches!(
                    rec.record,
                    Record::RefusedWrite {
                        attempted_decision: Some(DecisionKind::CommitAnyway),
                        ..
                    }
                ));
            }
            other => panic!("{w}: expected a refusal, got {other:?}"),
        }
    }
    // None of that opened an override.
    assert!(!verdict_of(&l, Lane::Standard).override_open);
}

#[test]
fn an_open_override_blocks_clear_to_ship_until_the_finding_is_resolved_or_accepted() {
    // A nit does not fail or block on its own, so any block here is the override's doing.
    let (_t, mut l) = release_ledger();
    find(&mut l, "release:note", Severity::Nit, Tier::Static);
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::ClearToShip);
    let ov = anyway(&mut l, "tree1", &["release:note"], "need it out today");
    let c = verdict_of(&l, Lane::Release);
    assert_ne!(
        c.word,
        VerdictWord::ClearToShip,
        "an open override blocks CLEAR TO SHIP"
    );
    assert!(c.override_open);
    assert!(
        c.yours_to_settle
            .iter()
            .any(|s| s.contains(&format!("record {ov}")))
    );

    // Accepting the finding as not real closes the override.
    human(
        &mut l,
        DecisionKind::AcceptFindingNotReal,
        "release:note",
        None,
    );
    let c = verdict_of(&l, Lane::Release);
    assert_eq!(c.word, VerdictWord::ClearToShip);
    assert!(!c.override_open);

    // Resolving the finding closes it too.
    let (_t, mut l) = release_ledger();
    let seq = find(&mut l, "release:note", Severity::Nit, Tier::Static);
    anyway(&mut l, "tree1", &["release:note"], "need it out today");
    assert_ne!(word(&l, Lane::Release).0, VerdictWord::ClearToShip);
    l.append(
        Writer::Core,
        Some(SID),
        Record::Correction {
            target_seq: seq,
            reason: "release re-ran clean".into(),
            replacement: None,
        },
    )
    .unwrap();
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::ClearToShip);
}

#[test]
fn a_failed_release_with_an_override_is_failed_and_never_clear() {
    let (_t, mut l) = release_ledger();
    find(&mut l, "regressions:x", Severity::Major, Tier::Static);
    anyway(&mut l, "tree1", &["regressions:x"], "emergency");
    assert_eq!(word(&l, Lane::Release).0, VerdictWord::Failed);
}

// ------------------------------------------------------------------ the record

#[test]
fn the_core_writes_the_verdict_record_with_counts_not_established_and_policy_version() {
    let (_t, mut l) = l_standard();
    find(&mut l, "wiring:a", Severity::Major, Tier::Static);
    let c = verdict_of(&l, Lane::Standard);
    let seq = record_verdict(&mut l, &c).unwrap();
    let stored = l.get(seq).unwrap().unwrap();
    assert_eq!(stored.writer, Writer::Core);
    match stored.record {
        Record::Verdict {
            word,
            highest_tier,
            counts_by_tier,
            not_established,
            policy_version,
            lane,
        } => {
            assert_eq!(word, VerdictWord::Failed);
            assert_eq!(highest_tier, Some(Tier::Ran));
            assert_eq!(policy_version, 1);
            assert_eq!(lane, Some(Lane::Standard));
            assert!(counts_by_tier.get(&Tier::Ran).copied().unwrap_or(0) >= 1);
            assert!(counts_by_tier.get(&Tier::Static).copied().unwrap_or(0) >= 1);
            assert!(!not_established.is_empty());
        }
        other => panic!("{other:?}"),
    }
    // An agent cannot write one.
    let r = l.append(
        Writer::Agent,
        Some(SID),
        Record::Verdict {
            word: VerdictWord::ClearToShip,
            highest_tier: Some(Tier::Browsed),
            counts_by_tier: Default::default(),
            not_established: vec![],
            policy_version: 1,
            lane: Some(Lane::Release),
        },
    );
    assert!(matches!(r, Err(LedgerError::Refused { .. })));
    assert!(l.verify().unwrap().is_ok());
}

#[test]
fn a_broken_ledger_is_never_clean() {
    let t = Tmp::new("broken");
    let mut l = seeded(t.path());
    run_all_required(&mut l, Lane::Low);
    assert_eq!(word(&l, Lane::Low).0, VerdictWord::Verified);
    let path = l.path().unwrap().to_path_buf();
    drop(l);
    // Tamper: drop the no-update trigger and rewrite a record.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TRIGGER records_no_update; UPDATE records SET body = replace(body, 'add a login page', 'x') WHERE kind = 'prompt';",
    )
    .unwrap();
    drop(conn);
    let l = Ledger::open_existing(t.path()).unwrap().unwrap();
    let c = verdict_of(&l, Lane::Low);
    assert_eq!(c.word, VerdictWord::NeedsYou);
    assert!(
        c.yours_to_settle
            .iter()
            .any(|s| s.contains("ledger integrity"))
    );
}

// ---------------------------------------------------- lane desks, release tiers and bars

#[test]
fn a_lane_that_requires_desks_is_partial_until_they_have_receipts() {
    let (_t, mut l) = l();
    for name in input(Lane::Standard).lane_checkers[&Lane::Standard].clone() {
        let tier = match name.as_str() {
            "exec" | "mutation" => Tier::Ran,
            _ => Tier::Static,
        };
        ran(&mut l, &name, tier);
    }
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(c.word, VerdictWord::Partial, "{:?}", c.not_established);
    assert!(
        c.not_established
            .iter()
            .any(|s| s.contains("desk:engineer")),
        "{:?}",
        c.not_established
    );
    // The same ledger is clean once both required desks have a receipt on this code.
    ran_desks(&mut l, Lane::Standard);
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);
}

#[test]
fn a_web_first_run_must_be_browsed_but_a_declared_non_web_one_needs_only_ran() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Release);
    assert_eq!(verdict_of(&l, Lane::Release).word, VerdictWord::ClearToShip);
    // A ran firstrun is not a browsed attestation while the project's first run is a web walk.
    ran(&mut l, "firstrun", Tier::Ran);
    let c = verdict_of(&l, Lane::Release);
    assert_ne!(c.word, VerdictWord::ClearToShip);
    assert!(
        c.not_established
            .iter()
            .any(|s| s.contains("firstrun") && s.contains("browsed")),
        "{:?}",
        c.not_established
    );
    // Declared as a non-web first run (a library or CLI), the same ran evidence is enough.
    let mut i = input(Lane::Release);
    i.firstrun_web = false;
    let c = compute(&l, SID, &i).unwrap();
    assert_eq!(c.word, VerdictWord::ClearToShip, "{:?}", c.not_established);
}

#[test]
fn a_release_bar_of_blockers_only_does_not_fail_on_a_major() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Release);
    find(
        &mut l,
        "wiring:src/login.rs:4",
        Severity::Major,
        Tier::Static,
    );
    let mut i = input(Lane::Release);
    i.current_tree = Some(TEST_TREE.into());
    i.lane_bars.insert(Lane::Release, Bar::BlockersOnly);
    let c = compute(&l, SID, &i).unwrap();
    assert_eq!(c.word, VerdictWord::NeedsYou, "{:?}", c.not_established);
    // The default (zero_above_nit) fails the same release.
    assert_eq!(verdict_of(&l, Lane::Release).word, VerdictWord::Failed);
}

#[test]
fn an_evidence_pointer_that_no_longer_matches_is_reported_as_stale() {
    let t = Tmp::new("stale");
    let mut l = seeded(t.path());
    run_all_required(&mut l, Lane::Standard);
    let path = t.path().join("src").join("login.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let pointer = EvidencePointer::file_line(t.path(), "src/login.rs", 2).unwrap();
    l.append(
        Writer::Checker,
        Some(SID),
        Record::Finding(Finding {
            id: "wiring:src/login.rs:2".into(),
            severity: Severity::Minor,
            title: "a line pointer".into(),
            claim_seq: None,
            evidence: vec![pointer],
            tier: Tier::Static,
            blast: None,
            judgment: false,
            fix_hint: None,
        }),
    )
    .unwrap();
    std::fs::write(&path, "one\nCHANGED\n").unwrap();
    let mut i = input(Lane::Standard);
    i.stale_pointers = live_stale(&t, &l);
    let c = compute(&l, SID, &i).unwrap();
    assert!(
        c.not_established
            .iter()
            .any(|s| s.contains("stale evidence")),
        "{:?}",
        c.not_established
    );
}

/// The stale-pointer lines `live_input` would compute, without needing a git baseline.
fn live_stale(t: &Tmp, l: &Ledger) -> Vec<String> {
    let live = verdict_core::verdict::live_input(t.path(), l, SID, None).unwrap();
    live.input.stale_pointers
}

#[test]
fn desks_that_disagree_about_one_claim_are_both_kept_and_shown_as_dissent() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    for (id, severity, title) in [
        (
            "desk:engineer.tax",
            Severity::Major,
            "the refund double-counts tax",
        ),
        ("desk:qa.fine", Severity::Minor, "the refund path is fine"),
    ] {
        l.append(
            Writer::Desk,
            Some(SID),
            Record::Finding(Finding {
                id: id.into(),
                severity,
                title: title.into(),
                claim_seq: Some(3),
                evidence: vec![EvidencePointer::command(id, b"x")],
                tier: Tier::Judgment,
                blast: Some(Lane::Standard),
                judgment: true,
                fix_hint: None,
            }),
        )
        .unwrap();
    }
    let a = analyze(&l, SID).unwrap();
    let tension = verdict_core::verdict::dissent(&a);
    assert_eq!(tension.len(), 1, "{tension:?}");
    assert!(
        tension[0].contains("disagree about claim record 3"),
        "{tension:?}"
    );
    let c = compute(&l, SID, &input(Lane::Standard)).unwrap();
    let report = report_with(&l, &c);
    assert!(
        report.contains("DESK DISSENT (both findings stay)"),
        "{report}"
    );
}
