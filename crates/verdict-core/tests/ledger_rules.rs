mod common;

use common::*;
use verdict_core::*;

const SID: &str = "s-test000001";

#[test]
fn every_record_type_can_be_written_by_its_writer() {
    let t = TempDir::new("kinds");
    let mut l = seeded(t.path());
    let ev = EvidencePointer::command("cargo test", b"ok");
    let cases: Vec<(Writer, Record)> = vec![
        (
            Writer::Core,
            Record::Check {
                name: "wiring".into(),
                tier: Tier::Static,
                inputs_hash: "h".into(),
                started_at: "a".into(),
                finished_at: "b".into(),
                outcome: CheckOutcome::Ran,
                detail: None,
            },
        ),
        (
            Writer::Checker,
            Record::Finding(finding("f1", vec![ev.clone()])),
        ),
        (Writer::Desk, Record::Finding(finding("f2", vec![]))),
        (
            Writer::Human,
            Record::Decision {
                decision: DecisionKind::ModeSwitch,
                choice: "enforce".into(),
                reason: None,
                who: Writer::Human,
            },
        ),
        (
            Writer::Core,
            Record::Verdict {
                word: VerdictWord::Unverified,
                highest_tier: None,
                counts_by_tier: Default::default(),
                not_established: vec!["everything".into()],
                policy_version: 1,
            },
        ),
        (
            Writer::Adapter,
            Record::CheckUnavailable {
                component: "adapter".into(),
                reason: "core not found".into(),
            },
        ),
    ];
    for (w, r) in cases {
        let kind = r.kind();
        l.append(w, Some(SID), r)
            .unwrap_or_else(|e| panic!("{w} {kind}: {e}"));
    }
    let kinds: Vec<_> = l.records().unwrap().iter().map(|s| s.kind).collect();
    for k in [
        RecordKind::Session,
        RecordKind::Prompt,
        RecordKind::Claim,
        RecordKind::ToolCall,
        RecordKind::Check,
        RecordKind::Finding,
        RecordKind::Hypothesis,
        RecordKind::Decision,
        RecordKind::Verdict,
        RecordKind::CheckUnavailable,
    ] {
        assert!(kinds.contains(&k), "missing {k}");
    }
    assert!(l.verify().unwrap().is_ok());
}

#[test]
fn agent_cannot_write_finding_decision_or_verdict_and_it_is_recorded() {
    let mut l = Ledger::open_in_memory().unwrap();
    let attempts = vec![
        Record::Finding(finding("x", vec![EvidencePointer::command("c", b"o")])),
        Record::Decision {
            decision: DecisionKind::AcceptFindingNotReal,
            choice: "x".into(),
            reason: None,
            who: Writer::Human,
        },
        Record::Decision {
            decision: DecisionKind::CommitAnyway,
            choice: "commit".into(),
            reason: Some("because".into()),
            who: Writer::Human,
        },
        Record::Decision {
            decision: DecisionKind::ModeSwitch,
            choice: "enforce".into(),
            reason: None,
            who: Writer::Human,
        },
        Record::Verdict {
            word: VerdictWord::ClearToShip,
            highest_tier: Some(Tier::Ran),
            counts_by_tier: Default::default(),
            not_established: vec![],
            policy_version: 1,
        },
        Record::Check {
            name: "n".into(),
            tier: Tier::Ran,
            inputs_hash: "h".into(),
            started_at: "a".into(),
            finished_at: "b".into(),
            outcome: CheckOutcome::Ran,
            detail: None,
        },
    ];
    for (i, attempt) in attempts.into_iter().enumerate() {
        let kind = attempt.kind();
        let before = l.count().unwrap();
        let err = l.append(Writer::Agent, Some(SID), attempt).unwrap_err();
        let LedgerError::Refused { refused_seq, .. } = err else {
            panic!("attempt {i} ({kind}) was not refused: {err}");
        };
        assert_eq!(
            l.count().unwrap(),
            before + 1,
            "only the refusal was stored"
        );
        let stored = l.get(refused_seq).unwrap().unwrap();
        assert_eq!(stored.kind, RecordKind::RefusedWrite);
        match stored.record {
            Record::RefusedWrite {
                attempted_by,
                attempted_kind,
                ..
            } => {
                assert_eq!(attempted_by, Writer::Agent);
                assert_eq!(attempted_kind, kind);
            }
            other => panic!("{other:?}"),
        }
    }
    let kinds: Vec<_> = l.records().unwrap().iter().map(|s| s.kind).collect();
    assert!(kinds.iter().all(|k| *k == RecordKind::RefusedWrite));
}

