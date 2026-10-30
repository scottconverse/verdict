//! The human's phase-2 decisions, in one place, so the localhost page and `vdt` write exactly the
//! same records.
//!
//! Every action here writes a decision record as writer `human` (the ledger refuses a decision from
//! anyone else) and validates first, so a bad request writes nothing:
//!
//! * accept a finding as not real (closes that exact occurrence),
//! * accept a prompt-translation claim as what the human asked for,
//! * approve the repo's commands,
//! * raise the lane (never lower it),
//! * switch between shadow and enforce,
//! * pause the gate for a stated duration,
//! * commit anyway, with a typed reason, for the exact staged tree.
//!
//! Nothing in the MCP server or a hook calls this module.

use std::path::Path;

use crate::lane::{lane_name, parse_lane, staged_tree};
use crate::ledger::{Ledger, LedgerError};
use crate::model::{DecisionKind, Lane, Record, Writer};
use crate::part1c::open_decisions;
use crate::verdict::{
    AcceptChoice, CommitAnywayChoice, analyze, compute_live, finding_view_at, is_blocking,
};

/// What the human asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Accept a finding as not real: its record number (closes that exact occurrence, in any
    /// session) or its id (the open occurrence in this session).
    AcceptFinding(String),
    /// Accept the prompt-translation claim recorded at this number as what the human asked for.
    AcceptPromptTranslation(u64),
    /// Approve running this repo's own commands (the exec checker).
    ApproveCommands,
    /// Raise the lane to this one.
    RaiseLane(Lane),
    /// Switch the gate to `shadow` or `enforce`.
    SetMode(String),
    /// Pause the gate; the text is a duration such as `30m`, `2h`, `1d`.
    Pause(String),
    /// Commit anyway, with the reason the human typed. `tree` overrides the staged tree.
    CommitAnyway {
        /// The reason, typed by the human.
        reason: String,
        /// The tree hash, when the human names one; else the current staged tree.
        tree: Option<String>,
    },
    /// Which file is the plan (a path relative to the project).
    SelectPlan(String),
    /// Withdraw the prompt with this ordinal (the human cancelled it).
    WithdrawPrompt(u32),
    /// Waive words no check can cover ("faster", "nicer").
    WaiveWords(String),
    /// Register Verdict's MCP server with a host (human only, like installing hooks).
    McpInstall(String),
    /// Accept the rules as they are now, after they changed during this session.
    AcceptPolicy,
}

/// Parse a pause duration: a whole number then `s`, `m`, `h` or `d`. Returns seconds.
pub fn parse_pause(text: &str) -> Result<u64, String> {
    let t = text.trim().to_ascii_lowercase();
    let split = t
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("{text:?} needs a unit: s, m, h or d (for example 30m)"))?;
    let (num, unit) = t.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("{text:?} is not a duration (for example 30m, 2h, 1d)"))?;
    if n == 0 {
        return Err("a pause must be longer than zero".into());
    }
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => {
            return Err(format!(
                "{text:?}: the unit must be s, m, h or d (for example 30m)"
            ));
        }
    };
    n.checked_mul(mult)
        .filter(|s| *s <= 10 * 365 * 86_400)
        .ok_or_else(|| format!("{text:?} is too long"))
}

/// Milliseconds since the Unix epoch for an RFC 3339 UTC time like `2026-10-26T12:00:00.000Z`.
pub fn parse_rfc3339_millis(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
        return None;
    }
    let num = |a: usize, z: usize| ts.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut millis = 0;
    if b.get(19) == Some(&b'.') {
        let frac: String = ts[20..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let padded = format!("{frac:0<3}");
        millis = padded[..3].parse::<i64>().ok()?;
    }
    // Howard Hinnant's civil-to-days algorithm.
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) + h * 3600 + mi * 60 + s) * 1000 + millis)
}

/// What a recorded pause means now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePause {
    /// The decision record.
    pub seq: u64,
    /// When it ends, milliseconds since the Unix epoch.
    pub until_ms: i64,
}

/// The pause in force at `now_ms`, if any: the one that ends last among the human's pause
/// decisions. Pauses are project-wide.
pub fn active_pause(ledger: &Ledger, now_ms: i64) -> Result<Option<ActivePause>, LedgerError> {
    let mut best: Option<ActivePause> = None;
    for s in ledger.records()? {
        if let Record::Decision {
            decision: DecisionKind::Pause,
            choice,
            who: Writer::Human,
            ..
        } = &s.record
        {
            let (Some(start), Ok(secs)) = (parse_rfc3339_millis(&s.ts), parse_pause(choice)) else {
                continue;
            };
            let until = start.saturating_add((secs as i64).saturating_mul(1000));
            if until > now_ms && best.as_ref().is_none_or(|b| until > b.until_ms) {
                best = Some(ActivePause {
                    seq: s.seq,
                    until_ms: until,
                });
            }
        }
    }
    Ok(best)
}

