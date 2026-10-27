//! Verdict computation: one word from the ledger alone.
//!
//! The agent never writes a verdict. [`compute`] reads a session's records and says:
//!
//! * `FAILED`: a blocking finding is open.
//! * `PARTIAL`: a required checker did not run (and nothing is proven wrong).
//! * `UNVERIFIED`: nothing was checked.
//! * `NEEDS YOU`: nothing proven wrong, but something is the human's to settle.
//! * `VERIFIED (static|ran|browsed)`: the suffix is the highest proof tier reached, always shown.
//! * `CLEAR TO SHIP`: release lane only, with every condition of the design.
//!
//! Only things that are not the builder count: checker and desk findings and the human's
//! decisions. Claims, prompts and tool calls (all an agent or adapter can write) never improve a
//! verdict, and a hypothesis (a finding with no evidence pointer) counts for nothing. The core
//! writes the result with [`record_verdict`], as writer `core`.
//!
//! Precedence, strongest first: open blocking finding, required checker missing or unavailable,
//! nothing checked, something for the human, an open commit-anyway override, then clean.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

use crate::lane::{
    ALL_LANES, Classification, classify_lane, effective_lane, is_checker, lane_checkers,
    normalize_paths, touched_paths,
};
use crate::ledger::{Ledger, LedgerError};
use crate::model::{
    CheckOutcome, DecisionKind, Finding, Lane, Record, Severity, Tier, VerdictWord, Writer,
};
use crate::part1c::open_decisions;
use crate::policy::{Policy, PolicyError};
use crate::report_head::three_state;

/// Checkers whose findings never fail a commit; they force NEEDS YOU instead (the design's
/// "never blocks; forces NEEDS YOU").
pub const NON_BLOCKING_CHECKERS: [&str; 2] = ["baseline_diff", "tests_weakened"];

/// What the verdict needs to know that is not in the ledger: the lane the diff implies and the
/// checker list of every lane (so a human raise picks the right list).
#[derive(Debug, Clone)]
pub struct VerdictInput {
    /// The lane the session's diff implies (before any human raise).
    pub implied_lane: Lane,
    /// The expanded checker list of each lane.
    pub lane_checkers: BTreeMap<Lane, Vec<String>>,
    /// Policy version in force.
    pub policy_version: u32,
    /// Why the lane is what it is, for the report.
    pub lane_reasons: Vec<String>,
}

impl VerdictInput {
    /// Build the input from a policy and the lane the diff implies.
    pub fn from_policy(policy: &Policy, implied: Classification) -> VerdictInput {
        let mut lane_checkers_map = BTreeMap::new();
        for lane in ALL_LANES {
            lane_checkers_map.insert(lane, lane_checkers(policy, lane));
        }
        VerdictInput {
            implied_lane: implied.lane,
            lane_checkers: lane_checkers_map,
            policy_version: policy.version,
            lane_reasons: implied.reasons,
        }
    }
}

/// Whether a finding still counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingStatus {
    /// Open: it counts.
    Open,
    /// The human accepted it as not real (a decision record). That closes this one finding.
    AcceptedNotReal {
        /// The decision record.
        decision_seq: u64,
    },
    /// A checker re-run or the core withdrew it (a correction record).
    Resolved {
        /// The correction record.
        correction_seq: u64,
    },
}

/// A finding with its status.
#[derive(Debug, Clone, PartialEq)]
pub struct FindingView {
    /// Ledger sequence number.
    pub seq: u64,
    /// The finding.
    pub finding: Finding,
    /// Open or closed, and how.
    pub status: FindingStatus,
}

impl FindingView {
    /// True while it counts.
    pub fn is_open(&self) -> bool {
        self.status == FindingStatus::Open
    }

    /// The checker named by the id prefix (`wiring:src/a.rs:3`), if the id carries one.
    pub fn checker(&self) -> Option<&str> {
        checker_of(&self.finding)
    }
}

