//! The plain-text report for one session, in the design's report shape.
//!
//! The headline is the verdict. [`render_report`] shows the session's latest verdict record (the
//! one `vdt verdict` or the gate wrote); [`render_report_with`] shows a verdict computed just now.
//! A session with no verdict at all reads honestly: the verdict line says nothing was checked
//! (never VERIFIED, never clean). Either way the report ends with NOT ESTABLISHED.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::lane::lane_name;
use crate::ledger::{Ledger, LedgerError};
use crate::model::{CheckOutcome, Lane, Record};
use crate::report_head::{Head, three_state};
use crate::verdict::{CheckState, Computed, FindingStatus, analyze, sections};

fn indent_block(text: &str, pad: &str) -> String {
    text.lines().collect::<Vec<_>>().join(&format!("\n{pad}"))
}

/// The desk a finding id names, if it came from one (`desk:<name>.<hash>`).
fn desk_name(id: &str) -> Option<&str> {
    id.strip_prefix("desk:")?.split('.').next()
}

/// A desk is capped at five findings in the report body; the rest stay in the ledger and are
/// counted on one line.
fn cap_desks(
    items: &[crate::verdict::FixItem],
) -> (Vec<&crate::verdict::FixItem>, BTreeMap<String, usize>) {
    const CAP: usize = 5;
    let mut shown: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden: BTreeMap<String, usize> = BTreeMap::new();
    let mut visible = Vec::new();
    for f in items {
        match desk_name(&f.id) {
            Some(d) => {
                let n = shown.entry(d.to_string()).or_default();
                if *n < CAP {
                    *n += 1;
                    visible.push(f);
                } else {
                    *hidden.entry(d.to_string()).or_default() += 1;
                }
            }
            None => visible.push(f),
        }
    }
    (visible, hidden)
}

/// Render the report for `session_id`, or for the most recent session when `None`, with the
/// session's latest stored verdict (or none, if no verdict was ever written).
///
/// Returns [`LedgerError::Invalid`] when the ledger has no such session.
pub fn render_report(ledger: &Ledger, session_id: Option<&str>) -> Result<String, LedgerError> {
    render_report_with(ledger, session_id, None)
}

