//! Exports: SARIF for an editor or CI, and the acceptance packet for a release.
//!
//! Both are pure renderings of the ledger. Neither writes, neither decides: the findings, their
//! tiers and their record numbers are exactly what the checkers and desks recorded.

use std::path::Path;

use serde_json::{Value, json};

use crate::lane::lane_name;
use crate::ledger::{Ledger, LedgerError};
use crate::model::{Lane, Record, Severity, Tier};
use crate::pointer::EvidencePointer;
use crate::report::render_report_with;
use crate::verdict::{Computed, FindingStatus, analyze, compute_live, word_label};

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
                "verdictOverrides": a.overrides.iter().map(|ov| json!({
                    "record": ov.seq,
                    "tree": ov.tree,
                    "reason": ov.reason,
                    "findingIds": ov.finding_ids,
                    "open": ov.open,
                })).collect::<Vec<_>>(),
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
    packet_with_lane(root, ledger, session, None)
}

/// [`packet`] computed in a lane the caller names (the human's release-lane run), never lower
/// than the lane the diff implies.
pub fn packet_with_lane(
    root: &Path,
    ledger: &Ledger,
    session: &str,
    lane: Option<Lane>,
) -> Result<String, LedgerError> {
    let mut live_input = super::verdict::live_input(root, ledger, session, None)?;
    if let Some(chosen) = lane
        && chosen > live_input.input.implied_lane
    {
        live_input.input.lane_reasons.insert(
            0,
            format!(
                "you chose the {} lane for this packet",
                crate::lane::lane_name(chosen)
            ),
        );
        live_input.input.implied_lane = chosen;
    }
    let live = super::verdict::compute(ledger, session, &live_input.input)?;
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
    let a = analyze(ledger, session)?;
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
            let (status, why) = classify_claim(s.seq, names, &a, &live);
            out.push_str(&format!(
                "{claims}. [{}] {}\n   status: {status}{}\n   names: {}\n   record {}\n",
                source.as_str(),
                text,
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                },
                if names.is_empty() {
                    "nothing".into()
                } else {
                    names.join(", ")
                },
                s.seq
            ));
            for f in a
                .findings
                .iter()
                .filter(|f| f.finding.claim_seq == Some(s.seq))
            {
                out.push_str(&format!(
                    "   bearing finding: [{}] {} ({}, record {}, {})\n",
                    severity_name(f.finding.severity),
                    f.finding.title,
                    f.finding.tier.as_str(),
                    f.seq,
                    status_name(&f.status)
                ));
                for e in &f.finding.evidence {
                    out.push_str(&format!("      evidence: {}\n", e.describe()));
                }
            }
        }
    }
    if claims == 0 {
        out.push_str("(none)\n");
    }
    out.push('\n');

    out.push_str("FINDINGS\n--------\n");
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
    if live.lane == Lane::Release {
        out.push_str(&release_evidence(root, ledger, session, &live, &a));
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

/// How a claim stands, for the acceptance document.
fn classify_claim(
    seq: u64,
    names: &[String],
    a: &crate::verdict::Analysis,
    live: &Computed,
) -> (&'static str, String) {
    if names.is_empty() {
        return (
            "out of scope",
            "it names no URL, file or symbol, so no check can look it up".into(),
        );
    }
    let bearing: Vec<_> = a
        .findings
        .iter()
        .filter(|f| f.finding.claim_seq == Some(seq))
        .collect();
    if bearing
        .iter()
        .any(|f| f.is_open() && crate::verdict::is_blocking(f, live.lane))
    {
        return ("failed", "a blocking finding bears on it".into());
    }
    if bearing.iter().any(|f| f.is_open()) {
        return (
            "unproven",
            "an open finding bears on it; nothing proves it and nothing proves it wrong".into(),
        );
    }
    // CheckState is aggregate evidence: it has no positive assertion binding this claim's
    // meaning to the result. A clean build, file lookup or missing contradiction is not proof
    // of every claim, even on the current tree. Keep those receipts in the packet, but never
    // promote their mere existence (or stale history) to individual claim verification.
    (
        "unproven",
        "no positive evidence establishes this individual claim; aggregate check results and absence of findings are not proof".into(),
    )
}

/// Release-level evidence: the release and firstrun check records, the version, and every
/// configured asset's SHA-256. This is what a release packet needs beyond one session's checks.
fn release_evidence(
    root: &Path,
    ledger: &Ledger,
    session: &str,
    live: &Computed,
    a: &crate::verdict::Analysis,
) -> String {
    let mut out = String::from("\nRELEASE EVIDENCE\n----------------\n");
    for name in ["release", "firstrun"] {
        match a.checks.get(name) {
            Some(crate::verdict::CheckState::Ran { tier, seq, tree: _ }) => {
                let detail = ledger
                    .records_for_session(session)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|s| s.seq == *seq)
                    .and_then(|s| match s.record {
                        Record::Check { detail, .. } => detail,
                        _ => None,
                    })
                    .unwrap_or_default();
                out.push_str(&format!(
                    "- {name}: ran at the {} tier (record {seq}){}\n",
                    tier.as_str(),
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", one_line_detail(&detail))
                    }
                ));
            }
            Some(crate::verdict::CheckState::Skipped { detail, .. }) => {
                out.push_str(&format!("- {name}: skipped by policy ({detail})\n"));
            }
            Some(crate::verdict::CheckState::Unavailable { reason, .. }) => {
                out.push_str(&format!("- {name}: could not run ({reason})\n"));
            }
            None => out.push_str(&format!("- {name}: no record\n")),
        }
    }
    let firstrun_ok = matches!(
        a.checks.get("firstrun"),
        Some(crate::verdict::CheckState::Ran {
            tier: Tier::Browsed,
            ..
        })
    );
    out.push_str(&format!(
        "- firstrun at the browsed tier: {}\n- release coherence ran: {}\n- open commit-anyway override: {}\n",
        if firstrun_ok { "yes" } else { "no" },
        matches!(
            a.checks.get("release"),
            Some(crate::verdict::CheckState::Ran { .. })
        ),
        if live.override_open { "yes" } else { "no" }
    ));
    if let Ok((policy, _)) = crate::verdict::load_policy(root, None) {
        if let Some(version) = policy
            .value
            .pointer("/release/version")
            .and_then(Value::as_str)
        {
            out.push_str(&format!("- release version: {version}\n"));
        }
        let assets: Vec<String> = policy
            .value
            .pointer("/release/assets")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        for pattern in assets {
            let files = expand_asset(root, &pattern);
            if files.is_empty() {
                out.push_str(&format!("- asset {pattern}: (no file matched)\n"));
                continue;
            }
            for rel in files {
                match std::fs::read(root.join(&rel)) {
                    Ok(bytes) => out.push_str(&format!(
                        "- asset {rel}: {} bytes, SHA-256 {}\n",
                        bytes.len(),
                        crate::sha256::hex(&bytes)
                    )),
                    Err(e) => out.push_str(&format!("- asset {rel}: cannot be read ({e})\n")),
                }
            }
        }
    }
    out
}