/// The checker a finding came from, read from its id: `<checker>:<anything>`. Ids without that
/// prefix have no known checker and are judged by severity alone.
pub fn checker_of(f: &Finding) -> Option<&str> {
    f.id.split_once(':')
        .map(|(c, _)| c)
        .filter(|c| is_checker(c))
}

/// The state of one named check in a session (the latest record about it wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckState {
    /// It ran, at this tier.
    Ran {
        /// Tier earned.
        tier: Tier,
        /// Record.
        seq: u64,
    },
    /// Policy said not to run it.
    Skipped {
        /// Why.
        detail: String,
        /// Record.
        seq: u64,
    },
    /// It could not run.
    Unavailable {
        /// Why.
        reason: String,
        /// Record.
        seq: u64,
    },
}

/// A commit-anyway decision as the verdict sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitAnywayView {
    /// Decision record.
    pub seq: u64,
    /// The staged tree it covers.
    pub tree: String,
    /// The findings it went past.
    pub finding_ids: Vec<String>,
    /// The reason the human typed.
    pub reason: String,
    /// True while any finding it went past is still open.
    pub open: bool,
}

/// The `choice` of a commit-anyway decision: which staged tree, and which findings it went past.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitAnywayChoice {
    /// The staged tree hash (`git write-tree`), or `none` outside a git repository.
    pub tree: String,
    /// The open blocking findings at the time.
    pub finding_ids: Vec<String>,
}

impl CommitAnywayChoice {
    /// The text stored as the decision's `choice` (JSON).
    pub fn encode(&self) -> String {
        serde_json::json!({"tree": self.tree, "findings": self.finding_ids}).to_string()
    }

