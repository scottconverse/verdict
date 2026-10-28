//! Verdict's verification engine.
//!
//! One function, [`verify`], is the only way the ordinary agent completion path (the Stop hook and
//! the MCP `verdict_check` tool) turns the ledger into a verdict. It runs the lane's deterministic
//! checkers, dispatches the lane's desks through the configured providers, computes the verdict
//! from the ledger, writes the verdict record and writes the general gate-result record the design
//! requires after the final message.
//!
//! Before this crate existed the hook and MCP paths only computed from evidence that was already
//! in the ledger: an agent could say it was done without any fresh check. Both now call [`verify`].
//! A caller may give a time budget: checkers that cannot start inside it are recorded as
//! `check_unavailable`, never as clean, so a timed-out run reads PARTIAL/UNVERIFIED.
//!
//! The engine is also the one [`verdict_core::gate::CheckRunner`] implementation: the pre-commit
//! gate and `vdt verdict --run` use the same checker and desk code, so there is no second path.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use verdict_core::gate::{CheckRunner, CheckSummary, RunOptions as GateRunOptions};
use verdict_core::lane::effective_lane;
use verdict_core::model::{CheckOutcome, Lane, Record, Tier, VerdictWord, Writer};
use verdict_core::verdict::{Computed, compute, live_input, record_verdict};
use verdict_core::{Ledger, LedgerError};

/// Runs the real checkers and desks. `home` is the folder holding `.verdict/policy.json` (`None`
/// means the user's home folder); tests pass an empty folder so a developer's policy cannot leak
/// into their result.
#[derive(Debug, Clone, Default)]
pub struct RealRunner {
    /// The policy home.
    pub home: Option<PathBuf>,
}

impl RealRunner {
    /// A runner that reads its policy from `home`.
    pub fn with_home(home: Option<PathBuf>) -> Self {
        RealRunner { home }
    }
}

impl CheckRunner for RealRunner {
    fn run_lane(
        &self,
        ledger: &mut Ledger,
        root: &Path,
        session: &str,
        checkers: &[String],
        opts: &GateRunOptions,
    ) -> Result<Vec<CheckSummary>, String> {
        let o = verdict_checkers::RunOptions {
            lane: opts.lane,
            timeout: opts.timeout,
            deadline: opts.deadline,
        };
        verdict_checkers::run_lane_with_home(
            ledger,
            root,
            session,
            checkers,
            &o,
            self.home.as_deref(),
        )
        .map(|v| {
            v.into_iter()
                .map(|s| CheckSummary {
                    name: s.name,
                    outcome: s.outcome,
                    findings: s.findings,
                    check_seq: s.check_seq,
                })
                .collect()
        })
    }

    fn run_desks(
        &self,
        ledger: &mut Ledger,
        root: &Path,
        session: &str,
        lane: Lane,
    ) -> Result<(), String> {
        let policy = verdict_core::verdict::load_policy(root, self.home.as_deref())
            .map(|(p, _)| p)
            .map_err(|e| e.to_string())?;
        let specs = verdict_desks::providers_from_policy(&policy)?;
        let providers: Vec<Box<dyn verdict_desks::DeskProvider>> = specs
            .into_iter()
            .map(|p| Box::new(p) as Box<dyn verdict_desks::DeskProvider>)
            .collect();
        let opts = verdict_desks::DeskOptions {
            lane,
            timeout: Duration::from_secs(300),
        };
        match verdict_desks::run(ledger, root, session, &policy, &opts, &providers)? {
            verdict_desks::Outcome::Ran { .. } => Ok(()),
            verdict_desks::Outcome::Refused { reason, .. } => Err(reason),
        }
    }
}

/// What a fresh verification came to.
#[derive(Debug, Clone)]
pub struct Verified {
    /// The verdict computed from the ledger after the run.
    pub computed: Computed,
    /// The sequence number of the verdict record.
    pub verdict_seq: u64,
    /// The sequence number of the general gate-result record.
    pub gate_seq: u64,
}

