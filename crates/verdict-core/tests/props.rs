mod common;

use common::*;
use proptest::prelude::*;
use verdict_core::*;

#[derive(Debug, Clone)]
enum Spec {
    Prompt(String),
    Claim(String),
    ToolCall(String),
    Finding { with_evidence: bool },
    Hypothesis,
    Decision(usize),
    Verdict,
    Check,
    Unavailable(String),
    Session,
}

const DECISIONS: [DecisionKind; 4] = [
    DecisionKind::ModeSwitch,
    DecisionKind::CommitAnyway,
    DecisionKind::AcceptFindingNotReal,
    DecisionKind::RaiseLane,
];

fn build(spec: &Spec) -> Record {
    match spec {
        Spec::Prompt(t) => prompt(1, t),
        Spec::Claim(t) => claim(t),
        Spec::ToolCall(t) => tool_call("Edit", t),
        Spec::Finding { with_evidence } => Record::Finding(finding(
            "f",
            if *with_evidence {
                vec![EvidencePointer::command("cargo test", b"out")]
            } else {
                vec![]
            },
        )),
        Spec::Hypothesis => Record::Hypothesis(finding("h", vec![])),
        Spec::Decision(i) => Record::Decision {
            decision: DECISIONS[*i % DECISIONS.len()],
            choice: "x".into(),
            reason: Some("typed reason".into()),
            who: Writer::Human,
        },
        Spec::Verdict => Record::Verdict {
            word: VerdictWord::Verified,
            highest_tier: Some(Tier::Ran),
            counts_by_tier: Default::default(),
            not_established: vec![],
            policy_version: 1,
        },
        Spec::Check => Record::Check {
            name: "wiring".into(),
            tier: Tier::Static,
            inputs_hash: "h".into(),
            started_at: "a".into(),
            finished_at: "b".into(),
            outcome: CheckOutcome::Ran,
            detail: None,
        },
        Spec::Unavailable(t) => Record::CheckUnavailable {
            component: "adapter".into(),
            reason: t.clone(),
        },
        Spec::Session => session_record("s-prop"),
    }
}

fn spec_strategy() -> impl Strategy<Value = Spec> {
    let text = "[a-z ]{0,20}";
    prop_oneof![
        text.prop_map(Spec::Prompt),
        text.prop_map(Spec::Claim),
        text.prop_map(Spec::ToolCall),
        any::<bool>().prop_map(|b| Spec::Finding { with_evidence: b }),
        Just(Spec::Hypothesis),
        (0usize..4).prop_map(Spec::Decision),
        Just(Spec::Verdict),
        Just(Spec::Check),
        text.prop_map(Spec::Unavailable),
        Just(Spec::Session),
    ]
}

fn writer_strategy() -> impl Strategy<Value = Writer> {
    (0usize..Writer::ALL.len()).prop_map(|i| Writer::ALL[i])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn ledger_invariants_hold_for_any_sequence_of_writes(
        ops in proptest::collection::vec((writer_strategy(), spec_strategy()), 1..40)
    ) {
        let mut l = Ledger::open_in_memory().unwrap();
        l.set_clock(fixed_clock());
        let mut refused = 0u64;
        let mut stored_ok = 0u64;
        let mut snapshots: Vec<Vec<Stored>> = Vec::new();

        for (w, spec) in &ops {
            let rec = build(spec);
            let kind = rec.kind();
            let allowed = w.may_write(kind);
            let before = l.count().unwrap();
            let result = l.append(*w, Some("s-prop"), rec);
            if allowed {
                let a = result.unwrap();
                stored_ok += 1;
                // A finding with no evidence never lands as a finding.
                if a.kind == RecordKind::Finding {
                    let Record::Finding(f) = l.get(a.seq).unwrap().unwrap().record else { unreachable!() };
                    prop_assert!(!f.evidence.is_empty());
                }
                prop_assert_eq!(l.count().unwrap(), before + 1);
            } else {
                refused += 1;
                let is_refused = matches!(result, Err(LedgerError::Refused { .. }));
                prop_assert!(is_refused);
                // Only the refusal itself was written.
                prop_assert_eq!(l.count().unwrap(), before + 1);
            }
            snapshots.push(l.records().unwrap());
        }

        let all = l.records().unwrap();
        prop_assert_eq!(all.len() as u64, refused + stored_ok);

        // Chain unbroken, sequence monotonic from 1.
        let report = l.verify().unwrap();
        prop_assert!(report.is_ok(), "{}", report.summary());
        for (i, r) in all.iter().enumerate() {
            prop_assert_eq!(r.seq, i as u64 + 1);
        }

        // Append only: every earlier snapshot is an exact prefix of the final ledger.
        for snap in &snapshots {
            prop_assert_eq!(&all[..snap.len()], &snap[..]);
        }

        // Nobody wrote anything their role forbids, and every refusal is on the record.
        let mut refused_in_ledger = 0u64;
        for r in &all {
            match &r.record {
                Record::RefusedWrite { attempted_by, attempted_kind, .. } => {
                    refused_in_ledger += 1;
                    prop_assert_eq!(r.writer, Writer::Core);
                    prop_assert!(!attempted_by.may_write(*attempted_kind));
                }
                other => {
                    prop_assert!(r.writer.may_write(other.kind()), "{} wrote {}", r.writer, other.kind());
                    if r.writer == Writer::Agent {
                        let ok = matches!(other, Record::Claim { .. } | Record::Correction { .. });
                        prop_assert!(ok);
                    }
                    if let Record::Decision { who, .. } = other {
                        prop_assert_eq!(r.writer, Writer::Human);
                        prop_assert_eq!(*who, Writer::Human);
                    }
                }
            }
        }
        prop_assert_eq!(refused_in_ledger, refused);
    }

    #[test]
    fn any_single_byte_change_to_a_stored_body_is_detected(
        idx in 0usize..5, pos in 0usize..40
    ) {
        let t = TempDir::new("prop-tamper");
        drop(seeded(t.path()));
        let c = rusqlite::Connection::open(t.path().join(".verdict").join("ledger.db")).unwrap();
        c.execute_batch("DROP TRIGGER records_no_update").unwrap();
        let seq = idx as i64 + 1;
        let body: String = c.query_row("SELECT body FROM records WHERE seq = ?1", [seq], |r| r.get(0)).unwrap();
        let mut bytes = body.into_bytes();
        let p = pos % bytes.len();
        bytes[p] = if bytes[p] == b'z' { b'y' } else { b'z' };
        c.execute("UPDATE records SET body = ?1 WHERE seq = ?2",
            rusqlite::params![String::from_utf8(bytes).unwrap(), seq]).unwrap();
        drop(c);
        let r = Ledger::open(t.path()).unwrap().verify().unwrap();
        prop_assert!(!r.is_ok());
        prop_assert_eq!(r.first_broken_seq(), Some(seq as u64));
    }
}