    /// Read it back. A choice that is not this shape reads as a tree-less override of nothing.
    pub fn decode(choice: &str) -> CommitAnywayChoice {
        let v: Value = serde_json::from_str(choice).unwrap_or(Value::Null);
        CommitAnywayChoice {
            tree: v
                .get("tree")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            finding_ids: v
                .get("findings")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Everything about a session that does not depend on the policy.
#[derive(Debug, Clone)]
pub struct Analysis {
    /// The session.
    pub session: String,
    /// Findings, the latest record per id, oldest first.
    pub findings: Vec<FindingView>,
    /// Hypotheses (findings with no evidence pointer). They count for nothing.
    pub hypotheses: Vec<(u64, Finding)>,
    /// The latest state of each named check.
    pub checks: BTreeMap<String, CheckState>,
    /// Commit-anyway overrides.
    pub overrides: Vec<CommitAnywayView>,
    /// Components that could not run and are not checkers or the MCP tool surface (adapter, core,
    /// policy): the ledger may be missing records, so the verdict cannot be clean.
    pub plumbing_unavailable: Vec<(u64, String, String)>,
    /// Counts by tier of every check and finding record.
    pub counts_by_tier: BTreeMap<Tier, u64>,
}

fn in_scope(session: &str, s: &crate::ledger::Stored) -> bool {
    s.session_id.as_deref() == Some(session)
}

/// Read a session's records into an [`Analysis`].
pub fn analyze(ledger: &Ledger, session: &str) -> Result<Analysis, LedgerError> {
    let all = ledger.records()?;
    // Decisions also count when recorded for no session (project-wide).
    let applies = |s: &crate::ledger::Stored| in_scope(session, s) || s.session_id.is_none();

    let mut latest: BTreeMap<String, (u64, Finding)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut hypotheses = Vec::new();
    let mut checks: BTreeMap<String, CheckState> = BTreeMap::new();
    let mut plumbing = Vec::new();
    let mut counts: BTreeMap<Tier, u64> = BTreeMap::new();
    let mut accepted: BTreeMap<String, u64> = BTreeMap::new();
    let mut corrected: BTreeMap<u64, u64> = BTreeMap::new();
    let mut raw_overrides: Vec<(u64, CommitAnywayChoice, String)> = Vec::new();

    for s in &all {
        match &s.record {
            Record::Finding(f) if in_scope(session, s) => {
                *counts.entry(f.tier).or_default() += 1;
                if !latest.contains_key(&f.id) {
                    order.push(f.id.clone());
                }
                latest.insert(f.id.clone(), (s.seq, f.clone()));
            }
            Record::Hypothesis(f) if in_scope(session, s) => {
                hypotheses.push((s.seq, f.clone()));
            }
            Record::Check {
                name,
                tier,
                outcome,
                detail,
                ..
            } if in_scope(session, s) => {
                *counts.entry(*tier).or_default() += 1;
                let state = match outcome {
                    CheckOutcome::Ran => CheckState::Ran {
                        tier: *tier,
                        seq: s.seq,
                    },
                    CheckOutcome::SkippedByPolicy => CheckState::Skipped {
                        detail: detail.clone().unwrap_or_default(),
                        seq: s.seq,
                    },
                    CheckOutcome::FailedToRun => CheckState::Unavailable {
                        reason: detail.clone().unwrap_or_else(|| "failed to run".into()),
                        seq: s.seq,
                    },
                };
                checks.insert(name.clone(), state);
            }
            Record::CheckUnavailable { component, reason } if in_scope(session, s) => {
                if is_checker(component) {
                    checks.insert(
                        component.clone(),
                        CheckState::Unavailable {
                            reason: reason.clone(),
                            seq: s.seq,
                        },
                    );
                } else if !component.starts_with("verdict_") {
                    plumbing.push((s.seq, component.clone(), reason.clone()));
                }
            }
            Record::Decision {
                decision: DecisionKind::AcceptFindingNotReal,
                choice,
                who: Writer::Human,
                ..
            } if applies(s) => {
                accepted.entry(choice.clone()).or_insert(s.seq);
            }
            Record::Decision {
                decision: DecisionKind::CommitAnyway,
                choice,
                reason,
                who: Writer::Human,
            } if applies(s) => {
                raw_overrides.push((
                    s.seq,
                    CommitAnywayChoice::decode(choice),
                    reason.clone().unwrap_or_default(),
                ));
            }
            // A finding is resolved by the checker that wrote it, or by the core, never by a desk,
            // an agent or an adapter. (The human closes a finding with a decision instead.)
            Record::Correction {
                target_seq,
                replacement: None,
                ..
            } if in_scope(session, s) && matches!(s.writer, Writer::Core | Writer::Checker) => {
                corrected.entry(*target_seq).or_insert(s.seq);
            }
            _ => {}
        }
    }

    let findings: Vec<FindingView> = order
        .iter()
        .filter_map(|id| latest.get(id))
        .map(|(seq, f)| {
            let status = if let Some(c) = corrected.get(seq) {
                FindingStatus::Resolved { correction_seq: *c }
            } else if let Some(d) = accepted.get(&f.id) {
                FindingStatus::AcceptedNotReal { decision_seq: *d }
            } else {
                FindingStatus::Open
            };
            FindingView {
                seq: *seq,
                finding: f.clone(),
                status,
            }
        })
        .collect();

    let overrides = raw_overrides
        .into_iter()
        .map(|(seq, choice, reason)| {
            let open = choice
                .finding_ids
                .iter()
                .any(|id| findings.iter().any(|f| &f.finding.id == id && f.is_open()));
            CommitAnywayView {
                seq,
                tree: choice.tree,
                finding_ids: choice.finding_ids,
                reason,
                open,
            }
        })
        .collect();

    Ok(Analysis {
        session: session.to_string(),
        findings,
        hypotheses,
        checks,
        overrides,
        plumbing_unavailable: plumbing,
        counts_by_tier: counts,
    })
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Blocker => 0,
        Severity::Critical => 1,
        Severity::Major => 2,
        Severity::Minor => 3,
        Severity::Nit => 4,
    }
}

/// True if this open finding fails the verdict in `lane`.
///
/// * A deterministic finding (not from a desk) of severity major or worse fails it, except one
///   from `baseline_diff` or `tests_weakened`, which never block.
/// * In the release lane any open finding above minor fails it, desks included.
/// * Everything else, and every closed finding, does not.
pub fn is_blocking(view: &FindingView, lane: Lane) -> bool {
    if !view.is_open() {
        return false;
    }
    let f = &view.finding;
    let serious = severity_rank(f.severity) <= severity_rank(Severity::Major);
    if lane == Lane::Release && serious {
        return true;
    }
    let desk = f.judgment || f.tier == Tier::Judgment;
    let soft = view
        .checker()
        .is_some_and(|c| NON_BLOCKING_CHECKERS.contains(&c));
    serious && !desk && !soft
}

/// True if this open, non-blocking finding is the human's to settle (anything above a nit).
pub fn needs_attention(view: &FindingView, lane: Lane) -> bool {
    view.is_open() && !is_blocking(view, lane) && view.finding.severity != Severity::Nit
}

/// The proof tier a tier counts as for the VERIFIED suffix. Only tiers that read, ran or loaded
/// something count; a desk's judgment, an honest red and "unverified" never do. A hashed release
/// asset counts as `static` (bytes were read, nothing was run).
fn proof_rank(t: Tier) -> Option<(u8, Tier)> {
    match t {
        Tier::Static | Tier::Asset => Some((1, Tier::Static)),
        Tier::Ran => Some((2, Tier::Ran)),
        Tier::Browsed => Some((3, Tier::Browsed)),
        Tier::Judgment | Tier::HonestRed | Tier::Unverified => None,
    }
}

/// How a verdict word reads, suffix included: `VERIFIED (ran)`, `NEEDS YOU`, ...
pub fn word_label(word: VerdictWord, tier: Option<Tier>) -> String {
    match word {
        VerdictWord::Failed => "FAILED".into(),
        VerdictWord::NeedsYou => "NEEDS YOU".into(),
        VerdictWord::Verified => format!(
            "VERIFIED ({})",
            // The suffix is mandatory. A verified record with no tier reads as the weakest.
            tier.and_then(proof_rank)
                .map_or("static", |(_, t)| t.as_str())
        ),
        VerdictWord::ClearToShip => "CLEAR TO SHIP".into(),
        VerdictWord::Partial => "PARTIAL".into(),
        VerdictWord::Unverified => "UNVERIFIED".into(),
    }
}

/// An open finding, summarized for FIX FIRST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixItem {
    /// Ledger record.
    pub seq: u64,
    /// Finding id.
    pub id: String,
    /// Severity.
    pub severity: Severity,
    /// Title.
    pub title: String,
    /// Tier.
    pub tier: Tier,
    /// Where the evidence points.
    pub evidence: Vec<String>,
}

fn fix_item(v: &FindingView) -> FixItem {
    FixItem {
        seq: v.seq,
        id: v.finding.id.clone(),
        severity: v.finding.severity,
        title: v.finding.title.clone(),
        tier: v.finding.tier,
        evidence: v.finding.evidence.iter().map(|e| e.describe()).collect(),
    }
}

/// A computed verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Computed {
    /// The session.
    pub session: String,
    /// The word.
    pub word: VerdictWord,
    /// Highest proof tier reached (the VERIFIED suffix).
    pub highest_tier: Option<Tier>,
    /// The lane in force (the diff's lane, raised by the human if they did).
    pub lane: Lane,
    /// The lane the diff implies.
    pub implied_lane: Lane,
    /// Why the diff implies that lane.
    pub lane_reasons: Vec<String>,
    /// The checkers the lane requires.
    pub required: Vec<String>,
    /// Counts by tier.
    pub counts_by_tier: BTreeMap<Tier, u64>,
    /// What was not established.
    pub not_established: Vec<String>,
    /// Open blocking findings (FIX FIRST).
    pub fix_first: Vec<FixItem>,
    /// Things the human has to settle (YOURS TO SETTLE).
    pub yours_to_settle: Vec<String>,
    /// Policy version.
    pub policy_version: u32,
    /// The three-state line: read from code, build and tests, real app.
    pub three_state: [String; 3],
    /// True while a commit-anyway override is open.
    pub override_open: bool,
}

