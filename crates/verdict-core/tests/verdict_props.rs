//! Property test of the one rule: nothing an agent can write (claims, prompts, tool calls) buys a
//! better verdict than UNVERIFIED, whatever it writes, whichever lane, however it tries.

mod p2b_common;

use p2b_common::*;
use proptest::prelude::*;
use verdict_core::*;

#[derive(Debug, Clone)]
enum Move {
    Claim(String),
    Prompt(String),
    ToolCall(String),
    // Things the agent should not be able to write at all.
    Finding,
    Check(usize),
    Verdict,
    Decision(usize),
    Correction,
    Session,
}

const DECISIONS: [DecisionKind; 6] = [
    DecisionKind::AcceptFindingNotReal,
    DecisionKind::CommitAnyway,
    DecisionKind::RaiseLane,
    DecisionKind::ModeSwitch,
    DecisionKind::RunProjectCommands,
    DecisionKind::Pause,
];

fn text() -> impl Strategy<Value = String> {
    "[a-z/_.]{1,12}( [a-z]{1,8}){0,3}"
}

fn moves() -> impl Strategy<Value = Vec<(bool, Move)>> {
    let mv = prop_oneof![
        text().prop_map(Move::Claim),
        text().prop_map(Move::Prompt),
        text().prop_map(Move::ToolCall),
        Just(Move::Finding),
        (0usize..10).prop_map(Move::Check),
        Just(Move::Verdict),
        (0usize..6).prop_map(Move::Decision),
        Just(Move::Correction),
        Just(Move::Session),
    ];
    // (true: written as the agent, false: written as the adapter)
    proptest::collection::vec((any::<bool>(), mv), 0..40)
}

fn build(m: &Move) -> Record {
    match m {
        Move::Claim(t) => Record::Claim {
            source: ClaimSource::FinalMessage,
            text: t.clone(),
            names: vec!["src/a.rs".into()],
            prompt_ordinal: Some(1),
        },
        Move::Prompt(t) => Record::Prompt {
            ordinal: 1,
            text: t.clone(),
            agent: "claude".into(),
            model: "m".into(),
        },
        Move::ToolCall(t) => Record::ToolCall {
            tool: "edit".into(),
            target: t.clone(),
            output_hash: "0".repeat(64),
            duration_ms: 1,
        },
        Move::Finding => Record::Finding(finding_record(
            "wiring:x",
            Severity::Blocker,
            Tier::Static,
            false,
            true,
        )),
        Move::Check(i) => Record::Check {
            name: verdict_core::lane::ALL_CHECKERS[*i % 10].into(),
            tier: Tier::Browsed,
            inputs_hash: "h".into(),
            started_at: "a".into(),
            finished_at: "b".into(),
            outcome: CheckOutcome::Ran,
            detail: None,
            tree_state: Some(TEST_TREE.into()),
        },
        Move::Verdict => Record::Verdict {
            word: VerdictWord::ClearToShip,
            highest_tier: Some(Tier::Browsed),
            counts_by_tier: Default::default(),
            not_established: vec![],
            policy_version: 1,
            lane: Some(Lane::Release),
        },
        Move::Decision(i) => Record::Decision {
            decision: DECISIONS[*i % DECISIONS.len()],
            choice: "release".into(),
            reason: Some("typed".into()),
            who: Writer::Human,
        },
        Move::Correction => Record::Correction {
            target_seq: 1,
            reason: "x".into(),
            replacement: None,
        },
        Move::Session => session_record(SID),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Whatever an agent or an adapter manages to write, no lane reads better than UNVERIFIED.
    #[test]
    fn no_agent_written_ledger_is_better_than_unverified(seq in moves()) {
        let t = Tmp::new("prop");
        let mut l = seeded(t.path());
        for (as_agent, m) in &seq {
            let w = if *as_agent { Writer::Agent } else { Writer::Adapter };
            // Refusals are expected and recorded; only what the ledger accepts is in it.
            let _ = l.append(w, Some(SID), build(m));
        }
        for lane in [Lane::Low, Lane::Standard, Lane::High, Lane::Release] {
            let c = verdict_of(&l, lane);
            prop_assert_eq!(c.word, VerdictWord::Unverified, "lane {:?}: {:?}", lane, c.not_established);
            prop_assert!(c.highest_tier.is_none());
        }
        // The text agrees.
        let c = verdict_of(&l, Lane::Release);
        let text = report_with(&l, &c);
        prop_assert!(text.starts_with("VERDICT: UNVERIFIED"));
        prop_assert!(!text.contains("CLEAR TO SHIP") || text.contains("withheld"));
    }
}
