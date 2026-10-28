//! Verdict's deterministic checkers (Part 2a). No model in any of them: the same inputs give the
//! same findings, every run.
//!
//! * [`Checker`] is the plugin contract: `name`, `tiers`, `applies(policy, lane)`,
//!   `run(ctx) -> Result<Vec<Finding>, Unavailable>`. Checkers never call each other; ordering is
//!   [`run_lane`]'s job.
//! * [`registry`] lists the seven checkers by name: `wiring`, `baseline_diff`, `regressions`,
//!   `tests_weakened`, `claims_vs_code`, `unfinished`, `exec`.
//! * [`run_lane`] runs a lane's checkers against a session and writes the results to the ledger:
//!   one `check` record per run (written as the core, the only writer the ledger lets write
//!   checks) and one `finding` per problem (written as the checker). A checker that cannot run
//!   writes `check_unavailable` and a `failed_to_run` check, never a clean one.
//!
//! "Lines added this session" is the working tree compared with the session's baseline: `git diff`
//! against the baseline commit, plus untracked files. A folder baseline holds only a content hash,
//! so a line-level diff is not possible there; see [`changes`].
//!
//! A finding **blocks** when its severity is [`Severity::Blocker`] (see [`blocks`]); every other
//! severity is something for the human to look at (NEEDS YOU) or a note.

pub mod baseline_diff;
pub mod changes;
pub mod claims_vs_code;
pub mod context;
pub mod exec;
pub mod firstrun;
pub mod mutation;
pub mod paths;
pub mod regressions;
pub mod release;
pub mod scan;
pub mod tests_weakened;
pub mod tree;
pub mod unfinished;
pub mod wiring;

use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::time::Duration;

use verdict_core::timefmt::now_rfc3339;
use verdict_core::{
    CheckOutcome, Finding, Lane, Ledger, Policy, Record, SessionParams, Severity, Tier, Writer,
    list_sessions, start_session,
};

pub use context::{CheckContext, ClaimRef, Unavailable};

/// The plugin contract for a deterministic checker.
pub trait Checker: Send + Sync {
    /// The name used in the policy and in the ledger.
    fn name(&self) -> &'static str;
    /// The proof tiers this checker can reach.
    fn tiers(&self) -> &'static [Tier];
    /// Does the policy want this checker in this lane?
    fn applies(&self, policy: &Policy, lane: Lane) -> bool;
    /// Run against the context. `Err` means "could not run": the runner records
    /// `check_unavailable`, never a clean check.
    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable>;
}

/// Names of the checkers this crate provides.
pub const CHECKER_NAMES: [&str; 10] = [
    "wiring",
    "baseline_diff",
    "regressions",
    "tests_weakened",
    "unfinished",
    "exec",
    "mutation",
    "firstrun",
    "release",
    "claims_vs_code",
];

/// The fixed running order. Evidence-gathering checkers first, `exec` before anything that needs
/// its results (mutation, later), `claims_vs_code` last. Names not listed run after `exec` and
/// before `claims_vs_code`.
const ORDER: [&str; 11] = [
    "wiring",
    "baseline_diff",
    "regressions",
    "tests_weakened",
    "unfinished",
    "exec",
    "mutation",
    "firstrun",
    "release",
    "claims_vs_code",
    "packet",
];

fn rank(name: &str) -> usize {
    match ORDER.iter().position(|n| *n == name) {
        Some(p) => p * 2,
        None => 5 * 2 + 1, // after exec, before mutation
    }
}

/// Order checker names for running: the core rule, stable for names with equal rank.
pub fn order_checkers(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    out.sort_by_key(|n| rank(n));
    out
}

/// Expand a lane's checker list from the policy: `*` means every checker in [`CHECKER_NAMES`],
/// `-name` removes one (wherever it appears in the list), any other name is kept as written (a
/// name this crate does not know becomes a `check_unavailable` record when run). The result is in
/// running order with no repeats.
pub fn expand_selection(selection: &[String]) -> Vec<String> {
    let mut chosen: Vec<String> = Vec::new();
    let mut removed: Vec<&str> = Vec::new();
    for item in selection {
        let item = item.trim();
        if item == "*" {
            for n in CHECKER_NAMES {
                if !chosen.iter().any(|c| c == n) {
                    chosen.push(n.to_string());
                }
            }
        } else if let Some(rest) = item.strip_prefix('-') {
            removed.push(rest.trim());
        } else if !item.is_empty() && !chosen.iter().any(|c| c == item) {
            chosen.push(item.to_string());
        }
    }
    chosen.retain(|c| !removed.contains(&c.as_str()));
    order_checkers(&chosen)
}

