//! The plain-text report for one session, in the design's report shape.
//!
//! This build has no checkers and no verdict computation, so the report is honest about it:
//! the verdict line says nothing was checked (never VERIFIED, never clean) and the report ends
//! with NOT ESTABLISHED.

use std::fmt::Write as _;

use crate::ledger::{Ledger, LedgerError};
use crate::model::{CheckOutcome, Record};

fn indent_block(text: &str, pad: &str) -> String {
    text.lines().collect::<Vec<_>>().join(&format!("\n{pad}"))
}

/// Render the report for `session_id`, or for the most recent session when `None`.
///
/// Returns [`LedgerError::Invalid`] when the ledger has no such session.
pub fn render_report(ledger: &Ledger, session_id: Option<&str>) -> Result<String, LedgerError> {
    let all = ledger.records()?;
    let session = match session_id {
        Some(id) => all
            .iter()
            .find(|s| matches!(&s.record, Record::Session { id: sid, .. } if sid == id)),
        None => all
            .iter()
            .rfind(|s| matches!(s.record, Record::Session { .. })),
    };
    let Some(session) = session else {
        return Err(LedgerError::Invalid(match session_id {
            Some(id) => format!("no session {id} in this ledger"),
            None => "this ledger has no sessions yet".into(),
        }));
    };
    let Record::Session {
        id,
        started_at,
        baseline,
        agent,
        model,
        vendor,
        policy_version,
        ..
    } = &session.record
    else {
        unreachable!("selected a session record");
    };
    let mine: Vec<_> = all
        .iter()
        .filter(|s| s.session_id.as_deref() == Some(id.as_str()))
        .collect();
    let integrity = ledger.verify()?;

    let mut prompts = Vec::new();
    let mut claims = Vec::new();
    let mut tools = Vec::new();
    let mut unavailable = Vec::new();
    let mut refused = Vec::new();
    let mut findings = Vec::new();
    let mut hypotheses = Vec::new();
    let mut decisions = Vec::new();
    let mut checks = Vec::new();
    for s in &mine {
        match &s.record {
            Record::Prompt { .. } => prompts.push(*s),
            Record::Claim { .. } => claims.push(*s),
            Record::ToolCall { .. } => tools.push(*s),
            Record::CheckUnavailable { .. } => unavailable.push(*s),
            Record::RefusedWrite { .. } => refused.push(*s),
            Record::Finding(_) => findings.push(*s),
            Record::Hypothesis(_) => hypotheses.push(*s),
            Record::Decision { .. } => decisions.push(*s),
            Record::Check { .. } => checks.push(*s),
            _ => {}
        }
    }

    let mut o = String::new();
    let _ = writeln!(
        o,
        "VERDICT: UNVERIFIED - nothing was checked  (lane: not classified, policy v{policy_version}, builder: {agent}/{model} [{vendor}], verifier: none)"
    );
    let _ = writeln!(
        o,
        "read from code: not run  |  your build and tests: not run  |  your real app: not run"
    );
    let ledger_line = if integrity.is_ok() {
        format!("intact, {} records", integrity.records)
    } else {
        match integrity.first_broken_seq() {
            Some(n) => format!("BROKEN at record {n}"),
            None => "BROKEN".to_string(),
        }
    };
    let _ = writeln!(
        o,
        "session {id}, started {started_at}  |  ledger: {ledger_line}  |  baseline {}{}",
        &baseline.hash[..baseline.hash.len().min(12)],
        if baseline.is_git_repo {
            ""
        } else {
            " (not a git repository)"
        }
    );

    let _ = writeln!(o, "\nFIX FIRST");
    if findings.is_empty() {
        let _ = writeln!(
            o,
            "  (nothing was checked, so nothing is proven wrong and nothing is proven right)"
        );
    } else {
        for (i, s) in findings.iter().enumerate() {
            if let Record::Finding(f) = &s.record {
                let _ = writeln!(
                    o,
                    "  {}. [{:?}] {} ({}, record {}; verdict computation is not in this build, so not weighed)",
                    i + 1,
                    f.severity,
                    f.title,
                    f.tier.as_str(),
                    s.seq
                );
            }
        }
    }

    let _ = writeln!(o, "\nYOURS TO SETTLE");
    let mut n = 0;
    if !integrity.is_ok() {
        for p in &integrity.problems {
            n += 1;
            let _ = writeln!(o, "  {n}. ledger integrity: {}", p.message);
        }
    }
    for s in &unavailable {
        if let Record::CheckUnavailable { component, reason } = &s.record {
            n += 1;
            let _ = writeln!(
                o,
                "  {n}. check unavailable: {component}: {} (record {})",
                indent_block(reason, "     "),
                s.seq
            );
        }
    }
    for s in &refused {
        if let Record::RefusedWrite {
            attempted_by,
            attempted_kind,
            attempted_decision,
            reason,
            ..
        } = &s.record
        {
            n += 1;
            let what = match attempted_decision {
                Some(d) => format!("{attempted_kind} ({})", d.as_str()),
                None => attempted_kind.to_string(),
            };
            let _ = writeln!(
                o,
                "  {n}. refused write: {attempted_by} tried to write a {what}: {reason} (record {})",
                s.seq
            );
        }
    }
    for s in &hypotheses {
        if let Record::Hypothesis(f) = &s.record {
            n += 1;
            let _ = writeln!(
                o,
                "  {n}. hypothesis (no evidence pointer, counts for nothing): {} (record {})",
                f.title, s.seq
            );
        }
    }
    if n == 0 {
        let _ = writeln!(o, "  (nothing recorded for you to settle)");
    }

    if !decisions.is_empty() {
        let _ = writeln!(o, "\nDECISIONS BY THE HUMAN");
        for s in &decisions {
            if let Record::Decision {
                decision,
                choice,
                reason,
                ..
            } = &s.record
            {
                let _ = writeln!(
                    o,
                    "  record {}: {} = {}{}",
                    s.seq,
                    decision.as_str(),
                    choice,
                    reason
                        .as_ref()
                        .map(|r| format!("  (reason: {r})"))
                        .unwrap_or_default()
                );
            }
        }
    }

    let _ = writeln!(o, "\nPROMPTS ({})", prompts.len());
    if prompts.is_empty() {
        let _ = writeln!(o, "  (none recorded)");
    }
    for s in &prompts {
        if let Record::Prompt { ordinal, text, .. } = &s.record {
            let _ = writeln!(o, "  {ordinal}. {}", indent_block(text, "     "));
        }
    }

    let _ = writeln!(o, "\nCLAIMS ({})", claims.len());
    if claims.is_empty() {
        let _ = writeln!(o, "  (none recorded)");
    }
    for (i, s) in claims.iter().enumerate() {
        if let Record::Claim {
            source,
            text,
            names,
            ..
        } = &s.record
        {
            let names = if names.is_empty() {
                String::new()
            } else {
                format!("  [names: {}]", names.join(", "))
            };
            let _ = writeln!(
                o,
                "  {}. ({}) {}{names}  - not checked",
                i + 1,
                source.as_str(),
                indent_block(text, "     ")
            );
        }
    }

    let _ = writeln!(o, "\nTOOL CALLS ({})", tools.len());
    if tools.is_empty() {
        let _ = writeln!(o, "  (none recorded)");
    }
    for (i, s) in tools.iter().enumerate() {
        if let Record::ToolCall {
            tool,
            target,
            output_hash,
            duration_ms,
        } = &s.record
        {
            let _ = writeln!(
                o,
                "  {}. {tool} {target}  (output {}, {duration_ms} ms)",
                i + 1,
                &output_hash[..output_hash.len().min(12)]
            );
        }
    }

    let _ = writeln!(o, "\nCHECKS");
    let ran = checks
        .iter()
        .filter(|s| {
            matches!(
                &s.record,
                Record::Check {
                    outcome: CheckOutcome::Ran,
                    ..
                }
            )
        })
        .count();
    if checks.is_empty() {
        let _ = writeln!(
            o,
            "  none ran. This build has no checkers yet; nothing above was compared with the code."
        );
    } else {
        for s in &checks {
            if let Record::Check {
                name,
                tier,
                outcome,
                detail,
                ..
            } = &s.record
            {
                let _ = writeln!(
                    o,
                    "  {name}: {outcome:?}, tier {}{} (record {})",
                    tier.as_str(),
                    detail
                        .as_ref()
                        .map(|d| format!(": {d}"))
                        .unwrap_or_default(),
                    s.seq
                );
            }
        }
        let _ = writeln!(
            o,
            "  {ran} of {} checks ran; verdict computation is not in this build, so none is weighed.",
            checks.len()
        );
    }

    let _ = writeln!(o, "\nNOT ESTABLISHED");
    let _ = writeln!(
        o,
        "  - whether the work does what the {} prompt(s) asked: no checker has run",
        prompts.len()
    );
    let _ = writeln!(
        o,
        "  - whether any of the {} claim(s) is true: claims are the agent's words, not evidence",
        claims.len()
    );
    let _ = writeln!(
        o,
        "  - whether your build and tests pass, or your real app works: nothing was run"
    );
    for s in &unavailable {
        if let Record::CheckUnavailable { component, .. } = &s.record {
            let _ = writeln!(o, "  - {component}: could not run (record {})", s.seq);
        }
    }
    if !integrity.is_ok() {
        let _ = writeln!(
            o,
            "  - whether this ledger can be trusted: it failed verification"
        );
    }
    Ok(o)
}
