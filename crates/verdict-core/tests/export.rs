mod p2b_common;

use p2b_common::*;
use verdict_core::export::{packet, sarif};
use verdict_core::*;

fn project_with_a_finding() -> (Tmp, String) {
    let t = Tmp::new("export");
    let mut l = seeded(t.path());
    std::fs::create_dir_all(t.path().join("src")).unwrap();
    std::fs::write(
        t.path().join("src/login.rs"),
        "pub fn login() {\n    todo!()\n}\n",
    )
    .unwrap();
    let pointer = EvidencePointer::file_line(t.path(), "src/login.rs", 2).unwrap();
    l.append(
        Writer::Checker,
        Some(SID),
        Record::Finding(Finding {
            id: "unfinished:stub.abc123".into(),
            severity: Severity::Blocker,
            title: "login is a stub".into(),
            claim_seq: Some(3),
            evidence: vec![pointer],
            tier: Tier::Static,
            blast: Some(Lane::Standard),
            judgment: false,
            fix_hint: Some("write the body".into()),
        }),
    )
    .unwrap();
    let sid = SID.to_string();
    (t, sid)
}

#[test]
fn sarif_is_valid_sarif_with_the_tier_and_the_ledger_record() {
    let (t, sid) = project_with_a_finding();
    let l = Ledger::open_existing(t.path()).unwrap().unwrap();
    let text = sarif(&l, Some(&sid)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["version"], "2.1.0");
    assert!(v["$schema"].as_str().unwrap().contains("sarif-2.1.0"));
    let run = &v["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "Verdict");
    let result = &run["results"][0];
    assert_eq!(result["ruleId"], "unfinished");
    assert_eq!(result["level"], "error");
    assert_eq!(result["message"]["text"], "login is a stub");
    assert_eq!(
        result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
        "src/login.rs"
    );
    assert_eq!(
        result["locations"][0]["physicalLocation"]["region"]["startLine"],
        2
    );
    assert_eq!(result["properties"]["verdictTier"], "static");
    assert_eq!(result["properties"]["verdictSeverity"], "blocker");
    assert_eq!(result["properties"]["verdictStatus"], "open");
    assert!(result["properties"]["verdictRecord"].as_u64().unwrap() > 0);
    assert_eq!(run["properties"]["verdictSession"], sid);
}

#[test]
fn the_packet_states_the_verdict_claims_findings_decisions_and_overrides() {
    let (t, sid) = project_with_a_finding();
    let mut l = Ledger::open_existing(t.path()).unwrap().unwrap();
    l.append(
        Writer::Human,
        Some(&sid),
        Record::Decision {
            decision: DecisionKind::RaiseLane,
            choice: "high".into(),
            reason: None,
            who: Writer::Human,
        },
    )
    .unwrap();
    let text = packet(t.path(), &l, &sid).unwrap();
    assert!(text.starts_with("VERDICT ACCEPTANCE PACKET"), "{text}");
    assert!(text.contains("CLAIMS"), "{text}");
    assert!(
        text.contains("login route exists in src/login.rs"),
        "the claim is quoted: {text}"
    );
    assert!(text.contains("FINDINGS"), "{text}");
    assert!(text.contains("login is a stub"), "{text}");
    assert!(text.contains("evidence: src/login.rs:2"), "{text}");
    assert!(text.contains("DECISIONS"), "{text}");
    assert!(text.contains("raise_lane: high"), "{text}");
    assert!(text.contains("OVERRIDES"), "{text}");
    assert!(text.contains("NOT ESTABLISHED"), "{text}");
    assert!(text.contains("THE REPORT AS PRINTED"), "{text}");
    assert!(text.contains("VERDICT:"), "{text}");
    // The packet never turns a FAILED verdict into a pass.
    assert!(!text.contains("VERDICT: VERIFIED"), "{text}");
}

#[test]
fn every_claim_is_classified_proven_failed_unproven_or_out_of_scope() {
    let (t, sid) = project_with_a_finding();
    let l = Ledger::open_existing(t.path()).unwrap().unwrap();
    let text = packet(t.path(), &l, &sid).unwrap();
    assert!(
        text.contains("status: failed"),
        "a blocking finding about the claim makes it failed: {text}"
    );
    // A finding about the claim is printed with its evidence and tier.
    assert!(
        text.contains("bearing finding: [blocker] login is a stub"),
        "{text}"
    );
}

#[test]
fn aggregate_checks_cannot_prove_an_individual_claim() {
    for name in ["wiring", "claims_vs_code", "exec", "firstrun", "release"] {
        let t = Tmp::new("packet-unbound-proof");
        let mut l = seeded(t.path());
        // Even a fresh successful check is not positive evidence of every claim.
        l.append(
            Writer::Core,
            Some(SID),
            Record::Check {
                name: name.into(),
                tier: Tier::Ran,
                inputs_hash: "h".into(),
                started_at: "2026-10-26T12:00:00.000Z".into(),
                finished_at: "2026-10-26T12:00:01.000Z".into(),
                outcome: CheckOutcome::Ran,
                detail: Some("unrelated test succeeded".into()),
                tree_state: baseline::tree_state(t.path()),
            },
        )
        .unwrap();
        for changed in [false, true] {
            if changed {
                std::fs::write(t.path().join("changed.rs"), "changed").unwrap();
            }
            let text = packet(t.path(), &l, SID).unwrap();
            assert!(
                text.contains("status: unproven"),
                "{name}, changed={changed}: {text}"
            );
            assert!(
                !text.contains("status: proven"),
                "{name}, changed={changed}: {text}"
            );
        }
    }
}

#[test]
fn decisions_jsonl_exports_the_humans_decisions_and_the_override_context() {
    let (t, sid) = project_with_a_finding();
    let mut l = Ledger::open_existing(t.path()).unwrap().unwrap();
    l.append(
        Writer::Human,
        Some(&sid),
        Record::Decision {
            decision: DecisionKind::RaiseLane,
            choice: "high".into(),
            reason: None,
            who: Writer::Human,
        },
    )
    .unwrap();
    l.append(
        Writer::Human,
        Some(&sid),
        Record::Decision {
            decision: DecisionKind::CommitAnyway,
            choice: verdict_core::verdict::CommitAnywayChoice {
                tree: "tree-1".into(),
                finding_ids: vec!["unfinished:stub.abc123".into()],
            }
            .encode(),
            reason: Some("hotfix".into()),
            who: Writer::Human,
        },
    )
    .unwrap();
    let text = verdict_core::export::decisions_jsonl(&l).unwrap();
    let lines: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["decision"], "raise_lane");
    assert_eq!(lines[0]["choice"], "high");
    assert_eq!(lines[0]["who"], "human");
    assert_eq!(lines[1]["decision"], "commit_anyway");
    assert_eq!(lines[1]["reason"], "hotfix");
    assert_eq!(lines[1]["tree"], "tree-1");
    assert_eq!(lines[1]["findings"][0], "unfinished:stub.abc123");
}

