//! Fix 2b: proof bound to the code it checked, project-wide overrides, exact-occurrence
//! acceptance, an unreadable diff never lowering the lane, and one line per unavailable component.

mod p2b_common;

use p2b_common::*;
use verdict_core::baseline::tree_state;
use verdict_core::gate::run_verdict;
use verdict_core::human::{Action, apply};
use verdict_core::part1c::open_decisions;
use verdict_core::verdict::{AcceptChoice, CommitAnywayChoice, STALE_CHECK, compute, live_input};
use verdict_core::*;

fn l() -> (Tmp, Ledger) {
    let t = Tmp::new("fix2b");
    let l = seeded(t.path());
    (t, l)
}

fn check_in(l: &mut Ledger, sid: &str, name: &str, tier: Tier, tree: Option<&str>) -> u64 {
    l.append(
        Writer::Core,
        Some(sid),
        Record::Check {
            name: name.into(),
            tier,
            inputs_hash: "h".into(),
            started_at: "2026-10-26T12:00:00.000Z".into(),
            finished_at: "2026-10-26T12:00:01.000Z".into(),
            outcome: CheckOutcome::Ran,
            detail: None,
            tree_state: tree.map(str::to_string),
        },
    )
    .unwrap()
    .seq
}

fn tier_of(name: &str) -> Tier {
    match name {
        "exec" | "mutation" => Tier::Ran,
        "firstrun" => Tier::Browsed,
        "release" => Tier::Asset,
        _ => Tier::Static,
    }
}

/// Every checker of `lane`, in `sid`, recorded as having seen `tree`.
fn run_lane_in(l: &mut Ledger, sid: &str, lane: Lane, tree: Option<&str>) {
    for name in input(lane).lane_checkers[&lane].clone() {
        check_in(l, sid, &name, tier_of(&name), tree);
    }
}

// ------------------------------------------------------------- 1. proof bound to the code

#[test]
fn a_check_counts_only_for_the_code_it_saw() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);

    // Same ledger, but the code changed after the checks ran.
    let mut i = input(Lane::Standard);
    i.current_tree = Some("the code is different now".into());
    let c = compute(&l, SID, &i).unwrap();
    assert_eq!(c.word, VerdictWord::Partial, "{:#?}", c.not_established);
    assert!(
        c.not_established.iter().any(|s| s.contains(STALE_CHECK)),
        "{:#?}",
        c.not_established
    );

    // When the state of the code now cannot be read, no check can be shown to match it.
    i.current_tree = None;
    assert_eq!(compute(&l, SID, &i).unwrap().word, VerdictWord::Partial);
}

#[test]
fn a_stale_release_run_can_never_be_clear_to_ship() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Release);
    assert_eq!(verdict_of(&l, Lane::Release).word, VerdictWord::ClearToShip);
    let mut i = input(Lane::Release);
    i.current_tree = Some("edited after the release run".into());
    let c = compute(&l, SID, &i).unwrap();
    assert_eq!(c.word, VerdictWord::Partial);
    let text = report_with(&l, &c);
    assert!(text.starts_with("VERDICT: PARTIAL"), "{text}");
    assert!(text.contains(STALE_CHECK), "{text}");
}

#[test]
fn a_check_with_no_tree_state_is_treated_as_stale() {
    let (_t, mut l) = l();
    run_lane_in(&mut l, SID, Lane::Standard, None);
    let c = verdict_of(&l, Lane::Standard);
    assert_eq!(c.word, VerdictWord::Partial);
    assert!(c.not_established.iter().any(|s| s.contains(STALE_CHECK)));
}

#[test]
fn rerunning_on_the_current_code_counts_again() {
    let (_t, mut l) = l();
    run_lane_in(&mut l, SID, Lane::Standard, Some("old"));
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Partial);
    run_lane_in(&mut l, SID, Lane::Standard, Some(TEST_TREE));
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);
}