/// How a fresh verification is run.
#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    /// Stop when this much time has passed; checkers that have not started are recorded as
    /// unavailable. `None` means no extra budget (each checker still has its command timeout).
    pub budget: Option<Duration>,
    /// Why this verification ran, for the gate-result record (`stop hook`, `verdict_check`, ...).
    pub reason: Option<String>,
}

/// The label of the general gate-result record. It is a `check` record, so it appears in the
/// ledger sequence the design requires after the final message.
pub const GATE_RECORD: &str = "gate";

/// Run the lane's checkers and desks for `session`, then compute and record the verdict and the
/// general gate result. Nothing is treated as clean because it was not looked at.
pub fn verify(
    root: &Path,
    ledger: &mut Ledger,
    session: &str,
    home: Option<&Path>,
    opts: &VerifyOptions,
) -> Result<Verified, String> {
    let live = live_input(root, ledger, session, home).map_err(|e| e.to_string())?;
    record_lane_unavailable(ledger, session, &live).map_err(|e| e.to_string())?;
    let lane =
        effective_lane(ledger, session, live.input.implied_lane).map_err(|e| e.to_string())?;
    let checkers = live
        .input
        .lane_checkers
        .get(&lane)
        .cloned()
        .unwrap_or_default();
    let deadline = opts.budget.map(|b| Instant::now() + b);
    let configured = default_timeout(&live.policy);
    let timeout = remaining(deadline).map_or(configured, |left| {
        left.min(configured).max(Duration::from_millis(1))
    });

    let runner = RealRunner::with_home(home.map(Path::to_path_buf));
    verdict_core::gate::run_checkers(
        ledger,
        root,
        session,
        &checkers,
        &runner,
        &GateRunOptions {
            lane,
            timeout,
            deadline,
        },
    )
    .map_err(|e| e.to_string())?;

    let budget_left = remaining(deadline);
    if budget_left.is_none_or(|left| !left.is_zero()) {
        let desks = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            runner.run_desks(ledger, root, session, lane)
        }))
        .unwrap_or_else(|_| Err("the desk engine crashed".into()));
        if let Err(reason) = desks {
            ledger
                .append(
                    Writer::Core,
                    Some(session),
                    Record::CheckUnavailable {
                        component: "desks".into(),
                        reason,
                    },
                )
                .map_err(|e| e.to_string())?;
        }
        run_proposals(
            ledger,
            root,
            session,
            home,
            lane,
            deadline,
            &runner,
            &live.policy,
        )
        .map_err(|e| e.to_string())?;
    } else {
        ledger
            .append(
                Writer::Core,
                Some(session),
                Record::CheckUnavailable {
                    component: "desks".into(),
                    reason:
                        "the run's time budget was used up before the desks could be dispatched"
                            .into(),
                },
            )
            .map_err(|e| e.to_string())?;
    }

    let computed = compute(ledger, session, &live.input).map_err(|e| e.to_string())?;
    let verdict_seq = record_verdict(ledger, &computed).map_err(|e| e.to_string())?;
    let gate_seq = record_gate_result(ledger, session, &computed, opts.reason.as_deref())
        .map_err(|e| e.to_string())?;
    Ok(Verified {
        computed,
        verdict_seq,
        gate_seq,
    })
}

/// The general gate result after the final message: one `check` record naming the word, the lane
/// and the reason the run happened. It is not a special send-back check; it is the result record
/// the design's Stop sequence asks for.
fn record_gate_result(
    ledger: &mut Ledger,
    session: &str,
    c: &Computed,
    reason: Option<&str>,
) -> Result<u64, LedgerError> {
    let now = verdict_core::timefmt::now_rfc3339();
    Ok(ledger
        .append(
            Writer::Core,
            Some(session),
            Record::Check {
                name: GATE_RECORD.into(),
                tier: Tier::Unverified,
                inputs_hash: String::new(),
                started_at: now.clone(),
                finished_at: now,
                outcome: CheckOutcome::Ran,
                detail: Some(format!(
                    "gate result{}: VERDICT {} (lane {}, policy v{}); {} required checker(s); {} item(s) not established",
                    reason.map(|r| format!(" ({r})")).unwrap_or_default(),
                    c.label(),
                    verdict_core::lane::lane_name(c.lane),
                    c.policy_version,
                    c.required.len(),
                    c.not_established.len()
                )),
                tree_state: verdict_core::baseline::tree_state_of_ledger(ledger),
            },
        )?
        .seq)
}

