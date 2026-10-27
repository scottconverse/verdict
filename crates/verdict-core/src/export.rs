//! Exports: SARIF for an editor or CI, and the acceptance packet for a release.
//!
//! Both are pure renderings of the ledger. Neither writes, neither decides: the findings, their
//! tiers and their record numbers are exactly what the checkers and desks recorded.

use std::path::Path;

use serde_json::{Value, json};

use crate::lane::lane_name;
use crate::ledger::{Ledger, LedgerError};
use crate::model::{Record, Severity, Tier};
use crate::pointer::EvidencePointer;
use crate::report::render_report_with;
use crate::verdict::{Computed, FindingStatus, analyze, compute_live};

/// SARIF level for a severity.
fn sarif_level(s: Severity) -> &'static str {
    match s {
        Severity::Blocker | Severity::Critical => "error",
        Severity::Major | Severity::Minor => "warning",
        Severity::Nit => "note",
    }
}

fn status_name(s: &FindingStatus) -> &'static str {
    match s {
        FindingStatus::Open => "open",
        FindingStatus::AcceptedNotReal { .. } => "accepted_not_real",
        FindingStatus::Resolved { .. } => "resolved",
    }
}

/// The SARIF 2.1.0 document for one session's findings.
///
/// A finding with a `file:line` pointer becomes a result with a physical location; a finding whose
/// only pointers are command outputs still appears, with the pointers in its properties. Every
/// result carries the proof tier and the ledger record number.
pub fn sarif(ledger: &Ledger, session: Option<&str>) -> Result<String, LedgerError> {
    let a = analyze(
        ledger,
        session.ok_or_else(|| LedgerError::Invalid("export --sarif needs a session".into()))?,
    )?;
    let mut rules: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut results: Vec<Value> = Vec::new();
    for f in &a.findings {
        let rule_id = f
            .checker()
            .map(str::to_string)
            .unwrap_or_else(|| "desk".to_string());
        if !seen.contains(&rule_id) {
            seen.push(rule_id.clone());
            rules.push(json!({
                "id": rule_id,
                "name": rule_id,
                "shortDescription": { "text": format!("{rule_id}: what Verdict's checkers record") },
                "properties": { "tags": ["verdict"] }
            }));
        }
        let mut locations = Vec::new();
        let mut other: Vec<String> = Vec::new();
        for e in &f.finding.evidence {
            match e {
                EvidencePointer::FileLine { path, line, .. } => locations.push(json!({
                    "physicalLocation": {
                        "artifactLocation": { "uri": path },
                        "region": { "startLine": line }
                    }
                })),
                other_pointer => other.push(other_pointer.describe()),
            }
        }
        let mut properties = json!({
            "verdictFindingId": f.finding.id,
            "verdictRecord": f.seq,
            "verdictTier": f.finding.tier.as_str(),
            "verdictSeverity": severity_name(f.finding.severity),
            "verdictStatus": status_name(&f.status),
            "judgment": f.finding.judgment,
        });
        if !other.is_empty() {
            properties["verdictEvidence"] = json!(other);
        }
        if let Some(hint) = &f.finding.fix_hint {
            properties["verdictFixHint"] = json!(hint);
        }
        results.push(json!({
            "ruleId": rule_id,
            "level": sarif_level(f.finding.severity),
            "message": { "text": f.finding.title },
            "locations": locations,
            "partialFingerprints": { "verdictFindingId": f.finding.id },
            "properties": properties
        }));
    }
    let doc = json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "Verdict",
                    "informationUri": "https://github.com/scottconverse/verdict",
                    "semanticVersion": env!("CARGO_PKG_VERSION"),
                    "rules": rules
                }
            },
            "results": results,
            "properties": {
                "verdictSession": a.session,
                "verdictHypotheses": a.hypotheses.len(),
                "verdictLedgerIntact": ledger.verify()?.is_ok(),
            }
        }]
    });
    serde_json::to_string_pretty(&doc).map_err(|e| LedgerError::Invalid(e.to_string()))
}

fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Blocker => "blocker",
        Severity::Critical => "critical",
        Severity::Major => "major",
        Severity::Minor => "minor",
        Severity::Nit => "nit",
    }
}