#[test]
fn the_tree_state_follows_the_content_not_the_commit() {
    let repo = git_repo("fix2b-tree");
    let root = repo.path();
    let s0 = tree_state(root).expect("a git tree state");
    assert!(s0.starts_with("git:"), "{s0}");
    assert_eq!(tree_state(root).unwrap(), s0, "stable when nothing changed");

    std::fs::write(root.join("README.md"), "changed\n").unwrap();
    let s1 = tree_state(root).unwrap();
    assert_ne!(s1, s0, "an unstaged edit changes it");
    git(root, &["add", "README.md"]);
    assert_eq!(tree_state(root).unwrap(), s1, "staging does not");
    git(root, &["commit", "-q", "-m", "second"]);
    assert_eq!(tree_state(root).unwrap(), s1, "committing does not");

    std::fs::write(root.join("new.txt"), "x\n").unwrap();
    let s2 = tree_state(root).unwrap();
    assert_ne!(s2, s1, "a new untracked file changes it");
    std::fs::remove_file(root.join("new.txt")).unwrap();
    assert_eq!(tree_state(root).unwrap(), s1);

    // Verdict's own folder never counts.
    std::fs::create_dir_all(root.join(".verdict")).unwrap();
    std::fs::write(root.join(".verdict").join("x"), "y").unwrap();
    assert_eq!(tree_state(root).unwrap(), s1);

    // Outside git, a folder hash.
    let plain = Tmp::new("fix2b-plain");
    std::fs::write(plain.path().join("a.txt"), "1").unwrap();
    let f0 = tree_state(plain.path()).unwrap();
    assert!(f0.starts_with("folder:"), "{f0}");
    std::fs::write(plain.path().join("a.txt"), "2").unwrap();
    assert_ne!(tree_state(plain.path()).unwrap(), f0);
}

#[test]
fn a_ledger_written_before_tree_state_existed_still_verifies() {
    // Written by the code at commit 298757f (before this field existed); see the report.
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ledger-before-tree-state");
    let t = Tmp::new("fix2b-old-ledger");
    let dir = t.path().join(".verdict");
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["ledger.db", "ledger.head"] {
        std::fs::copy(fixture.join(f), dir.join(f)).unwrap();
    }
    let mut l = Ledger::open_existing(t.path()).unwrap().unwrap();
    let report = l.verify().unwrap();
    assert!(report.is_ok(), "{report:?}");
    // The last hash is the one the old code wrote in its head file.
    let (seq, hash) = l.head().unwrap().unwrap();
    assert_eq!(seq, 4);
    assert_eq!(
        hash,
        "136588706f46502e43b53a8c59d9e27518b2cb94f66c2735da97cbf59433fb67"
    );
    let checks: Vec<_> = l
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::Check { tree_state, .. } => Some(tree_state),
            _ => None,
        })
        .collect();
    assert_eq!(
        checks,
        vec![None, None],
        "old checks read with no tree state"
    );

    // New records chain on, and the whole ledger still verifies.
    l.append(
        Writer::Core,
        Some("old-session"),
        Record::Check {
            name: "wiring".into(),
            tier: Tier::Static,
            inputs_hash: "h".into(),
            started_at: "a".into(),
            finished_at: "b".into(),
            outcome: CheckOutcome::Ran,
            detail: None,
            tree_state: Some("t".into()),
        },
    )
    .unwrap();
    assert!(l.verify().unwrap().is_ok());
    // The old checks do not count for the code now.
    let mut i = input(Lane::Low);
    i.current_tree = Some("anything".into());
    let c = compute(&l, "old-session", &i).unwrap();
    assert!(
        c.not_established.iter().any(|s| s.contains(STALE_CHECK)),
        "{:#?}",
        c.not_established
    );
}

// ------------------------------------------------------------- 2. project-wide overrides

const S2: &str = "s-later";

fn anyway_in(l: &mut Ledger, sid: &str, ids: &[&str]) -> u64 {
    human_in(
        l,
        sid,
        DecisionKind::CommitAnyway,
        &CommitAnywayChoice {
            tree: "tree1".into(),
            finding_ids: ids.iter().map(|s| s.to_string()).collect(),
        }
        .encode(),
        Some("hotfix, will fix Monday"),
    )
}