#[test]
fn a_release_packet_carries_release_evidence_and_asset_hashes() {
    let (t, sid) = project_with_a_finding();
    std::fs::create_dir_all(t.path().join("dist")).unwrap();
    std::fs::write(t.path().join("dist/app.zip"), b"asset bytes").unwrap();
    std::fs::write(
        t.path().join("verdict.json"),
        r#"{"version":1,"release":{"tag":"v1.0.0","version":"1.0.0",
            "assets":["dist/*.zip"],"readme":"README.md"}}"#,
    )
    .unwrap();
    let mut l = Ledger::open_existing(t.path()).unwrap().unwrap();
    for (name, tier) in [("release", Tier::Asset), ("firstrun", Tier::Browsed)] {
        l.append(
            Writer::Core,
            Some(&sid),
            Record::Check {
                name: name.into(),
                tier,
                inputs_hash: "h".into(),
                started_at: "2026-10-26T12:00:00.000Z".into(),
                finished_at: "2026-10-26T12:00:01.000Z".into(),
                outcome: CheckOutcome::Ran,
                detail: Some(format!("{name} ran")),
                tree_state: verdict_core::baseline::tree_state(t.path()),
            },
        )
        .unwrap();
    }
    l.append(
        Writer::Human,
        Some(&sid),
        Record::Decision {
            decision: DecisionKind::RaiseLane,
            choice: "release".into(),
            reason: None,
            who: Writer::Human,
        },
    )
    .unwrap();
    let text = packet(t.path(), &l, &sid).unwrap();
    assert!(text.contains("RELEASE EVIDENCE"), "{text}");
    assert!(text.contains("asset dist/app.zip"), "{text}");
    assert!(text.contains("SHA-256"), "{text}");
}