/// The acceptance packet for `session`: every claim as proven / failed / unproven / out of scope,
/// the findings with their evidence, the decisions taken, and every commit-anyway override printed
/// beside the FAILED verdict it went past.
pub fn packet(root: &Path, ledger: &Ledger, session: &str) -> Result<String, LedgerError> {
    let live = compute_live(root, ledger, session, None)?;
    let report = render_report_with(ledger, Some(session), None)?;
    let mut out = String::new();
    out.push_str("VERDICT ACCEPTANCE PACKET\n");
    out.push_str("=========================\n\n");
    out.push_str(&format!(
        "Session: {session}\nLane: {} (policy v{})\nBuilder: {}\nVerdict: {}\n",
        lane_name(live.lane),
        live.policy_version,
        builder_text(ledger, session),
        live.label()
    ));
    out.push_str(&format!(
        "Ledger: {}\n\n",
        match ledger.verify() {
            Ok(v) if v.is_ok() => "intact".to_string(),
            Ok(v) => format!("BROKEN: {}", v.problems.len()),
            Err(e) => format!("could not be verified: {e}"),
        }
    ));

    out.push_str("CLAIMS\n------\n");
    let records = ledger.records_for_session(session)?;
    let mut claims = 0;
    for s in &records {
        if let Record::Claim {
            source,
            text,
            names,
            ..
        } = &s.record
        {
            claims += 1;
            out.push_str(&format!(
                "{claims}. [{}] {}\n   names: {}\n   record {}\n",
                source.as_str(),
                text,
                if names.is_empty() {
                    "nothing".into()
                } else {
                    names.join(", ")
                },
                s.seq
            ));
        }
    }
    if claims == 0 {
        out.push_str("(none)\n");
    }
    out.push('\n');

    out.push_str("FINDINGS\n--------\n");
    let a = analyze(ledger, session)?;
    if a.findings.is_empty() {
        out.push_str("(none)\n");
    }
    for f in &a.findings {
        out.push_str(&format!(
            "- [{}] {} ({}, record {}, {})\n",
            severity_name(f.finding.severity),
            f.finding.title,
            f.finding.tier.as_str(),
            f.seq,
            status_name(&f.status)
        ));
        for e in &f.finding.evidence {
            out.push_str(&format!("    evidence: {}\n", e.describe()));
        }
    }
    out.push_str("\nHYPOTHESES (move nothing)\n------------------------\n");
    if a.hypotheses.is_empty() {
        out.push_str("(none)\n");
    }
    for (seq, h) in &a.hypotheses {
        out.push_str(&format!(
            "- [{}] {} (record {seq})\n",
            severity_name(h.severity),
            h.title
        ));
    }
    out.push_str("\nDECISIONS\n---------\n");
    let mut decisions = 0;
    for s in &records {
        if let Record::Decision {
            decision,
            choice,
            reason,
            ..
        } = &s.record
        {
            decisions += 1;
            out.push_str(&format!(
                "- {}: {}{} (record {})\n",
                decision.as_str(),
                choice,
                reason
                    .as_deref()
                    .map(|r| format!("; reason: {r}"))
                    .unwrap_or_default(),
                s.seq
            ));
        }
    }
    if decisions == 0 {
        out.push_str("(none)\n");
    }
    out.push_str("\nOVERRIDES\n---------\n");
    if a.overrides.is_empty() {
        out.push_str("(none)\n");
    }
    for ov in &a.overrides {
        out.push_str(&format!(
            "- commit-anyway record {}: reason {:?}; findings {}; staged tree {}{}\n",
            ov.seq,
            ov.reason,
            if ov.finding_ids.is_empty() {
                "none".into()
            } else {
                ov.finding_ids.join(", ")
            },
            ov.tree,
            if ov.open {
                " (STILL OPEN)"
            } else {
                " (resolved)"
            }
        ));
    }
    out.push_str("\nNOT ESTABLISHED\n---------------\n");
    if live.not_established.is_empty() && live.lane_reasons.is_empty() {
        out.push_str("(none recorded)\n");
    }
    for r in &live.lane_reasons {
        out.push_str(&format!("- lane: {r}\n"));
    }
    for r in &live.not_established {
        out.push_str(&format!("- {r}\n"));
    }
    for (seq, component, reason) in &a.plumbing_unavailable {
        out.push_str(&format!("- {component}: {reason} (record {seq})\n"));
    }
    out.push_str("\nTHE REPORT AS PRINTED\n---------------------\n");
    out.push_str(&report);
    Ok(out)
}

fn builder_text(ledger: &Ledger, session: &str) -> String {
    for s in ledger.records_for_session(session).unwrap_or_default() {
        if let Record::Session {
            agent,
            model,
            vendor,
            ..
        } = &s.record
        {
            return format!("{agent}/{model} [{vendor}]");
        }
    }
    "unknown".to_string()
}

/// The computed verdict, for callers that also want the word beside the packet.
pub fn packet_with(
    root: &Path,
    ledger: &Ledger,
    session: &str,
) -> Result<(String, Computed), LedgerError> {
    let c = compute_live(root, ledger, session, None)?;
    let text = packet(root, ledger, session)?;
    Ok((text, c))
}

/// The proof tier names, for a caller that prints a legend.
pub fn tier_names() -> Vec<&'static str> {
    [
        Tier::Static,
        Tier::Ran,
        Tier::Browsed,
        Tier::Asset,
        Tier::Judgment,
        Tier::HonestRed,
        Tier::Unverified,
    ]
    .iter()
    .map(|t| t.as_str())
    .collect()
}