/// Session one has a nit finding it committed past; session two ran the whole release lane clean.
fn override_then_later_session() -> (Tmp, Ledger, u64, u64) {
    let (t, mut l) = l();
    let f = find(&mut l, "release:note", Severity::Nit, Tier::Static);
    let ov = anyway_in(&mut l, SID, &["release:note"]);
    l.append(Writer::Adapter, Some(S2), session_record(S2))
        .unwrap();
    run_lane_in(&mut l, S2, Lane::Release, Some(TEST_TREE));
    (t, l, f, ov)
}

#[test]
fn an_override_from_an_earlier_session_blocks_clear_to_ship_in_a_later_one() {
    let (_t, l, _f, ov) = override_then_later_session();
    let c = compute(&l, S2, &input(Lane::Release)).unwrap();
    assert_ne!(c.word, VerdictWord::ClearToShip, "{c:#?}");
    assert!(c.override_open);
    assert!(
        c.yours_to_settle
            .iter()
            .any(|s| s.contains(&format!("record {ov}, from session {SID}"))),
        "{:#?}",
        c.yours_to_settle
    );
    let text = report_with(&l, &c);
    assert!(text.contains(&format!("from session {SID}")), "{text}");
}

#[test]
fn accepting_the_finding_in_any_session_closes_the_override_everywhere() {
    let (t, mut l, f, _ov) = override_then_later_session();
    // The human answers from the later session, naming the exact finding by its record number.
    apply(
        t.path(),
        &mut l,
        S2,
        &Action::AcceptFinding(f.to_string()),
        None,
    )
    .unwrap();
    let c = compute(&l, S2, &input(Lane::Release)).unwrap();
    assert_eq!(c.word, VerdictWord::ClearToShip, "{:#?}", c.yours_to_settle);
    assert!(!c.override_open);
}

#[test]
fn a_correction_of_the_finding_closes_the_override_everywhere() {
    let (_t, mut l, f, _ov) = override_then_later_session();
    l.append(
        Writer::Core,
        Some(SID),
        Record::Correction {
            target_seq: f,
            reason: "fixed".into(),
            replacement: None,
        },
    )
    .unwrap();
    let c = compute(&l, S2, &input(Lane::Release)).unwrap();
    assert_eq!(c.word, VerdictWord::ClearToShip);
}

#[test]
fn a_later_clean_run_of_the_same_checker_resolves_the_overridden_finding() {
    let (_t, mut l) = l();
    find(&mut l, "release:note", Severity::Nit, Tier::Static);
    // The run that reported it.
    check_in(&mut l, SID, "release", Tier::Asset, Some(TEST_TREE));
    anyway_in(&mut l, SID, &["release:note"]);
    l.append(Writer::Adapter, Some(S2), session_record(S2))
        .unwrap();
    // Session two runs everything, release included, and release no longer reports it.
    run_lane_in(&mut l, S2, Lane::Release, Some(TEST_TREE));
    let c = compute(&l, S2, &input(Lane::Release)).unwrap();
    assert_eq!(c.word, VerdictWord::ClearToShip, "{:#?}", c.yours_to_settle);
}

// ------------------------------------------------------------- 4. exact occurrence

fn finding_with(id: &str, evidence: &[u8]) -> Finding {
    Finding {
        id: id.into(),
        severity: Severity::Major,
        title: format!("problem {id}"),
        claim_seq: None,
        evidence: vec![EvidencePointer::command("cargo test", evidence)],
        tier: Tier::Static,
        blast: None,
        judgment: false,
        fix_hint: None,
    }
}

fn find_with(l: &mut Ledger, id: &str, evidence: &[u8]) -> u64 {
    l.append(
        Writer::Checker,
        Some(SID),
        Record::Finding(finding_with(id, evidence)),
    )
    .unwrap()
    .seq
}