/// The expanded checker list for a lane, from the policy. A lane the policy does not mention
/// gets every checker.
pub fn lane_checkers(policy: &Policy, lane: Lane) -> Vec<String> {
    match lane_selection(policy, lane) {
        Some(sel) => expand_selection(&sel),
        None => expand_selection(&["*".to_string()]),
    }
}

fn lane_key(lane: Lane) -> &'static str {
    match lane {
        Lane::Low => "low",
        Lane::Standard => "standard",
        Lane::High => "high",
        Lane::Release => "release",
    }
}

fn lane_selection(policy: &Policy, lane: Lane) -> Option<Vec<String>> {
    policy
        .value
        .pointer(&format!("/lanes/{}/checkers", lane_key(lane)))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
}

/// True when the policy's list for `lane` includes `name` (or the policy has no list for it).
pub fn lane_selects(policy: &Policy, lane: Lane, name: &str) -> bool {
    match lane_selection(policy, lane) {
        Some(sel) => expand_selection(&sel).iter().any(|n| n == name),
        None => true,
    }
}

/// Does this finding block? Only a `Blocker` does; the rest force NEEDS YOU or are notes.
pub fn blocks(finding: &Finding) -> bool {
    finding.severity == Severity::Blocker
}

/// A checker that finds nothing. Wrapped around the real one by [`stubbed_registry`], to prove a
/// red fixture really depends on the checker.
pub struct Stubbed(pub Box<dyn Checker>);

impl Checker for Stubbed {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn tiers(&self) -> &'static [Tier] {
        self.0.tiers()
    }
    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        self.0.applies(policy, lane)
    }
    fn run(&self, _ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        Ok(Vec::new())
    }
}

/// The real checkers.
pub fn real_registry() -> Vec<Box<dyn Checker>> {
    vec![
        Box::new(wiring::Wiring),
        Box::new(baseline_diff::BaselineDiff),
        Box::new(regressions::Regressions),
        Box::new(tests_weakened::TestsWeakened),
        Box::new(unfinished::Unfinished),
        Box::new(exec::Exec),
        Box::new(mutation::Mutation),
        Box::new(firstrun::FirstRun),
        Box::new(release::Release),
        Box::new(claims_vs_code::ClaimsVsCode),
    ]
}

/// The same checkers, each replaced by a stub that returns no findings.
pub fn stubbed_registry() -> Vec<Box<dyn Checker>> {
    real_registry()
        .into_iter()
        .map(|c| Box::new(Stubbed(c)) as Box<dyn Checker>)
        .collect()
}

/// The checkers `run_lane` uses: the real ones, or (with the `stub-checkers` feature, which CI
/// uses to prove the red fixtures fail) the stubs.
pub fn registry() -> Vec<Box<dyn Checker>> {
    if cfg!(feature = "stub-checkers") {
        stubbed_registry()
    } else {
        real_registry()
    }
}

/// How to run a lane.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The lane being run.
    pub lane: Lane,
    /// The longest any one command may run (the policy's `exec.timeout` may lower it).
    pub timeout: Duration,
    /// When the whole run must be done (the Stop hook's configurable budget). A checker that has
    /// not started by then is recorded as unavailable, never as clean.
    pub deadline: Option<std::time::Instant>,
}

/// What one checker's run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSummary {
    /// The checker's name.
    pub name: String,
    /// Ran, failed to run, or skipped by policy.
    pub outcome: CheckOutcome,
    /// How many findings were written.
    pub findings: usize,
    /// Sequence number of the `check` record.
    pub check_seq: u64,
}