#[test]
fn commit_anyway_and_mode_switch_refusals_name_the_decision() {
    let mut l = Ledger::open_in_memory().unwrap();
    let d = |k| Record::Decision {
        decision: k,
        choice: "x".into(),
        reason: Some("r".into()),
        who: Writer::Human,
    };
    for k in [DecisionKind::CommitAnyway, DecisionKind::ModeSwitch] {
        let e = l.append(Writer::Adapter, None, d(k)).unwrap_err();
        assert!(matches!(e, LedgerError::Refused { .. }));
    }
    let r = l.records().unwrap();
    assert!(matches!(
        &r[0].record,
        Record::RefusedWrite {
            attempted_decision: Some(DecisionKind::CommitAnyway),
            ..
        }
    ));
    assert!(matches!(
        &r[1].record,
        Record::RefusedWrite {
            attempted_decision: Some(DecisionKind::ModeSwitch),
            ..
        }
    ));
}

#[test]
fn only_the_human_writes_decisions_and_only_core_writes_verdicts_and_checks() {
    for w in Writer::ALL {
        assert_eq!(w.may_write(RecordKind::Decision), w == Writer::Human, "{w}");
        assert_eq!(w.may_write(RecordKind::Verdict), w == Writer::Core, "{w}");
        assert_eq!(w.may_write(RecordKind::Check), w == Writer::Core, "{w}");
        assert_eq!(
            w.may_write(RecordKind::Finding),
            matches!(w, Writer::Checker | Writer::Desk),
            "{w}"
        );
    }
}

#[test]
fn commit_anyway_needs_a_typed_reason_from_the_human() {
    let mut l = Ledger::open_in_memory().unwrap();
    let mk = |reason: Option<&str>| Record::Decision {
        decision: DecisionKind::CommitAnyway,
        choice: "commit".into(),
        reason: reason.map(Into::into),
        who: Writer::Human,
    };
    assert!(matches!(
        l.append(Writer::Human, None, mk(None)),
        Err(LedgerError::Invalid(_))
    ));
    assert!(matches!(
        l.append(Writer::Human, None, mk(Some("  "))),
        Err(LedgerError::Invalid(_))
    ));
    l.append(Writer::Human, None, mk(Some("shipping the hotfix")))
        .unwrap();
}

#[test]
fn finding_without_evidence_is_stored_as_a_hypothesis() {
    let mut l = Ledger::open_in_memory().unwrap();
    let a = l
        .append(
            Writer::Desk,
            Some(SID),
            Record::Finding(finding("h", vec![])),
        )
        .unwrap();
    assert!(a.stored_as_hypothesis);
    assert_eq!(a.kind, RecordKind::Hypothesis);
    assert_eq!(l.get(a.seq).unwrap().unwrap().kind, RecordKind::Hypothesis);

    let ev = EvidencePointer::command("cargo test", b"output");
    let b = l
        .append(
            Writer::Checker,
            Some(SID),
            Record::Finding(finding("f", vec![ev])),
        )
        .unwrap();
    assert!(!b.stored_as_hypothesis);
    assert_eq!(b.kind, RecordKind::Finding);
}