#[test]
fn accepting_a_finding_closes_that_occurrence_and_not_a_new_one() {
    let (t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    let first = find_with(&mut l, "wiring:a", b"old evidence");
    let d = apply(
        t.path(),
        &mut l,
        SID,
        &Action::AcceptFinding("wiring:a".into()),
        None,
    )
    .unwrap();
    let Record::Decision { choice, .. } = l.get(d).unwrap().unwrap().record else {
        panic!()
    };
    let c = AcceptChoice::decode(&choice);
    assert_eq!(c.seq, Some(first));
    assert!(c.evidence.is_some());
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);

    // The same problem reported again (same id, same evidence) stays closed.
    find_with(&mut l, "wiring:a", b"old evidence");
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);

    // Same id, different evidence: a new occurrence, open again, and asked about again.
    let second = find_with(&mut l, "wiring:a", b"new evidence");
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Failed);
    assert!(
        open_decisions(&l, Some(SID))
            .unwrap()
            .iter()
            .any(|d| d.decision == DecisionKind::AcceptFindingNotReal
                && d.record_seq == Some(second)),
    );
}

#[test]
fn an_older_id_only_acceptance_never_closes_a_finding_recorded_after_it() {
    let (_t, mut l) = l();
    run_all_required(&mut l, Lane::Standard);
    find_with(&mut l, "wiring:a", b"one");
    // A decision written before this fix: the choice is just the id.
    human(&mut l, DecisionKind::AcceptFindingNotReal, "wiring:a", None);
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Verified);
    find_with(&mut l, "wiring:a", b"two");
    assert_eq!(verdict_of(&l, Lane::Standard).word, VerdictWord::Failed);
}

#[test]
fn a_record_number_that_is_not_an_open_finding_is_refused() {
    let (t, mut l) = l();
    let f = find_with(&mut l, "wiring:a", b"x");
    let before = l.count().unwrap();
    for bad in ["9999", "1", "no-such"] {
        assert!(
            apply(
                t.path(),
                &mut l,
                SID,
                &Action::AcceptFinding(bad.into()),
                None
            )
            .is_err(),
            "{bad}"
        );
    }
    assert_eq!(l.count().unwrap(), before);
    apply(
        t.path(),
        &mut l,
        SID,
        &Action::AcceptFinding(format!("#{f}")),
        None,
    )
    .unwrap();
    // Twice is refused.
    assert!(
        apply(
            t.path(),
            &mut l,
            SID,
            &Action::AcceptFinding(f.to_string()),
            None
        )
        .is_err()
    );
}

// ------------------------------------------------------------- 6. prompt translation (core)

#[test]
fn accepting_a_prompt_translation_names_the_claim_and_closes_the_question() {
    let (t, mut l) = l();
    let claim = l
        .append(
            Writer::Agent,
            Some(SID),
            Record::Claim {
                source: ClaimSource::PromptTranslation,
                text: "src/billing.rs charges the card".into(),
                names: vec!["src/billing.rs".into()],
                prompt_ordinal: Some(1),
            },
        )
        .unwrap()
        .seq;
    let open = open_decisions(&l, Some(SID)).unwrap();
    let q = open
        .iter()
        .find(|d| d.decision == DecisionKind::AcceptPromptTranslation)
        .expect("an open translation");
    assert_eq!(q.record_seq, Some(claim));
    assert_eq!(
        q.claim_text.as_deref(),
        Some("src/billing.rs charges the card")
    );
    assert_eq!(q.added_names, vec!["src/billing.rs".to_string()]);

    // Not a translation claim: refused, nothing written.
    let before = l.count().unwrap();
    assert!(
        apply(
            t.path(),
            &mut l,
            SID,
            &Action::AcceptPromptTranslation(2),
            None
        )
        .is_err()
    );
    assert_eq!(l.count().unwrap(), before);

    let d = apply(
        t.path(),
        &mut l,
        SID,
        &Action::AcceptPromptTranslation(claim),
        None,
    )
    .unwrap();
    let rec = l.get(d).unwrap().unwrap();
    assert_eq!(rec.writer, Writer::Human);
    assert!(matches!(
        &rec.record,
        Record::Decision { decision: DecisionKind::AcceptPromptTranslation, choice, who: Writer::Human, .. }
            if *choice == claim.to_string()
    ));
    assert!(
        !open_decisions(&l, Some(SID))
            .unwrap()
            .iter()
            .any(|d| d.decision == DecisionKind::AcceptPromptTranslation)
    );
    // Answered once; a second press is refused.
    assert!(
        apply(
            t.path(),
            &mut l,
            SID,
            &Action::AcceptPromptTranslation(claim),
            None
        )
        .is_err()
    );
}