fn one_line_detail(text: &str) -> String {
    let t = text.replace(['\n', '\r'], " ");
    if t.len() > 400 {
        let mut cut = t;
        cut.truncate(400);
        cut.push_str("...");
        cut
    } else {
        t
    }
}

/// `decisions.jsonl`: the project's human decisions as JSON lines, for cross-machine continuity.
/// The ledger itself is never committed; this export is the part a project may track in git.
pub fn decisions_jsonl(ledger: &Ledger) -> Result<String, LedgerError> {
    let all = ledger.records()?;
    let mut out = String::new();
    for (i, s) in all.iter().enumerate() {
        let Record::Decision {
            decision,
            choice,
            reason,
            who: crate::model::Writer::Human,
        } = &s.record
        else {
            continue;
        };
        // The verdict in force when the human decided, for the overrides that went past FAILED.
        let verdict_at = all[..i].iter().rev().find_map(|p| match &p.record {
            Record::Verdict {
                word, highest_tier, ..
            } => Some(word_label(*word, *highest_tier)),
            _ => None,
        });
        let mut extra = serde_json::Map::new();
        if *decision == crate::model::DecisionKind::CommitAnyway {
            let c = crate::verdict::CommitAnywayChoice::decode(choice);
            extra.insert("tree".into(), Value::String(c.tree));
            extra.insert("findings".into(), json!(c.finding_ids));
        }
        let mut line = json!({
            "seq": s.seq,
            "time": s.ts,
            "session": s.session_id,
            "decision": decision.as_str(),
            "choice": choice,
            "reason": reason,
            "who": "human",
            "verdict_at_decision": verdict_at,
        });
        if let Some(obj) = line.as_object_mut() {
            for (k, v) in extra {
                obj.insert(k, v);
            }
        }
        out.push_str(&serde_json::to_string(&line).unwrap_or_default());
        out.push('\n');
    }
    Ok(out)
}

/// Every file a configured release asset names: an exact path, a folder, or a simple glob
/// (`dist/*.zip`). Used for the packet's asset hashes, so a pattern is not printed unverified.
fn expand_asset(root: &Path, pattern: &str) -> Vec<String> {
    let pattern = pattern.trim_start_matches("./").replace('\\', "/");
    if !pattern.contains('*') && !pattern.contains('?') {
        if pattern.ends_with('/') {
            let Ok(entries) = std::fs::read_dir(root.join(pattern.trim_end_matches('/'))) else {
                return Vec::new();
            };
            return entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| {
                    e.path()
                        .strip_prefix(root)
                        .unwrap_or(&e.path())
                        .to_string_lossy()
                        .replace('\\', "/")
                })
                .collect();
        }
        return root
            .join(&pattern)
            .exists()
            .then_some(pattern)
            .into_iter()
            .collect();
    }
    let literal = pattern.split(['*', '?']).next().unwrap_or("");
    let dir = literal.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let Ok(entries) = std::fs::read_dir(if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    }) else {
        return Vec::new();
    };
    let Ok(glob) = globset::GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| {
            e.path()
                .strip_prefix(root)
                .unwrap_or(&e.path())
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|rel| matcher.is_match(rel))
        .collect()
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