impl Computed {
    /// The word as it reads, suffix included.
    pub fn label(&self) -> String {
        word_label(self.word, self.highest_tier)
    }
}

/// The pieces of the report that depend on the lane only.
pub struct Sections {
    /// Open blocking findings.
    pub fix_first: Vec<FixItem>,
    /// Open findings that are the human's to settle.
    pub settle: Vec<FixItem>,
}

/// Split the open findings into FIX FIRST and YOURS TO SETTLE for `lane`.
pub fn sections(a: &Analysis, lane: Lane) -> Sections {
    Sections {
        fix_first: a
            .findings
            .iter()
            .filter(|f| is_blocking(f, lane))
            .map(fix_item)
            .collect(),
        settle: a
            .findings
            .iter()
            .filter(|f| needs_attention(f, lane))
            .map(fix_item)
            .collect(),
    }
}

/// Compute the verdict of `session` from the ledger. See the [module docs](self) for the rules.
pub fn compute(
    ledger: &Ledger,
    session: &str,
    input: &VerdictInput,
) -> Result<Computed, LedgerError> {
    let a = analyze(ledger, session)?;
    let lane = effective_lane(ledger, session, input.implied_lane)?;
    let required: Vec<String> = input.lane_checkers.get(&lane).cloned().unwrap_or_default();
    let sec = sections(&a, lane);
    let integrity = ledger.verify()?;

    // Which required checkers are not satisfied, and why.
    let mut missing: Vec<(String, String, bool)> = Vec::new(); // name, why, unavailable
    for name in &required {
        match a.checks.get(name) {
            Some(CheckState::Ran { .. }) | Some(CheckState::Skipped { .. }) => {}
            Some(CheckState::Unavailable { reason, .. }) => {
                missing.push((name.clone(), format!("could not run: {reason}"), true))
            }
            None => missing.push((name.clone(), "did not run".into(), false)),
        }
    }
    let highest = a
        .checks
        .values()
        .filter_map(|c| match c {
            CheckState::Ran { tier, .. } => proof_rank(*tier),
            _ => None,
        })
        .max_by_key(|(r, _)| *r)
        .map(|(_, t)| t);

    // Things for the human.
    let mut settle: Vec<String> = Vec::new();
    for v in &sec.settle {
        settle.push(format!(
            "[{:?}] {} ({}, {}; record {})",
            v.severity,
            v.title,
            v.tier.as_str(),
            v.evidence.first().map_or("no pointer", String::as_str),
            v.seq
        ));
    }
    for (seq, component, reason) in &a.plumbing_unavailable {
        settle.push(format!(
            "{component} could not run, so this session may be missing records: {reason} (record {seq})"
        ));
    }
    let translations: Vec<String> = open_decisions(ledger, Some(session))?
        .into_iter()
        .filter(|d| d.decision == DecisionKind::AcceptPromptTranslation)
        .map(|d| d.question)
        .collect();
    settle.extend(translations);
    if !integrity.is_ok() {
        settle.push(format!(
            "ledger integrity: {}",
            integrity
                .problems
                .first()
                .map_or("verification failed", |p| p.message.as_str())
        ));
    }
    let override_open = a.overrides.iter().any(|o| o.open);
    for o in a.overrides.iter().filter(|o| o.open) {
        settle.push(format!(
            "commit-anyway override open (record {}, reason: {}): resolve {} or accept it as not real",
            o.seq,
            o.reason,
            o.finding_ids.join(", ")
        ));
    }

    let mut word = if !sec.fix_first.is_empty() {
        VerdictWord::Failed
    } else if !missing.is_empty() {
        if highest.is_some() || missing.iter().any(|m| m.2) {
            VerdictWord::Partial
        } else {
            VerdictWord::Unverified
        }
    } else if highest.is_none() {
        VerdictWord::Unverified
    } else if !settle.is_empty() {
        VerdictWord::NeedsYou
    } else {
        VerdictWord::Verified
    };

    // CLEAR TO SHIP: the release lane, clean, firstrun attested in a browser, release coherence
    // run, and no override open. (A zero-above-nit bar is already part of "clean": an open minor
    // finding is NEEDS YOU, never VERIFIED.)
    let mut withheld: Vec<String> = Vec::new();
    if word == VerdictWord::Verified && lane == Lane::Release {
        let firstrun_ok = matches!(
            a.checks.get("firstrun"),
            Some(CheckState::Ran {
                tier: Tier::Browsed,
                ..
            })
        );
        let release_ok = matches!(a.checks.get("release"), Some(CheckState::Ran { .. }));
        if !firstrun_ok {
            withheld.push("firstrun was not attested at the browsed tier".to_string());
        }
        if !release_ok {
            withheld.push("release coherence did not run".to_string());
        }
        if withheld.is_empty() && !override_open {
            word = VerdictWord::ClearToShip;
        }
    }
    // Never clean on a broken ledger, whatever else is true.
    if !integrity.is_ok() && matches!(word, VerdictWord::Verified | VerdictWord::ClearToShip) {
        word = VerdictWord::NeedsYou;
    }

    // What was not established.
    let mut ne: Vec<String> = Vec::new();
    for (name, why, _) in &missing {
        ne.push(format!("{name}: {why}"));
    }
    for (name, c) in &a.checks {
        match c {
            CheckState::Skipped { detail, .. } => ne.push(format!(
                "{name}: skipped by policy{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(" ({detail})")
                }
            )),
            CheckState::Unavailable { reason, .. } if !required.contains(name) => {
                ne.push(format!("{name}: could not run: {reason}"))
            }
            _ => {}
        }
    }
    for (_, component, reason) in &a.plumbing_unavailable {
        ne.push(format!("{component}: could not run: {reason}"));
    }
    if !a.hypotheses.is_empty() {
        ne.push(format!(
            "{} hypothesis(es) with no evidence pointer count for nothing",
            a.hypotheses.len()
        ));
    }
    if highest.is_none() {
        ne.push("whether the work does what the prompts asked: no checker has run".into());
    }
    if !a
        .checks
        .values()
        .any(|c| matches!(c, CheckState::Ran { tier, .. } if *tier == Tier::Ran || *tier == Tier::Browsed))
    {
        ne.push("whether your build and tests pass: nothing was run".into());
    }
    if !a.checks.values().any(|c| {
        matches!(
            c,
            CheckState::Ran {
                tier: Tier::Browsed,
                ..
            }
        )
    }) {
        ne.push("whether your real app works: no page was loaded".into());
    }
    for w in &withheld {
        ne.push(format!("CLEAR TO SHIP withheld: {w}"));
    }
    if !integrity.is_ok() {
        ne.push("whether this ledger can be trusted: it failed verification".into());
    }

    let ts = three_state(&a, lane);
    Ok(Computed {
        session: session.to_string(),
        word,
        highest_tier: highest,
        lane,
        implied_lane: input.implied_lane,
        lane_reasons: input.lane_reasons.clone(),
        required,
        counts_by_tier: a.counts_by_tier.clone(),
        not_established: ne,
        fix_first: sec.fix_first,
        yours_to_settle: settle,
        policy_version: input.policy_version,
        three_state: ts,
        override_open,
    })
}

