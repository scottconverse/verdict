//! The pre-commit gate: classify the lane, run the lane's checkers, compute the verdict, say what
//! it would do.
//!
//! * **Shadow mode** (the default) always lets the commit through and records what it would have
//!   done. A fresh install holds nothing.
//! * **Enforce mode** (a human mode-switch decision in the ledger, else the policy's `gate.mode`)
//!   holds a commit whose verdict is `FAILED`, unless the human recorded a commit-anyway decision
//!   for this exact staged tree. Commit anyway lets the commit through; the verdict stays `FAILED`.
//! * A **pause** decision makes the gate skip until it ends; the skip is recorded.
//!
//! Checkers are reached only through [`CheckRunner`], so the core never depends on the checker
//! crate. [`NoRunner`] is the stand-in that says no runner is wired: every required checker is then
//! recorded as unavailable and the verdict reads PARTIAL, never clean.
//!
//! The gate fails open on the tool: any error here is the caller's cue to let the commit through.
//! It never fails open on the verdict: a `FAILED` verdict is `FAILED` in either mode.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Duration;

use crate::human::{ActivePause, active_pause, chosen_mode, commit_anyway_for_tree};
use crate::lane::{lane_name, staged_tree};
use crate::ledger::{Ledger, LedgerError};
use crate::model::{CheckOutcome, Lane, Record, Tier, VerdictWord, Writer};
use crate::part1c::resolve_session;
use crate::session::{SessionParams, start_session};
use crate::timefmt::format_unix_millis;
use crate::verdict::{Computed, analyze, compute, live_input, load_policy, record_verdict};

/// What a checker run is told: which lane, and how long each checker may take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// The lane in force.
    pub lane: Lane,
    /// The time limit for the run.
    pub timeout: Duration,
}

/// What one checker reports back. The checker has already written its `check` and `finding`
/// records; this is the receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSummary {
    /// The checker's name.
    pub name: String,
    /// What happened.
    pub outcome: CheckOutcome,
    /// How many findings it wrote.
    pub findings: usize,
    /// The sequence number of its `check` record.
    pub check_seq: u64,
}

/// The one door to the checkers. The real implementation calls `verdict_checkers::run_lane`;
/// tests use a fake that writes check and finding records.
pub trait CheckRunner {
    /// Run `checkers` (already expanded, in run order) for `session` against the tree at `root`.
    /// Each checker writes its own records into `ledger`. An `Err` means the runner as a whole
    /// could not run (it is recorded as every required checker being unavailable).
    fn run_lane(
        &self,
        ledger: &mut Ledger,
        root: &Path,
        session: &str,
        checkers: &[String],
        opts: &RunOptions,
    ) -> Result<Vec<CheckSummary>, String>;
}

/// The runner used when none is wired in: it runs nothing and says so.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRunner;

impl CheckRunner for NoRunner {
    fn run_lane(
        &self,
        _ledger: &mut Ledger,
        _root: &Path,
        _session: &str,
        _checkers: &[String],
        _opts: &RunOptions,
    ) -> Result<Vec<CheckSummary>, String> {
        Err("no checker runner is wired into this build, so no checker ran".into())
    }
}

/// How the gate was asked to run.
#[derive(Debug, Clone)]
pub struct GateOptions<'a> {
    /// The project.
    pub root: &'a Path,
    /// The folder holding `.verdict/policy.json` (`None`: the user's home folder).
    pub home: Option<&'a Path>,
    /// The current time in milliseconds since the epoch (`None`: now). Tests set it.
    pub now_ms: Option<i64>,
}

/// What the gate did.
#[derive(Debug, Clone)]
pub struct GateOutcome {
    /// 0 lets the commit through; 1 holds it.
    pub exit_code: u8,
    /// `shadow` or `enforce`.
    pub mode: String,
    /// The computed verdict (none when the gate was paused).
    pub computed: Option<Computed>,
    /// True when the verdict is FAILED, so enforce mode would hold the commit.
    pub would_hold: bool,
    /// The pause that made the gate skip, if any.
    pub paused: Option<ActivePause>,
    /// The commit-anyway decision that let a FAILED commit through, if any.
    pub commit_anyway_seq: Option<u64>,
    /// The staged tree this run looked at.
    pub tree: Option<String>,
    /// The session the run was recorded in.
    pub session: String,
    /// The text to print.
    pub text: String,
}

