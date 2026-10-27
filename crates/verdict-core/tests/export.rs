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