/// Write the verdict record (writer `core`). Returns its sequence number.
pub fn record_verdict(ledger: &mut Ledger, c: &Computed) -> Result<u64, LedgerError> {
    let a = ledger.append(
        Writer::Core,
        Some(&c.session),
        Record::Verdict {
            word: c.word,
            highest_tier: c.highest_tier,
            counts_by_tier: c.counts_by_tier.clone(),
            not_established: c.not_established.clone(),
            policy_version: c.policy_version,
            lane: Some(c.lane),
        },
    )?;
    Ok(a.seq)
}

/// Everything needed to compute a verdict for the project at `root`, read from disk: the policy
/// (falling back to the built-in default if the files are unusable, and saying so), the changed
/// paths and the lane they imply.
pub struct Live {
    /// The computed input.
    pub input: VerdictInput,
    /// The policy.
    pub policy: Policy,
    /// Set when the policy files were unusable and the built-in default was used.
    pub policy_warning: Option<String>,
    /// The changed paths the lane was classified from.
    pub paths: Vec<String>,
    /// Problems with blast globs.
    pub warnings: Vec<String>,
}

/// Load the policy for `root` and classify the session's lane from what it changed.
pub fn live_input(
    root: &Path,
    ledger: &Ledger,
    session: &str,
    home: Option<&Path>,
) -> Result<Live, LedgerError> {
    let (policy, policy_warning) = load_policy(root, home)?;
    let paths = normalize_paths(&policy, root, &touched_paths(root, ledger, session)?);
    let class = classify_lane(&policy, &paths);
    let warnings = class.warnings.clone();
    Ok(Live {
        input: VerdictInput::from_policy(&policy, class),
        policy,
        policy_warning,
        paths,
        warnings,
    })
}