// ------------------------------------------------------------- 5. unreadable diff

/// A repository whose session baseline commit no longer exists (history was rewritten).
fn repo_with_lost_baseline() -> (Tmp, Tmp, String) {
    let repo = git_repo("fix2b-lost");
    let home = Tmp::new("fix2b-lost-home");
    let (_l, sid) = repo_ledger(repo.path(), home.path());
    let root = repo.path();
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = branch.trim().to_string();
    git(root, &["checkout", "-q", "--orphan", "rewritten"]);
    git(root, &["commit", "-q", "-m", "rewritten history"]);
    git(root, &["branch", "-q", "-D", &branch]);
    git(root, &["reflog", "expire", "--expire=now", "--all"]);
    git(root, &["gc", "-q", "--prune=now"]);
    (repo, home, sid)
}

#[test]
fn an_unreadable_diff_classifies_at_release_and_the_verdict_is_never_clean() {
    let (repo, home, sid) = repo_with_lost_baseline();
    let root = repo.path();
    let mut l = Ledger::open(root).unwrap();
    // Every release checker ran on the code as it is now, so only the diff is missing.
    let now = tree_state(root);
    run_lane_in(&mut l, &sid, Lane::Release, now.as_deref());
    let live = live_input(root, &l, &sid, Some(home.path())).unwrap();
    assert_eq!(live.input.implied_lane, Lane::Release);
    let why = live
        .input
        .diff_unavailable
        .clone()
        .expect("the diff is unreadable");
    assert!(why.contains("baseline commit"), "{why}");
    let c = compute(&l, &sid, &live.input).unwrap();
    assert_eq!(c.lane, Lane::Release);
    assert!(
        !matches!(c.word, VerdictWord::Verified | VerdictWord::ClearToShip),
        "{:?}",
        c.word
    );
    assert!(
        c.not_established
            .iter()
            .any(|s| s.starts_with("lane classification: could not run")),
        "{:#?}",
        c.not_established
    );
    drop(l);

    // `vdt verdict` records it as check_unavailable for lane classification, once.
    run_verdict(root, Some(home.path()), Some(&sid), None).unwrap();
    run_verdict(root, Some(home.path()), Some(&sid), None).unwrap();
    let l = Ledger::open(root).unwrap();
    let lane_records: Vec<String> = l
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::CheckUnavailable { component, reason }
                if component == verdict_core::lane::LANE_COMPONENT =>
            {
                Some(reason)
            }
            _ => None,
        })
        .collect();
    assert_eq!(lane_records.len(), 1, "{lane_records:#?}");
    assert!(lane_records[0].contains("release"), "{lane_records:#?}");
}

// ------------------------------------------------------------- 7. one line per component

#[test]
fn yours_to_settle_lists_each_unavailable_component_once() {
    let (_t, mut l) = l();
    // A gate run and then `vdt verdict --run` both say the adapter could not run.
    unavailable(&mut l, "adapter");
    unavailable(&mut l, "adapter");
    let latest = unavailable(&mut l, "adapter");
    let c = verdict_of(&l, Lane::Standard);
    let text = report_with(&l, &c);
    let settle = text
        .split("YOURS TO SETTLE")
        .nth(1)
        .and_then(|s| s.split("\n\n").next())
        .unwrap_or("");
    assert_eq!(
        settle.matches("check unavailable: adapter").count(),
        1,
        "{text}"
    );
    assert!(settle.contains(&format!("(record {latest})")), "{text}");
    assert_eq!(
        c.yours_to_settle
            .iter()
            .filter(|s| s.starts_with("adapter could not run"))
            .count(),
        1,
        "{:#?}",
        c.yours_to_settle
    );
}