#[test]
fn correction_references_the_old_record_and_never_edits_it() {
    let mut l = Ledger::open_in_memory().unwrap();
    let c = l.append(Writer::Agent, Some(SID), claim("v1")).unwrap();
    let before = l.get(c.seq).unwrap().unwrap();
    let fix = l
        .append(
            Writer::Agent,
            Some(SID),
            Record::Correction {
                target_seq: c.seq,
                reason: "typo".into(),
                replacement: Some(Box::new(claim("v2"))),
            },
        )
        .unwrap();
    assert_eq!(
        l.get(c.seq).unwrap().unwrap(),
        before,
        "old record untouched"
    );
    assert_eq!(l.corrections_for(c.seq).unwrap()[0].seq, fix.seq);
    assert!(l.verify().unwrap().is_ok());

    // Missing target is an invalid request, not a write.
    assert!(matches!(
        l.append(
            Writer::Core,
            None,
            Record::Correction {
                target_seq: 999,
                reason: "x".into(),
                replacement: None
            }
        ),
        Err(LedgerError::Invalid(_))
    ));
    // An agent cannot use a correction to smuggle in a verdict.
    let e = l
        .append(
            Writer::Agent,
            Some(SID),
            Record::Correction {
                target_seq: c.seq,
                reason: "sneaky".into(),
                replacement: Some(Box::new(Record::Verdict {
                    word: VerdictWord::ClearToShip,
                    highest_tier: None,
                    counts_by_tier: Default::default(),
                    not_established: vec![],
                    policy_version: 1,
                })),
            },
        )
        .unwrap_err();
    assert!(matches!(e, LedgerError::Refused { .. }));
    // An agent cannot correct someone else's record.
    let t = l
        .append(Writer::Adapter, Some(SID), tool_call("Edit", "a"))
        .unwrap();
    let e = l
        .append(
            Writer::Agent,
            Some(SID),
            Record::Correction {
                target_seq: t.seq,
                reason: "x".into(),
                replacement: None,
            },
        )
        .unwrap_err();
    assert!(matches!(e, LedgerError::Refused { .. }));
}

#[test]
fn storage_is_sqlite_wal_at_the_documented_path_and_exports_jsonl() {
    let t = TempDir::new("wal");
    let l = seeded(t.path());
    let db = t.path().join(".verdict").join("ledger.db");
    assert!(db.is_file());
    let mode: String = rusqlite_conn(&db)
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");

    let mut out = Vec::new();
    l.export_jsonl(&mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 5);
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(line["seq"], i as u64 + 1);
        assert!(line["hash"].as_str().unwrap().len() == 64);
        assert!(line["body"]["type"].is_string());
    }
    assert_eq!(lines[0]["prev_hash"], "0".repeat(64));
    assert_eq!(lines[1]["prev_hash"], lines[0]["hash"]);
}

#[test]
fn sequence_is_monotonic_and_the_chain_links_every_record() {
    let t = TempDir::new("chain");
    let l = seeded(t.path());
    let recs = l.records().unwrap();
    for (i, r) in recs.iter().enumerate() {
        assert_eq!(r.seq, i as u64 + 1);
        if i > 0 {
            assert_eq!(r.prev_hash, recs[i - 1].hash);
        }
    }
}

#[test]
fn raw_sql_cannot_update_or_delete_or_forge_an_insert() {
    let t = TempDir::new("rawsql");
    let _l = seeded(t.path());
    let db = t.path().join(".verdict").join("ledger.db");
    let c = rusqlite_conn(&db);
    let e = c
        .execute("UPDATE records SET body = 'x' WHERE seq = 1", [])
        .unwrap_err();
    assert!(e.to_string().contains("append-only"), "{e}");
    let e = c
        .execute("DELETE FROM records WHERE seq = 5", [])
        .unwrap_err();
    assert!(e.to_string().contains("append-only"), "{e}");
    let e = c.execute("DELETE FROM records", []).unwrap_err();
    assert!(e.to_string().contains("append-only"), "{e}");
    let e = c
        .execute(
            "INSERT INTO records (seq, ts, writer, session_id, kind, body, prev_hash, hash)
             VALUES (99, 't', 'agent', NULL, 'verdict', '{}', 'nope', 'nope')",
            [],
        )
        .unwrap_err();
    assert!(e.to_string().contains("chain"), "{e}");
    // Nothing changed.
    let l = Ledger::open(t.path()).unwrap();
    assert!(l.verify().unwrap().is_ok());
    assert_eq!(l.count().unwrap(), 5);
}

pub fn rusqlite_conn(db: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(db).unwrap()
}
