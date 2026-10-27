//! Condition 1 of commit anyway: it is human only. Every route an agent has through the MCP
//! server's tool surface is refused and recorded as a refused write, and none of them can change a
//! verdict, a mode or a finding.

use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::*;
use verdict_mcp::{FORBIDDEN_TOOL_NAMES, TOOL_NAMES, handle_tool, tool_definitions};

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn project() -> (Tmp, Tmp, String) {
    let dir = std::env::temp_dir().join(format!(
        "vdt-mcp-ca-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut l = Ledger::open(&dir).unwrap();
    let s = start_session(
        &mut l,
        &dir,
        &SessionParams {
            agent: "claude".into(),
            model: "m".into(),
            vendor: "anthropic".into(),
            home: Some(home.clone()),
        },
    )
    .unwrap();
    // A FAILED verdict: a checker found something, so there is something to "commit anyway" past.
    l.append(
        Writer::Checker,
        Some(&s.id),
        Record::Finding(Finding {
            id: "wiring:src/a.rs:1".into(),
            severity: Severity::Major,
            title: "broken".into(),
            claim_seq: None,
            evidence: vec![EvidencePointer::command("cargo test", b"red")],
            tier: Tier::Static,
            blast: None,
            judgment: false,
            fix_hint: None,
        }),
    )
    .unwrap();
    (Tmp(dir), Tmp(home), s.id)
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn decisions(dir: &std::path::Path) -> usize {
    Ledger::open_existing(dir)
        .unwrap()
        .unwrap()
        .records()
        .unwrap()
        .iter()
        .filter(|s| matches!(s.record, Record::Decision { .. }))
        .count()
}

#[test]
fn an_agent_calling_commit_anyway_through_mcp_is_refused_and_recorded() {
    let (t, _h, sid) = project();
    let out = handle_tool(
        &t.0,
        "verdict_commit_anyway",
        &args(json!({"reason": "the agent says so", "session": sid})),
    )
    .expect("the server answers the call");
    assert!(out.is_error, "{}", out.text);
    assert!(out.text.contains("REFUSED"), "{}", out.text);
    assert!(out.text.contains("human-only"), "{}", out.text);
    assert!(out.text.contains("refused_write"), "{}", out.text);

    let l = Ledger::open_existing(&t.0).unwrap().unwrap();
    let refused: Vec<_> = l
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::RefusedWrite {
                attempted_by,
                attempted_kind,
                attempted_decision,
                ..
            } => Some((s.writer, attempted_by, attempted_kind, attempted_decision)),
            _ => None,
        })
        .collect();
    assert_eq!(
        refused,
        vec![(
            Writer::Core,
            Writer::Agent,
            RecordKind::Decision,
            Some(DecisionKind::CommitAnyway)
        )]
    );
    assert_eq!(decisions(&t.0), 0, "no decision was written");
    assert!(l.verify().unwrap().is_ok());
}

#[test]
fn nothing_an_agent_can_call_changes_the_verdict_or_opens_an_override() {
    let (t, _h, sid) = project();
    for name in FORBIDDEN_TOOL_NAMES {
        let out = handle_tool(
            &t.0,
            name,
            &args(json!({"session": sid, "reason": "x", "choice": "enforce"})),
        )
        .unwrap_or_else(|| panic!("{name} is not recognized"));
        assert!(out.is_error, "{name}: {}", out.text);
        assert!(out.text.contains("REFUSED"), "{name}: {}", out.text);
    }
    assert_eq!(decisions(&t.0), 0);
    // Even after all of that, the computed verdict is FAILED: the finding is open, no override exists.
    let l = Ledger::open_existing(&t.0).unwrap().unwrap();
    let report =
        verdict_core::verdict::report_for_project(&t.0, &l, Some(&sid), Some(&_h.0)).unwrap();
    assert!(report.starts_with("VERDICT: FAILED"), "{report}");
    assert!(!report.contains("COMMIT ANYWAY"));
    // The mode is still shadow.
    assert_eq!(verdict_core::part1c::current_mode(&l).unwrap(), "shadow");
}

#[test]
fn no_advertised_tool_can_commit_anyway_and_verdict_check_returns_the_computed_word() {
    for tool in tool_definitions() {
        let n = tool.name.to_string();
        assert!(TOOL_NAMES.contains(&n.as_str()));
        assert!(
            !n.contains("commit_anyway") && !n.contains("decide") && !n.contains("mode"),
            "{n}"
        );
    }
    let (t, _h, sid) = project();
    let out = handle_tool(&t.0, "verdict_check", &args(json!({"session": sid}))).unwrap();
    assert!(!out.is_error, "{}", out.text);
    // The agent asked; the core answered with the computed word, and wrote it as the core.
    assert!(out.text.contains("the verdict is "), "{}", out.text);
    assert!(out.text.contains("VERDICT: "), "{}", out.text);
    let l = Ledger::open_existing(&t.0).unwrap().unwrap();
    let verdicts: Vec<_> = l
        .records()
        .unwrap()
        .into_iter()
        .filter(|s| matches!(s.record, Record::Verdict { .. }))
        .collect();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].writer, Writer::Core);
    // Asking again with nothing changed writes nothing new.
    handle_tool(&t.0, "verdict_check", &args(json!({"session": sid}))).unwrap();
    let again = Ledger::open_existing(&t.0)
        .unwrap()
        .unwrap()
        .records()
        .unwrap();
    assert_eq!(
        again
            .iter()
            .filter(|s| matches!(s.record, Record::Verdict { .. }))
            .count(),
        1
    );
}

#[test]
fn a_hook_adapter_cannot_commit_anyway_either() {
    // The adapters write as `adapter`; the ledger refuses a decision from anyone but the human.
    let (t, _h, sid) = project();
    let mut l = Ledger::open(&t.0).unwrap();
    for w in [
        Writer::Adapter,
        Writer::Agent,
        Writer::Checker,
        Writer::Desk,
        Writer::Core,
    ] {
        let r = l.append(
            w,
            Some(&sid),
            Record::Decision {
                decision: DecisionKind::CommitAnyway,
                choice: "{}".into(),
                reason: Some("a hook says so".into()),
                who: Writer::Human,
            },
        );
        assert!(matches!(r, Err(LedgerError::Refused { .. })), "{w}");
    }
    assert_eq!(decisions(&t.0), 0);
}