fn record_lane_unavailable(
    ledger: &mut Ledger,
    session: &str,
    live: &verdict_core::verdict::Live,
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
                if component == verdict_core::lane::LANE_COMPONENT =>
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
                component: verdict_core::lane::LANE_COMPONENT.into(),
                reason: why,
            },
        )?;
    }
    Ok(())
}

fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|d| d.saturating_duration_since(Instant::now()))
}

/// A desk may propose a checker run (a mutation cell or a first-run cell). When the policy allows
/// it (`desks.run_proposals: true`) and the proposal names a checker this build has, the core runs
/// that checker on the current code. The desk itself never runs code; the proposal is a
/// hypothesis in the ledger, and running it is recorded as a normal check.
#[allow(clippy::too_many_arguments)]
fn run_proposals(
    ledger: &mut Ledger,
    root: &Path,
    session: &str,
    home: Option<&Path>,
    lane: Lane,
    deadline: Option<Instant>,
    runner: &RealRunner,
    policy: &verdict_core::Policy,
) -> Result<(), LedgerError> {
    if !policy
        .value
        .pointer("/desks/run_proposals")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(());
    }
    let mut wanted: Vec<String> = Vec::new();
    for s in ledger.records_for_session(session)? {
        let Record::Hypothesis(f) = &s.record else {
            continue;
        };
        let Some(rest) = f.title.strip_prefix("PROPOSAL: ") else {
            continue;
        };
        let checker = rest
            .split([' ', '—', '-'])
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if verdict_checkers::CHECKER_NAMES.contains(&checker.as_str())
            && !wanted.contains(&checker)
            && matches!(checker.as_str(), "mutation" | "firstrun")
        {
            wanted.push(checker);
        }
    }
    if wanted.is_empty() {
        return Ok(());
    }
    let now = verdict_core::timefmt::now_rfc3339();
    ledger.append(
        Writer::Core,
        Some(session),
        Record::Check {
            name: "proposals".into(),
            tier: Tier::Unverified,
            inputs_hash: String::new(),
            started_at: now.clone(),
            finished_at: now,
            outcome: CheckOutcome::Ran,
            detail: Some(format!(
                "the desks proposed these checker runs and the policy allows running them: {}",
                wanted.join(", ")
            )),
            tree_state: verdict_core::baseline::tree_state_of_ledger(ledger),
        },
    )?;
    let timeout = default_timeout(policy);
    verdict_core::gate::run_checkers(
        ledger,
        root,
        session,
        &wanted,
        runner,
        &GateRunOptions {
            lane,
            timeout,
            deadline,
        },
    )?;
    let _ = home;
    Ok(())
}

fn default_timeout(policy: &verdict_core::Policy) -> Duration {
    policy
        .value
        .pointer("/exec/timeout")
        .and_then(serde_json::Value::as_str)
        .and_then(verdict_core::exec_approval::parse_duration)
        .unwrap_or(Duration::from_secs(180))
}

/// The design's Stop budget, read from the policy's `gate.stop_timeout`.
pub fn stop_timeout(policy: &verdict_core::Policy, fallback: Duration) -> Duration {
    policy
        .value
        .pointer("/gate/stop_timeout")
        .and_then(serde_json::Value::as_str)
        .and_then(verdict_core::exec_approval::parse_duration)
        .unwrap_or(fallback)
}

/// True when the word would hold the agent in enforce mode.
pub fn holds(word: VerdictWord) -> bool {
    word == VerdictWord::Failed
}