/// Load the effective policy, or the built-in default with a warning when the files are unusable
/// (fail open on the tool, never on the verdict).
pub fn load_policy(
    root: &Path,
    home: Option<&Path>,
) -> Result<(Policy, Option<String>), LedgerError> {
    match Policy::load(root, home) {
        Ok(p) => Ok((p, None)),
        Err(e) => Policy::builtin()
            .map(|p| (p, Some(e.to_string())))
            .map_err(|e2: PolicyError| {
                LedgerError::Invalid(format!("no usable policy: {e}; {e2}"))
            }),
    }
}

/// Compute the verdict of `session` for the project at `root`, without writing anything.
pub fn compute_live(
    root: &Path,
    ledger: &Ledger,
    session: &str,
    home: Option<&Path>,
) -> Result<Computed, LedgerError> {
    let live = live_input(root, ledger, session, home)?;
    compute(ledger, session, &live.input)
}

/// Compute and write the verdict record. When `only_if_changed`, an identical latest verdict is
/// not written again. Returns the computed verdict and the new record's number (if one was written).
pub fn compute_and_record(
    root: &Path,
    ledger: &mut Ledger,
    session: &str,
    home: Option<&Path>,
    only_if_changed: bool,
) -> Result<(Computed, Option<u64>), LedgerError> {
    let c = compute_live(root, ledger, session, home)?;
    if only_if_changed {
        let same = ledger
            .records_for_session(session)?
            .into_iter()
            .rev()
            .find_map(|s| match s.record {
                Record::Verdict {
                    word,
                    highest_tier,
                    counts_by_tier,
                    not_established,
                    policy_version,
                    lane,
                } => Some(
                    word == c.word
                        && highest_tier == c.highest_tier
                        && counts_by_tier == c.counts_by_tier
                        && not_established == c.not_established
                        && policy_version == c.policy_version
                        && lane == Some(c.lane),
                ),
                _ => None,
            })
            .unwrap_or(false);
        if same {
            return Ok((c, None));
        }
    }
    let seq = record_verdict(ledger, &c)?;
    Ok((c, Some(seq)))
}

/// The report for a session of the project at `root`, with the verdict computed now (nothing is
/// written). This is what `vdt report`, the localhost page and the MCP server show, so they all
/// say the same thing. `session` is the one asked for, else the most recent.
pub fn report_for_project(
    root: &Path,
    ledger: &Ledger,
    session: Option<&str>,
    home: Option<&Path>,
) -> Result<String, LedgerError> {
    match crate::part1c::resolve_session(ledger, session)? {
        Some(id) => {
            let c = compute_live(root, ledger, &id, home)?;
            crate::report::render_report_with(ledger, Some(&id), Some(&c))
        }
        None => crate::report::render_report(ledger, session),
    }
}