/// `Some("shadow" | "enforce")` from the human's latest mode switch, `None` when they never chose.
pub fn chosen_mode(ledger: &Ledger) -> Result<Option<String>, LedgerError> {
    let mut mode = None;
    for s in ledger.records()? {
        if let Record::Decision {
            decision: DecisionKind::ModeSwitch,
            choice,
            who: Writer::Human,
            ..
        } = &s.record
            && matches!(choice.as_str(), "shadow" | "enforce")
        {
            mode = Some(choice.clone());
        }
    }
    Ok(mode)
}

/// A commit-anyway decision for exactly this staged tree, in this session or project-wide.
pub fn commit_anyway_for_tree(
    ledger: &Ledger,
    session: &str,
    tree: &str,
) -> Result<Option<u64>, LedgerError> {
    Ok(ledger.records()?.into_iter().find_map(|s| match &s.record {
        Record::Decision {
            decision: DecisionKind::CommitAnyway,
            choice,
            who: Writer::Human,
            ..
        } if (s.session_id.is_none() || s.session_id.as_deref() == Some(session))
            && !tree.is_empty()
            && CommitAnywayChoice::decode(choice).tree == tree =>
        {
            Some(s.seq)
        }
        _ => None,
    }))
}

/// What a commit-anyway would cover right now: the staged tree (or the one the human names) and
/// the open blocking findings. Fails when the verdict is not FAILED, because then there is nothing
/// to commit past.
pub fn commit_anyway_plan(
    root: &Path,
    ledger: &Ledger,
    session: &str,
    tree: Option<&str>,
    home: Option<&Path>,
) -> Result<CommitAnywayChoice, String> {
    let io = |e: LedgerError| e.to_string();
    let c = compute_live(root, ledger, session, home).map_err(io)?;
    let a = analyze(ledger, session).map_err(io)?;
    let ids: Vec<String> = a
        .findings
        .iter()
        .filter(|f| is_blocking(f, c.lane))
        .map(|f| f.finding.id.clone())
        .collect();
    if ids.is_empty() {
        return Err(format!(
            "there is nothing to commit past: the verdict is {}, not FAILED",
            c.label()
        ));
    }
    let tree = tree
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| staged_tree(root))
        .unwrap_or_else(|| "none".into());
    Ok(CommitAnywayChoice {
        tree,
        finding_ids: ids,
    })
}