/// Like [`render_report`], but with a verdict computed just now (`Some`) instead of the stored one.
pub fn render_report_with(
    ledger: &Ledger,
    session_id: Option<&str>,
    computed: Option<&Computed>,
) -> Result<String, LedgerError> {
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
    // Each component that could not run is listed once: its latest record in the session. A later
    // successful check supersedes an earlier `check_unavailable`; otherwise re-running a checker
    // after fixing the cause would leave the old failure in the report for ever.
    let latest_ran: std::collections::BTreeMap<&str, u64> = mine
        .iter()
        .filter_map(|s| match &s.record {
            Record::Check {
                name,
                outcome: crate::model::CheckOutcome::Ran,
                ..
            } => Some((name.as_str(), s.seq)),
            _ => None,
        })
        .collect();
    let latest_unavailable: Vec<&crate::ledger::Stored> = {
        let mut by_component: std::collections::BTreeMap<&str, &crate::ledger::Stored> =
            std::collections::BTreeMap::new();
        for s in &mine {
            if let Record::CheckUnavailable { component, .. } = &s.record {
                if latest_ran
                    .get(component.as_str())
                    .is_some_and(|seq| *seq > s.seq)
                {
                    continue;
                }
                by_component.insert(component.as_str(), *s);
            }
        }
        let mut v: Vec<_> = by_component.into_values().collect();
        v.sort_by_key(|s| s.seq);
        v
    };

    let mut prompts = Vec::new();
    let mut claims = Vec::new();
    let mut tools = Vec::new();
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
            Record::RefusedWrite { .. } => refused.push(*s),
            Record::Finding(_) => findings.push(*s),
            Record::Hypothesis(_) => hypotheses.push(*s),
            Record::Decision { .. } => decisions.push(*s),
            Record::Check { .. } => checks.push(*s),
            _ => {}
        }
    }

    let head: Option<Head> = match computed {
        Some(c) if c.session == *id => Some(Head::from_computed(c)),
        _ => Head::from_stored(ledger, id)?,
    };
    let analysis = match &head {
        Some(_) => Some(analyze(ledger, id)?),
        None => None,
    };
    let lane_for_sections = head.as_ref().and_then(|h| h.lane).unwrap_or(Lane::Standard);

    let mut o = String::new();
    match (&head, &analysis) {
        (Some(h), Some(a)) => {
            let _ = writeln!(
                o,
                "VERDICT: {}  (lane: {}, policy v{}, builder: {agent}/{model} [{vendor}], verifier: none)",
                h.headline_word(),
                h.lane.map_or("not recorded", lane_name),
                h.policy_version
            );
            let ts = three_state(a, lane_for_sections);
            let _ = writeln!(
                o,
                "read from code: {}  |  your build and tests: {}  |  your real app: {}",
                ts[0], ts[1], ts[2]
            );
        }
        _ => {
            let _ = writeln!(
                o,
                "VERDICT: UNVERIFIED - nothing was checked  (lane: not classified, policy v{policy_version}, builder: {agent}/{model} [{vendor}], verifier: none)"
            );
            let _ = writeln!(
                o,
                "read from code: not run  |  your build and tests: not run  |  your real app: not run"
            );
        }
    }
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
    if let Some(a) = &analysis {
        let sec = sections(a, lane_for_sections);
        let (visible, hidden) = cap_desks(&sec.fix_first);
        if visible.is_empty() {
            let _ = writeln!(o, "  (none proven wrong)");
        }
        for (i, f) in visible.iter().enumerate() {
            let _ = writeln!(
                o,
                "  {}. [{:?}] {} ({}{}; record {})",
                i + 1,
                f.severity,
                f.title,
                f.tier.as_str(),
                f.evidence
                    .first()
                    .map(|e| format!(", {e}"))
                    .unwrap_or_default(),
                f.seq
            );
        }
        for (desk, n) in hidden {
            let _ = writeln!(o, "  (+{n} more from the {desk} desk, in the ledger)");
        }
    } else if findings.is_empty() {
        let _ = writeln!(
            o,
            "  (nothing was checked, so nothing is proven wrong and nothing is proven right)"
        );
    } else {
        for (i, s) in findings.iter().enumerate() {
            if let Record::Finding(f) = &s.record {
                let _ = writeln!(
                    o,
                    "  {}. [{:?}] {} ({}, record {}; no verdict has been computed yet, so not weighed)",
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
    if let Some(a) = &analysis {
        let sec = sections(a, lane_for_sections);
        let (visible, hidden) = cap_desks(&sec.settle);
        for f in &visible {
            n += 1;
            let _ = writeln!(
                o,
                "  {n}. [{:?}] {} ({}{}; record {})",
                f.severity,
                f.title,
                f.tier.as_str(),
                f.evidence
                    .first()
                    .map(|e| format!(", {e}"))
                    .unwrap_or_default(),
                f.seq
            );
        }
        for (desk, more) in hidden {
            n += 1;
            let _ = writeln!(
                o,
                "  {n}. (+{more} more from the {desk} desk, in the ledger)"
            );
        }
        for ov in a.overrides.iter().filter(|ov| ov.open) {
            n += 1;
            let _ = writeln!(
                o,
                "  {n}. commit-anyway override open (record {}{}, reason: {}): resolve {} or accept it as not real",
                ov.seq,
                crate::verdict::override_origin(ov, id),
                ov.reason,
                ov.finding_ids.join(", ")
            );
        }
    }
    if !integrity.is_ok() {
        for p in &integrity.problems {
            n += 1;
            let _ = writeln!(o, "  {n}. ledger integrity: {}", p.message);
        }
    }
    for s in &latest_unavailable {
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
    // Open prompt-translation questions: the verdict counts them as yours to settle, so the
    // report lists them too (it must never say "nothing" while the verdict says NEEDS YOU).
    for d in crate::part1c::open_decisions(ledger, Some(id))?
        .into_iter()
        .filter(|d| d.decision == crate::model::DecisionKind::AcceptPromptTranslation)
    {
        n += 1;
        let at = d
            .record_seq
            .map(|s| format!(" (record {s})"))
            .unwrap_or_default();
        let _ = writeln!(o, "  {n}. {}{at}", indent_block(&d.question, "     "));
    }
    if n == 0 {
        let _ = writeln!(o, "  (nothing recorded for you to settle)");
    }

    if let Some(a) = &analysis {
        let closed: Vec<_> = a.findings.iter().filter(|f| !f.is_open()).collect();
        if !closed.is_empty() {
            let _ = writeln!(o, "\nCLOSED FINDINGS");
            for f in closed {
                let how = match &f.status {
                    FindingStatus::AcceptedNotReal { decision_seq } => {
                        format!("accepted as not real by you (decision record {decision_seq})")
                    }
                    FindingStatus::Resolved { correction_seq } => {
                        format!("resolved (correction record {correction_seq})")
                    }
                    FindingStatus::Open => String::new(),
                };
                let _ = writeln!(
                    o,
                    "  [{:?}] {} (record {}): {how}",
                    f.finding.severity, f.finding.title, f.seq
                );
            }
        }
        if !a.overrides.is_empty() {
            let _ = writeln!(
                o,
                "\nCOMMIT ANYWAY (the commit went through; the verdict is unchanged)"
            );
        }
        for ov in &a.overrides {
            let _ = writeln!(
                o,
                "  record {}{}: past {} for tree {}. Reason you typed: {}{}",
                ov.seq,
                crate::verdict::override_origin(ov, id),
                ov.finding_ids.join(", "),
                ov.tree.get(..12).unwrap_or(&ov.tree),
                ov.reason,
                if ov.open {
                    " (still open)"
                } else {
                    " (closed: the finding was resolved or accepted as not real)"
                }
            );
        }
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
                let shown = if *decision == crate::model::DecisionKind::CommitAnyway {
                    let c = crate::verdict::CommitAnywayChoice::decode(choice);
                    format!(
                        "tree {} past {} finding(s)",
                        c.tree.get(..12).unwrap_or(&c.tree),
                        c.finding_ids.len()
                    )
                } else {
                    choice.clone()
                };
                let _ = writeln!(
                    o,
                    "  record {}: {} = {}{}",
                    s.seq,
                    decision.as_str(),
                    shown,
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
                "  {}. ({}) {}{names}{}",
                i + 1,
                source.as_str(),
                indent_block(text, "     "),
                if head.is_some() {
                    ""
                } else {
                    "  - not checked"
                }
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

    if let Some(a) = &analysis {
        let tension = crate::verdict::dissent(a);
        if !tension.is_empty() {
            let _ = writeln!(o, "\nDESK DISSENT (both findings stay)");
            for (i, s) in tension.iter().enumerate() {
                let _ = writeln!(o, "  {}. {s}", i + 1);
            }
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
        if head.is_some() {
            let _ = writeln!(o, "  none ran; nothing above was compared with the code.");
        } else {
            let _ = writeln!(
                o,
                "  none ran. This build has no checkers yet; nothing above was compared with the code."
            );
        }
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
        if head.is_some() {
            let _ = writeln!(o, "  {ran} of {} checks ran.", checks.len());
        } else {
            let _ = writeln!(
                o,
                "  {ran} of {} checks ran; no verdict has been computed, so none is weighed.",
                checks.len()
            );
        }
    }
    if let (Some(h), Some(a)) = (&head, &analysis) {
        for name in &h.required {
            match a.checks.get(name) {
                Some(CheckState::Ran { .. }) => {}
                Some(CheckState::Skipped { .. }) => {}
                Some(CheckState::Unavailable { reason, .. }) => {
                    let _ = writeln!(o, "  required: {name} could not run: {reason}");
                }
                None => {
                    let _ = writeln!(o, "  required: {name} did not run");
                }
            }
        }
    }

    let _ = writeln!(o, "\nNOT ESTABLISHED");
    if let Some(h) = &head {
        for line in &h.not_established {
            let _ = writeln!(o, "  - {line}");
        }
        let _ = writeln!(
            o,
            "  - whether any of the {} claim(s) is true beyond what the checks above show: claims are the agent's words, not evidence",
            claims.len()
        );
        return Ok(o);
    }
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
    for s in &latest_unavailable {
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