fn s(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run `checkers` (a lane's list, already expanded with [`expand_selection`]) against `session`,
/// writing the results to `ledger`. Uses the policy found for `root` (and the user's home).
pub fn run_lane(
    ledger: &mut Ledger,
    root: &Path,
    session: &str,
    checkers: &[String],
    opts: &RunOptions,
) -> Result<Vec<CheckSummary>, String> {
    run_lane_with_home(ledger, root, session, checkers, opts, None)
}

/// [`run_lane`] with the folder that holds `.verdict/policy.json` given (tests pass an empty one
/// so a developer's own global policy cannot change the result).
pub fn run_lane_with_home(
    ledger: &mut Ledger,
    root: &Path,
    session: &str,
    checkers: &[String],
    opts: &RunOptions,
    home: Option<&Path>,
) -> Result<Vec<CheckSummary>, String> {
    let (policy, policy_problem) = match Policy::load(root, home) {
        Ok(p) => (p, None),
        Err(e) => (Policy::builtin().map_err(s)?, Some(e.to_string())),
    };
    let ctx = CheckContext::from_ledger(ledger, root, session, opts.lane, opts.timeout, policy)?;
    // The snapshot stamp must describe the source the context is about to cache. It is taken
    // before the context warms its tree/changes, so the stamp can only be older than the cached
    // data, never newer. A check whose stamp differs from the tree at verdict time is stale, so an
    // edit made after this point can never be certified by evidence about the earlier snapshot.
    let tree_at_start = verdict_core::baseline::tree_state_of_ledger(ledger);
    ctx.warm();
    if let Some(problem) = &policy_problem {
        // The policy says which checkers run, what exec may run and what to exclude. If it cannot
        // be read, running under the built-in defaults would answer a different question, so no
        // checker runs: each one says why, and exec runs nothing.
        let mut summaries = Vec::new();
        for name in order_checkers(checkers) {
            let started = now_rfc3339();
            summaries.push(unavailable(
                ledger,
                session,
                &name,
                &started,
                &ctx.inputs_hash(&name),
                &tree_at_start,
                format!("policy could not be read: {problem}"),
            )?);
        }
        return Ok(summaries);
    }
    let available = registry();
    let existing = existing_ids(ledger, session)?;
    let mut seen_ids = existing;
    let mut summaries = Vec::new();

    for name in order_checkers(checkers) {
        ctx.reset_run_state();
        let started = now_rfc3339();
        if opts
            .deadline
            .is_some_and(|d| std::time::Instant::now() >= d)
        {
            summaries.push(unavailable(
                ledger,
                session,
                &name,
                &started,
                &ctx.inputs_hash(&name),
                &tree_at_start,
                "the run's time budget was used up before this checker could start".into(),
            )?);
            continue;
        }
        // The stamp is the pre-warm snapshot above: every checker in this run reads from that
        // snapshot (or a later lazy read, which can only be newer), so the recorded identity can
        // never be newer than the data inspected.
        let Some(checker) = available.iter().find(|c| c.name() == name) else {
            summaries.push(unavailable(
                ledger,
                session,
                &name,
                &started,
                &ctx.inputs_hash(&name),
                &tree_at_start,
                format!("no checker named {name} is installed in this build"),
            )?);
            continue;
        };
        if !checker.applies(&ctx.policy, opts.lane) {
            let seq = append_check(
                ledger,
                session,
                &name,
                Tier::Unverified,
                &ctx.inputs_hash(&name),
                &started,
                &tree_at_start,
                CheckOutcome::SkippedByPolicy,
                Some(format!(
                    "the policy's {} lane does not list {name}",
                    lane_key(opts.lane)
                )),
            )?;
            summaries.push(CheckSummary {
                name,
                outcome: CheckOutcome::SkippedByPolicy,
                findings: 0,
                check_seq: seq,
            });
            continue;
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| checker.run(&ctx)))
            .unwrap_or_else(|_| Err(Unavailable::new("the checker crashed (a bug in Verdict)")));
        match result {
            Err(u) => {
                let reason = u.reason;
                summaries.push(unavailable(
                    ledger,
                    session,
                    &name,
                    &started,
                    &ctx.inputs_hash(&name),
                    &tree_at_start,
                    reason,
                )?);
            }
            Ok(findings) => {
                let mut written = 0usize;
                for mut f in findings {
                    f.id = unique_id(&f.id, &mut seen_ids);
                    ledger
                        .append(Writer::Checker, Some(session), Record::Finding(f))
                        .map_err(s)?;
                    written += 1;
                }
                let mut detail = format!("{written} finding(s)");
                for n in ctx.notes() {
                    detail.push_str("; ");
                    detail.push_str(&n);
                }
                if let Some(t) = ctx.exec_tail() {
                    detail.push_str("; output tail: ");
                    detail.push_str(&t);
                }
                // A checker that says policy did not require it is recorded as skipped, never as
                // having run; a skip is not execution proof.
                if let Some(reason) = ctx.skip_reason() {
                    let seq = append_check(
                        ledger,
                        session,
                        &name,
                        Tier::Unverified,
                        &ctx.inputs_hash(&name),
                        &started,
                        &tree_at_start,
                        CheckOutcome::SkippedByPolicy,
                        Some(bounded(format!(
                            "{reason}; {written} finding(s) recorded separately"
                        ))),
                    )?;
                    summaries.push(CheckSummary {
                        name,
                        outcome: CheckOutcome::SkippedByPolicy,
                        findings: written,
                        check_seq: seq,
                    });
                    continue;
                }
                let tier = ctx
                    .reached_tier()
                    .or_else(|| checker.tiers().first().copied())
                    .unwrap_or(Tier::Static);
                write_receipts(
                    ledger,
                    session,
                    &ctx,
                    &started,
                    &tree_at_start,
                    &ctx.inputs_hash(&name),
                )?;
                let gaps = ctx.unestablished_reasons();
                if !gaps.is_empty() {
                    // Part of what this checker was asked could not be established. Its findings
                    // stay in the ledger, but the check reads as unavailable, never as clean.
                    let mut reason = format!(
                        "{written} finding(s) written, but not everything could be looked up: {}",
                        gaps.join("; ")
                    );
                    for n in ctx.notes() {
                        reason.push_str("; ");
                        reason.push_str(&n);
                    }
                    summaries.push(unavailable(
                        ledger,
                        session,
                        &name,
                        &started,
                        &ctx.inputs_hash(&name),
                        &tree_at_start,
                        reason,
                    )?);
                    continue;
                }
                let seq = append_check(
                    ledger,
                    session,
                    &name,
                    tier,
                    &ctx.inputs_hash(&name),
                    &started,
                    &tree_at_start,
                    CheckOutcome::Ran,
                    Some(bounded(detail)),
                )?;
                summaries.push(CheckSummary {
                    name,
                    outcome: CheckOutcome::Ran,
                    findings: written,
                    check_seq: seq,
                });
            }
        }
    }
    Ok(summaries)
}

/// One `check` record per command exec ran, written before the checker's own `check` record (the
/// latest record about a checker is the one that counts), so a reviewer can tell each command's
/// output apart: command, tier, how it ended, output hash and a bounded tail.
fn write_receipts(
    ledger: &mut Ledger,
    session: &str,
    ctx: &CheckContext,
    started: &str,
    tree: &Option<String>,
    inputs_hash: &str,
) -> Result<(), String> {
    for r in ctx.receipts() {
        let mut detail = format!(
            "receipt: `{}`; tier {}; {}; output hash {}",
            r.command,
            r.tier.as_str(),
            r.ended,
            r.output_hash
        );
        if let Some(p) = &r.problem {
            detail.push_str(&format!("; problem: {p}"));
        }
        if !r.tail.is_empty() {
            detail.push_str(&format!("; output tail: {}", r.tail));
        }
        append_check(
            ledger,
            session,
            "exec",
            r.tier,
            inputs_hash,
            started,
            tree,
            CheckOutcome::Ran,
            Some(bounded(detail)),
        )?;
    }
    Ok(())
}

fn bounded(mut text: String) -> String {
    const MAX: usize = 4000;
    if text.len() > MAX {
        let cut = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= MAX)
            .last()
            .unwrap_or(0);
        text.truncate(cut);
        text.push_str("...");
    }
    text
}