fn default_timeout(policy: &crate::policy::Policy) -> Duration {
    let text = policy
        .value
        .pointer("/exec/timeout")
        .and_then(|v| v.as_str())
        .unwrap_or("3m");
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (n, unit) = text.split_at(split);
    let n: u64 = n.parse().unwrap_or(180);
    match unit {
        "ms" => Duration::from_millis(n),
        "s" => Duration::from_secs(n),
        "m" => Duration::from_secs(n * 60),
        "h" => Duration::from_secs(n * 3600),
        _ => Duration::from_secs(180),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn gate_check(
    ledger: &mut Ledger,
    session: &str,
    outcome: CheckOutcome,
    detail: String,
) -> Result<u64, LedgerError> {
    let now = crate::timefmt::now_rfc3339();
    Ok(ledger
        .append(
            Writer::Core,
            Some(session),
            Record::Check {
                name: "gate".into(),
                tier: Tier::Unverified,
                inputs_hash: String::new(),
                started_at: now.clone(),
                finished_at: now,
                outcome,
                detail: Some(detail),
                tree_state: crate::baseline::tree_state_of_ledger(ledger),
            },
        )?
        .seq)
}

/// When the session's diff could not be read, say so in the ledger: a `check_unavailable` record
/// for lane classification (the lane was set to the highest). Not repeated while the latest such
/// record of the session says the same.
fn record_lane_unavailable(
    ledger: &mut Ledger,
    session: &str,
    live: &crate::verdict::Live,
) -> Result<(), LedgerError> {
    let Some(why) = &live.input.diff_unavailable else {
        return Ok(());
    };
    let why = format!("{why}; the lane was set to release");
    let same = ledger
        .records_for_session(session)?
        .iter()
        .rev()
        .find_map(|s| match &s.record {
            Record::CheckUnavailable { component, reason }
                if component == crate::lane::LANE_COMPONENT =>
            {
                Some(*reason == why)
            }
            _ => None,
        })
        .unwrap_or(false);
    if !same {
        ledger.append(
            Writer::Core,
            Some(session),
            Record::CheckUnavailable {
                component: crate::lane::LANE_COMPONENT.into(),
                reason: why,
            },
        )?;
    }
    Ok(())
}

/// Run the pre-commit gate for the project in `opts.root`.
pub fn run_pre_commit(
    opts: &GateOptions<'_>,
    runner: &dyn CheckRunner,
) -> Result<GateOutcome, LedgerError> {
    let root = opts.root;
    let now = opts.now_ms.unwrap_or_else(now_millis);
    let mut ledger = Ledger::open(root)?;
    let session = match resolve_session(&ledger, None)? {
        Some(s) => s,
        None => {
            start_session(
                &mut ledger,
                root,
                &SessionParams {
                    agent: "pre-commit-gate".into(),
                    model: "none".into(),
                    vendor: "none".into(),
                    home: opts.home.map(Path::to_path_buf),
                },
            )?
            .id
        }
    };
    let (policy, policy_warning) = load_policy(root, opts.home)?;
    if let Some(w) = &policy_warning {
        let reason = format!("{w}; the built-in default policy was used");
        let known = ledger.records_for_session(&session)?.iter().any(|s| {
            matches!(&s.record, Record::CheckUnavailable { component, reason: r }
                if component == "policy" && *r == reason)
        });
        if !known {
            ledger.append(
                Writer::Core,
                Some(&session),
                Record::CheckUnavailable {
                    component: "policy".into(),
                    reason,
                },
            )?;
        }
    }
    let mode = chosen_mode(&ledger)?.unwrap_or_else(|| policy.gate_mode().to_string());

    if let Some(p) = active_pause(&ledger, now)? {
        let until = format_unix_millis(p.until_ms);
        gate_check(
            &mut ledger,
            &session,
            CheckOutcome::SkippedByPolicy,
            format!(
                "gate paused by the human until {until} (decision record {}); no checker ran and this commit was not looked at",
                p.seq
            ),
        )?;
        return Ok(GateOutcome {
            exit_code: 0,
            mode,
            computed: None,
            would_hold: false,
            text: format!(
                "GATE: paused by you until {until} (decision record {}). Nothing was checked for this commit.\n",
                p.seq
            ),
            paused: Some(p),
            commit_anyway_seq: None,
            tree: None,
            session,
        });
    }

    let live = live_input(root, &ledger, &session, opts.home)?;
    record_lane_unavailable(&mut ledger, &session, &live)?;
    let lane = crate::lane::effective_lane(&ledger, &session, live.input.implied_lane)?;
    let checkers = live
        .input
        .lane_checkers
        .get(&lane)
        .cloned()
        .unwrap_or_default();

    run_checkers(
        &mut ledger,
        root,
        &session,
        &checkers,
        runner,
        &RunOptions {
            lane,
            timeout: default_timeout(&policy),
        },
    )?;

    let computed = compute(&ledger, &session, &live.input)?;
    record_verdict(&mut ledger, &computed)?;
    let would_hold = computed.word == VerdictWord::Failed;
    let tree = staged_tree(root);
    let anyway = if would_hold {
        match &tree {
            Some(t) => commit_anyway_for_tree(&ledger, &session, t)?,
            None => None,
        }
    } else {
        None
    };
    let enforce = mode == "enforce";
    let held = enforce && would_hold && anyway.is_none();

    let label = computed.label();
    let (gate_line, detail) = match (enforce, would_hold, anyway) {
        (false, true, _) => (
            "GATE: shadow mode. This commit would be held (FAILED); it goes through because nothing is held until you turn enforce on."
                .to_string(),
            "shadow mode: would have held this commit (FAILED); it went through".to_string(),
        ),
        (false, false, _) => (
            format!("GATE: shadow mode. This commit would go through ({label}). Not a release decision."),
            format!("shadow mode: would have let this commit through ({label})"),
        ),
        (true, true, Some(seq)) => (
            format!(
                "GATE: enforce mode. FAILED, but you chose commit anyway for this exact staged tree (decision record {seq}). The commit goes through; the verdict stays FAILED."
            ),
            format!("enforce mode: let through by commit-anyway record {seq}; verdict stays FAILED"),
        ),
        (true, true, None) => (
            format!(
                "GATE: enforce mode. This commit is HELD because the verdict is FAILED. To get past it: fix the findings above, accept one as not real, or commit anyway with a reason you type (vdt commit-anyway --reason \"...\" --i-am-the-human{}).",
                tree.as_ref()
                    .map(|t| format!(" --tree {t}"))
                    .unwrap_or_default()
            ),
            "enforce mode: held this commit (FAILED)".to_string(),
        ),
        (true, false, _) => (
            format!("GATE: enforce mode. This commit goes through ({label}). Not a release decision."),
            format!("enforce mode: let this commit through ({label})"),
        ),
    };
    gate_check(&mut ledger, &session, CheckOutcome::Ran, detail)?;

    let mut text = String::new();
    text.push_str(&format!(
        "VERDICT: {label}  (lane: {}, policy v{}, mode: {mode})\n",
        lane_name(computed.lane),
        computed.policy_version
    ));
    text.push_str(&format!(
        "read from code: {}  |  your build and tests: {}  |  your real app: {}\n",
        computed.three_state[0], computed.three_state[1], computed.three_state[2]
    ));
    if !computed.fix_first.is_empty() {
        text.push_str("\nFIX FIRST\n");
        for (i, f) in computed.fix_first.iter().enumerate() {
            text.push_str(&format!(
                "  {}. [{:?}] {} ({}{})\n",
                i + 1,
                f.severity,
                f.title,
                f.tier.as_str(),
                f.evidence
                    .first()
                    .map(|e| format!(", {e}"))
                    .unwrap_or_default()
            ));
        }
    }
    if !computed.yours_to_settle.is_empty() {
        text.push_str("\nYOURS TO SETTLE\n");
        for (i, s) in computed.yours_to_settle.iter().enumerate() {
            text.push_str(&format!("  {}. {s}\n", i + 1));
        }
    }
    if !computed.not_established.is_empty() {
        text.push_str("\nNOT ESTABLISHED\n");
        for s in &computed.not_established {
            text.push_str(&format!("  - {s}\n"));
        }
    }
    text.push_str(&format!("\n{gate_line}\n"));
    Ok(GateOutcome {
        exit_code: u8::from(held),
        mode,
        computed: Some(computed),
        would_hold,
        paused: None,
        commit_anyway_seq: anyway,
        tree,
        session,
        text,
    })
}

/// Run the lane's checkers through the runner. A runner that fails (or panics) is recorded as
/// every required checker being unavailable; one that works has its superseded findings resolved.
pub fn run_checkers(
    ledger: &mut Ledger,
    root: &Path,
    session: &str,
    checkers: &[String],
    runner: &dyn CheckRunner,
    opts: &RunOptions,
) -> Result<(), LedgerError> {
    let before = analyze(ledger, session)?;
    let ran = catch_unwind(AssertUnwindSafe(|| {
        runner.run_lane(ledger, root, session, checkers, opts)
    }))
    .unwrap_or_else(|_| Err("the checker runner crashed".into()));
    match ran {
        Ok(summaries) => resolve_superseded(ledger, session, &before, &summaries),
        Err(reason) => {
            for name in checkers {
                ledger.append(
                    Writer::Core,
                    Some(session),
                    Record::CheckUnavailable {
                        component: name.clone(),
                        reason: reason.clone(),
                    },
                )?;
            }
            Ok(())
        }
    }
}

/// A finding a checker no longer reports when it re-ran is resolved (a correction record by the
/// core). Only findings whose id names the checker (`<checker>:...`) are considered, and only when
/// that checker ran again and did not write the same id again.
fn resolve_superseded(
    ledger: &mut Ledger,
    session: &str,
    before: &crate::verdict::Analysis,
    summaries: &[CheckSummary],
) -> Result<(), LedgerError> {
    let rerun: BTreeMap<&str, u64> = summaries
        .iter()
        .filter(|s| s.outcome == CheckOutcome::Ran)
        .map(|s| (s.name.as_str(), s.check_seq))
        .collect();
    if rerun.is_empty() {
        return Ok(());
    }
    let after = analyze(ledger, session)?;
    for old in before.findings.iter().filter(|f| f.is_open()) {
        let Some(checker) = old.checker() else {
            continue;
        };
        let Some(check_seq) = rerun.get(checker) else {
            continue;
        };
        let reported_again = after
            .findings
            .iter()
            .any(|f| f.finding.id == old.finding.id && f.seq > old.seq);
        if !reported_again {
            ledger.append(
                Writer::Core,
                Some(session),
                Record::Correction {
                    target_seq: old.seq,
                    reason: format!(
                        "resolved: {checker} ran again (check record {check_seq}) and no longer reports it"
                    ),
                    replacement: None,
                },
            )?;
        }
    }
    Ok(())
}

/// What `vdt verdict` does: optionally run the lane's checkers through the runner, then compute and
/// write the verdict. Returns the computed verdict and the record's number.
pub fn run_verdict(
    root: &Path,
    home: Option<&Path>,
    session: Option<&str>,
    runner: Option<&dyn CheckRunner>,
) -> Result<(Computed, u64), LedgerError> {
    let mut ledger = Ledger::open(root)?;
    let Some(session) = resolve_session(&ledger, session)? else {
        return Err(LedgerError::Invalid(
            "this project's ledger has no sessions yet, so there is nothing to compute a verdict for"
                .into(),
        ));
    };
    let live = live_input(root, &ledger, &session, home)?;
    record_lane_unavailable(&mut ledger, &session, &live)?;
    if let Some(runner) = runner {
        let lane = crate::lane::effective_lane(&ledger, &session, live.input.implied_lane)?;
        let checkers = live
            .input
            .lane_checkers
            .get(&lane)
            .cloned()
            .unwrap_or_default();
        run_checkers(
            &mut ledger,
            root,
            &session,
            &checkers,
            runner,
            &RunOptions {
                lane,
                timeout: default_timeout(&live.policy),
            },
        )?;
    }
    let c = compute(&ledger, &session, &live.input)?;
    let seq = record_verdict(&mut ledger, &c)?;
    Ok((c, seq))
}