/// Carry out one human action: validate, then write the decision record as the human.
/// Returns the new record's sequence number, or the reason nothing was written.
///
/// `home` is the folder that holds `.verdict/policy.json` (`None` for the user's home).
pub fn apply(
    root: &Path,
    ledger: &mut Ledger,
    session: &str,
    action: &Action,
    home: Option<&Path>,
) -> Result<u64, String> {
    let io = |e: LedgerError| e.to_string();
    let (decision, choice, reason): (DecisionKind, String, Option<String>) = match action {
        Action::AcceptFinding(which) => {
            // The decision names the exact occurrence (record number and evidence), so a later
            // finding with the same id and different evidence is open again.
            let which = which.trim().trim_start_matches('#');
            let view = match which.parse::<u64>() {
                // A record number names one finding anywhere in the project (an override from
                // another session can be blocked by it).
                Ok(seq) => {
                    let all = ledger.records().map_err(io)?;
                    finding_view_at(&all, seq).filter(|v| v.is_open())
                }
                Err(_) => analyze(ledger, session)
                    .map_err(io)?
                    .findings
                    .into_iter()
                    .find(|f| f.finding.id == which && f.is_open()),
            };
            let Some(view) = view else {
                return Err(format!("there is no open finding {which:?}"));
            };
            (
                DecisionKind::AcceptFindingNotReal,
                AcceptChoice::for_finding(view.seq, &view.finding).encode(),
                None,
            )
        }
        Action::AcceptPromptTranslation(seq) => {
            let open = open_decisions(ledger, None).map_err(io)?;
            if !open.iter().any(|d| {
                d.decision == DecisionKind::AcceptPromptTranslation && d.record_seq == Some(*seq)
            }) {
                return Err(format!(
                    "claim #{seq} is not an open prompt-translation question"
                ));
            }
            (DecisionKind::AcceptPromptTranslation, seq.to_string(), None)
        }
        Action::ApproveCommands => {
            // The approval covers exactly the commands the policy lists right now.
            let policy = crate::policy::Policy::load(root, home).map_err(|e| {
                format!("the policy could not be read, so there are no commands to approve: {e}")
            })?;
            (
                DecisionKind::RunProjectCommands,
                crate::exec_approval::approval_choice(&policy),
                None,
            )
        }
        Action::AcceptPolicy => {
            // The acceptance names exactly the rules in force now (by hash).
            let policy = crate::policy::Policy::load(root, home).map_err(|e| {
                format!("the policy could not be read, so there is nothing to accept: {e}")
            })?;
            if crate::verdict::policy_change(ledger, session, &policy)
                .map_err(io)?
                .is_none()
            {
                return Err(
                    "the rules have not changed during this session; there is nothing to accept"
                        .into(),
                );
            }
            (DecisionKind::AcceptPolicy, policy.hash.clone(), None)
        }
        Action::RaiseLane(lane) => {
            let c = compute_live(root, ledger, session, home).map_err(io)?;
            if *lane <= c.lane {
                return Err(format!(
                    "the lane is already {} (what the diff touches implies {}); a lane can only be raised",
                    lane_name(c.lane),
                    lane_name(c.implied_lane)
                ));
            }
            (DecisionKind::RaiseLane, lane_name(*lane).into(), None)
        }
        Action::SetMode(mode) => {
            let mode = mode.trim().to_ascii_lowercase();
            if !matches!(mode.as_str(), "shadow" | "enforce") {
                return Err(format!("the mode must be shadow or enforce, not {mode:?}"));
            }
            (DecisionKind::ModeSwitch, mode, None)
        }
        Action::Pause(text) => {
            let secs = parse_pause(text)?;
            (
                DecisionKind::Pause,
                format!("{}s", secs),
                Some(format!("paused for {}", text.trim())),
            )
        }
        Action::CommitAnyway { reason, tree } => {
            if reason.trim().is_empty() {
                return Err(
                    "commit anyway needs a reason you type; an empty reason is refused".into(),
                );
            }
            let plan = commit_anyway_plan(root, ledger, session, tree.as_deref(), home)?;
            (
                DecisionKind::CommitAnyway,
                plan.encode(),
                Some(reason.trim().to_string()),
            )
        }
        Action::SelectPlan(path) => {
            let path = path.trim().replace('\\', "/");
            if path.is_empty() {
                return Err("choose a plan file (or answer that there is none)".into());
            }
            let joined = root.join(&path);
            if !joined.is_file() {
                return Err(format!("{path} is not a file in this project"));
            }
            (DecisionKind::PlanFile, path, None)
        }
        Action::WithdrawPrompt(ordinal) => {
            let exists =
                ledger.records().map_err(|e| e.to_string())?.iter().any(
                    |s| matches!(&s.record, Record::Prompt { ordinal: n, .. } if n == ordinal),
                );
            if !exists {
                return Err(format!(
                    "there is no prompt {ordinal} in this project's ledger"
                ));
            }
            (DecisionKind::WithdrawPrompt, ordinal.to_string(), None)
        }
        Action::WaiveWords(words) => {
            let words = words.trim();
            if words.is_empty() {
                return Err("name at least one word to waive".into());
            }
            (DecisionKind::WaiveWords, words.to_string(), None)
        }
        Action::McpInstall(host) => {
            let host = host.trim().to_ascii_lowercase();
            if !["claude", "cursor", "codex", "dsh", "antigravity"].contains(&host.as_str()) {
                return Err(format!("{host:?} is not a supported host"));
            }
            (DecisionKind::McpInstall, host, None)
        }
    };
    let appended = ledger
        .append(
            Writer::Human,
            Some(session),
            Record::Decision {
                decision,
                choice,
                reason,
                who: Writer::Human,
            },
        )
        .map_err(io)?;
    Ok(appended.seq)
}

/// Lane from a form or command-line word.
pub fn lane_from_text(s: &str) -> Result<Lane, String> {
    parse_lane(s).ok_or_else(|| format!("{s:?} is not a lane (low, standard, high or release)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_and_bad_ones_are_refused() {
        assert_eq!(parse_pause("30m"), Ok(1800));
        assert_eq!(parse_pause(" 2H "), Ok(7200));
        assert_eq!(parse_pause("1d"), Ok(86_400));
        assert!(parse_pause("0m").is_err());
        assert!(parse_pause("soon").is_err());
        assert!(parse_pause("10").is_err());
        assert!(parse_pause("10 weeks").is_err());
        assert!(parse_pause("99999999999999999999d").is_err());
    }

    #[test]
    fn timestamps_parse_to_the_same_instant_the_formatter_wrote() {
        let ms = 1_709_210_096_789;
        let text = crate::timefmt::format_unix_millis(ms);
        assert_eq!(parse_rfc3339_millis(&text), Some(ms));
        assert_eq!(parse_rfc3339_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_millis("garbage"), None);
    }
}