/// Ids already used by findings or hypotheses of the session.
fn existing_ids(
    ledger: &Ledger,
    session: &str,
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut ids = std::collections::BTreeSet::new();
    for r in ledger.records_for_session(session).map_err(s)? {
        match r.record {
            Record::Finding(f) | Record::Hypothesis(f) => {
                ids.insert(f.id);
            }
            _ => {}
        }
    }
    Ok(ids)
}

/// A checker gives the same finding the same id every run; a later run in the same session needs
/// its own ids (`<id>.r2`, `.r3`, ...), since ids are unique within a session.
fn unique_id(id: &str, seen: &mut std::collections::BTreeSet<String>) -> String {
    if seen.insert(id.to_string()) {
        return id.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{id}.r{n}");
        if seen.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn append_check(
    ledger: &mut Ledger,
    session: &str,
    name: &str,
    tier: Tier,
    inputs_hash: &str,
    started: &str,
    tree: &Option<String>,
    outcome: CheckOutcome,
    detail: Option<String>,
) -> Result<u64, String> {
    // The ledger lets only the core write `check` records, so the runner writes them as the core;
    // findings and `check_unavailable` are written as the checker.
    ledger
        .append(
            Writer::Core,
            Some(session),
            Record::Check {
                name: name.to_string(),
                tier,
                inputs_hash: inputs_hash.to_string(),
                started_at: started.to_string(),
                finished_at: now_rfc3339(),
                outcome,
                detail,
                tree_state: tree.clone(),
            },
        )
        .map(|a| a.seq)
        .map_err(s)
}

#[allow(clippy::too_many_arguments)]
fn unavailable(
    ledger: &mut Ledger,
    session: &str,
    name: &str,
    started: &str,
    inputs_hash: &str,
    tree: &Option<String>,
    reason: String,
) -> Result<CheckSummary, String> {
    ledger
        .append(
            Writer::Checker,
            Some(session),
            Record::CheckUnavailable {
                component: name.to_string(),
                reason: reason.clone(),
            },
        )
        .map_err(s)?;
    let seq = append_check(
        ledger,
        session,
        name,
        Tier::HonestRed,
        inputs_hash,
        started,
        tree,
        CheckOutcome::FailedToRun,
        Some(bounded(reason)),
    )?;
    Ok(CheckSummary {
        name: name.to_string(),
        outcome: CheckOutcome::FailedToRun,
        findings: 0,
        check_seq: seq,
    })
}

/// `vdt check --checkers a,b [--project <dir>] [--lane standard]`: run the named checkers on the
/// newest session of the project at `root` (or a new one), print one line per checker. Returns the
/// exit code (0 when every requested checker was run or skipped on purpose).
pub fn cli_check(
    root: &Path,
    checkers: &str,
    lane: &str,
    out: &mut dyn Write,
) -> Result<u8, String> {
    let lane = match lane {
        "low" => Lane::Low,
        "standard" => Lane::Standard,
        "high" => Lane::High,
        "release" => Lane::Release,
        other => {
            return Err(format!(
                "unknown lane {other:?} (use low, standard, high or release)"
            ));
        }
    };
    let selection: Vec<String> = checkers
        .split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    if selection.is_empty() {
        return Err("--checkers needs at least one name (or *)".into());
    }
    let names = expand_selection(&selection);
    let mut ledger = Ledger::open(root).map_err(s)?;
    let sessions = list_sessions(&ledger).map_err(s)?;
    let (session, is_new) = match sessions.last() {
        Some(info) => (info.id.clone(), false),
        None => {
            let params = SessionParams {
                agent: "vdt".into(),
                model: "none".into(),
                vendor: "none".into(),
                home: None,
            };
            (
                start_session(&mut ledger, root, &params).map_err(s)?.id,
                true,
            )
        }
    };
    eprintln!(
        "vdt check: {} session {session} in {}",
        if is_new { "new" } else { "newest" },
        root.display()
    );
    // The policy's own exec timeout governs a command, not a hardcoded three minutes.
    let timeout = Policy::load(root, None)
        .ok()
        .and_then(|p| {
            p.value
                .pointer("/exec/timeout")
                .and_then(serde_json::Value::as_str)
                .and_then(verdict_core::exec_approval::parse_duration)
        })
        .unwrap_or(Duration::from_secs(180));
    let opts = RunOptions {
        lane,
        timeout,
        deadline: None,
    };
    let summaries = run_lane(&mut ledger, root, &session, &names, &opts)?;
    for sm in &summaries {
        let outcome = match sm.outcome {
            CheckOutcome::Ran => "ran",
            CheckOutcome::FailedToRun => "could not run (check_unavailable)",
            CheckOutcome::SkippedByPolicy => "skipped by policy",
        };
        writeln!(
            out,
            "{:<16} {}, {} finding(s), check record {}",
            sm.name, outcome, sm.findings, sm.check_seq
        )
        .map_err(s)?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn star_and_minus_expand() {
        let all = expand_selection(&v(&["*"]));
        assert_eq!(all.len(), CHECKER_NAMES.len());
        assert_eq!(all.last().map(String::as_str), Some("claims_vs_code"));
        let some = expand_selection(&v(&["*", "-exec", "-unfinished"]));
        assert!(!some.contains(&"exec".to_string()));
        assert!(!some.contains(&"unfinished".to_string()));
        // `-name` works wherever it sits.
        let early = expand_selection(&v(&["-exec", "*"]));
        assert!(!early.contains(&"exec".to_string()));
    }

    #[test]
    fn order_puts_exec_before_later_checkers_and_claims_last() {
        let o = order_checkers(&v(&[
            "claims_vs_code",
            "mystery",
            "exec",
            "wiring",
            "regressions",
            "mutation",
        ]));
        assert_eq!(
            o,
            vec![
                "wiring",
                "regressions",
                "exec",
                "mystery",
                "mutation",
                "claims_vs_code"
            ]
        );
    }

    #[test]
    fn unknown_names_are_kept() {
        let o = expand_selection(&v(&["wiring", "regressions"]));
        assert_eq!(o, vec!["wiring", "regressions"]);
    }

    #[test]
    fn repeated_ids_get_a_run_suffix() {
        let mut seen = std::collections::BTreeSet::new();
        assert_eq!(unique_id("a.b.1", &mut seen), "a.b.1");
        assert_eq!(unique_id("a.b.1", &mut seen), "a.b.1.r2");
        assert_eq!(unique_id("a.b.1", &mut seen), "a.b.1.r3");
    }
}
