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
    ALL_LANES, Bar, Classification, classify_lane, effective_lane, is_checker,
    lane_bar_with_warning, lane_checkers, lane_desks, normalize_paths, touched_paths_checked,
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
    /// The expanded desk list of each lane. A required desk with no `desk:<name>` receipt on the
    /// code being judged makes the verdict PARTIAL, never clean (see [`compute`]).
    pub lane_desks: BTreeMap<Lane, Vec<String>>,
    /// The bar each lane declares. In the release lane it decides what "zero findings above nit"
    /// means.
    pub lane_bars: BTreeMap<Lane, Bar>,
    /// True when the project's first run is a web walk that needs browser evidence; false when the
    /// project declares it has no page to load (`firstrun.web: false`).
    pub firstrun_web: bool,
    /// Policy problems found while reading the lane configuration (an unknown bar, a bad glob).
    pub policy_warnings: Vec<String>,
    /// Policy version in force.
    pub policy_version: u32,
    /// Why the lane is what it is, for the report.
    pub lane_reasons: Vec<String>,
    /// The state of the code now ([`crate::baseline::tree_state`]). A check counts only if it ran
    /// on exactly this state; `None` (not known) means no check counts as current.
    pub current_tree: Option<String>,
    /// Set when the session's diff could not be read (why). The lane was then set to the highest
    /// one, and the verdict cannot be clean.
    pub diff_unavailable: Option<String>,
    /// Evidence pointers of the session's findings whose hash no longer matches the tree (or whose
    /// file or line is gone), as plain lines. Filled in by [`live_input`], which has the tree.
    pub stale_pointers: Vec<String>,
    /// Set when the rules in force now are not the rules the session started with, and no human
    /// has accepted the new ones (why). The verdict then cannot be clean.
    pub policy_changed: Option<String>,
}

impl VerdictInput {
    /// Build the input from a policy and the lane the diff implies. The code state is left unknown
    /// (set [`VerdictInput::current_tree`]); [`live_input`] fills it in from the disk.
    pub fn from_policy(policy: &Policy, implied: Classification) -> VerdictInput {
        let mut lane_checkers_map = BTreeMap::new();
        let mut lane_desks_map = BTreeMap::new();
        let mut lane_bars = BTreeMap::new();
        let mut warnings = implied.warnings.clone();
        for lane in ALL_LANES {
            lane_checkers_map.insert(lane, lane_checkers(policy, lane));
            lane_desks_map.insert(lane, lane_desks(policy, lane));
            let (bar, warning) = lane_bar_with_warning(policy, lane);
            lane_bars.insert(lane, bar);
            warnings.extend(warning);
        }
        let firstrun_web = policy
            .value
            .pointer("/firstrun/web")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        VerdictInput {
            implied_lane: implied.lane,
            lane_checkers: lane_checkers_map,
            lane_desks: lane_desks_map,
            lane_bars,
            firstrun_web,
            policy_warnings: warnings,
            policy_version: policy.version,
            lane_reasons: implied.reasons,
            current_tree: None,
            diff_unavailable: None,
            stale_pointers: Vec::new(),
            policy_changed: None,
        }
    }
}

/// How a check that ran on code that has since changed reads.
pub const STALE_CHECK: &str = "not established: the code changed after this check ran";

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
        /// The code state it ran on (`None` in records written before that was kept).
        tree: Option<String>,
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
    /// The session it was recorded in (`None`: project-wide).
    pub session: Option<String>,
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

/// The identity of a finding's evidence: blake3 over its evidence pointers (paths, lines and
/// content hashes). Two findings with the same id and the same evidence identity are the same
/// occurrence of the same problem.
pub fn evidence_identity(f: &Finding) -> String {
    let json = serde_json::to_string(&f.evidence).unwrap_or_default();
    blake3::hash(json.as_bytes()).to_hex().to_string()
}

/// The `choice` of an accept-as-not-real decision: exactly which occurrence of a finding it closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptChoice {
    /// The finding id.
    pub finding_id: String,
    /// The finding's record number, when the decision names it.
    pub seq: Option<u64>,
    /// The finding's [`evidence_identity`], when the decision names it.
    pub evidence: Option<String>,
}

impl AcceptChoice {
    /// The choice that closes the finding recorded at `seq`.
    pub fn for_finding(seq: u64, f: &Finding) -> AcceptChoice {
        AcceptChoice {
            finding_id: f.id.clone(),
            seq: Some(seq),
            evidence: Some(evidence_identity(f)),
        }
    }

    /// The text stored as the decision's `choice` (JSON).
    pub fn encode(&self) -> String {
        serde_json::json!({"finding": self.finding_id, "seq": self.seq, "evidence": self.evidence})
            .to_string()
    }

    /// Read it back. A choice that is not this shape is an older decision that named only the id.
    pub fn decode(choice: &str) -> AcceptChoice {
        if let Ok(v) = serde_json::from_str::<Value>(choice)
            && let Some(id) = v.get("finding").and_then(Value::as_str)
        {
            return AcceptChoice {
                finding_id: id.to_string(),
                seq: v.get("seq").and_then(Value::as_u64),
                evidence: v
                    .get("evidence")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            };
        }
        AcceptChoice {
            finding_id: choice.to_string(),
            seq: None,
            evidence: None,
        }
    }

    /// True when it names an exact occurrence (record number or evidence), not only an id.
    pub fn is_exact(&self) -> bool {
        self.seq.is_some() || self.evidence.is_some()
    }

    /// Whether this decision (recorded at `decision_seq`) closes the finding recorded at `seq`.
    ///
    /// * It names that record: yes.
    /// * It names the same id with the same evidence (the same problem reported again): yes.
    /// * Same id, different evidence: no, that is a new occurrence and it is open.
    /// * An older decision that named only the id closes the occurrence that existed when the
    ///   human decided, never one recorded later.
    pub fn closes(&self, seq: u64, f: &Finding, decision_seq: u64) -> bool {
        if f.id != self.finding_id {
            return false;
        }
        if self.seq == Some(seq) {
            return true;
        }
        match &self.evidence {
            Some(e) => *e == evidence_identity(f),
            None => self.seq.is_none() && seq < decision_seq,
        }
    }
}

/// A human accept-as-not-real decision.
pub(crate) struct Accept {
    decision_seq: u64,
    session: Option<String>,
    choice: AcceptChoice,
}

impl Accept {
    /// Whether this decision closes the finding recorded at `seq` in `session`. An exact decision
    /// applies wherever its occurrence is; an older id-only one applies in its own session, or
    /// everywhere when it was recorded for no session.
    pub(crate) fn applies(&self, seq: u64, f: &Finding, session: Option<&str>) -> bool {
        (self.choice.is_exact() || self.session.is_none() || self.session.as_deref() == session)
            && self.choice.closes(seq, f, self.decision_seq)
    }
}

/// The finding recorded at `seq`, anywhere in the project, with its status in its own session.
/// `None` when that record is not a finding, or a later record replaced it under the same id.
pub fn finding_view_at(all: &[crate::ledger::Stored], seq: u64) -> Option<FindingView> {
    let rec = all.iter().find(|s| s.seq == seq)?;
    if !matches!(rec.record, Record::Finding(_)) {
        return None;
    }
    let accepts = accepts_of(all);
    finding_views(all, rec.session_id.as_deref(), &accepts)
        .into_iter()
        .find(|v| v.seq == seq)
}

pub(crate) fn accepts_of(all: &[crate::ledger::Stored]) -> Vec<Accept> {
    all.iter()
        .filter_map(|s| match &s.record {
            Record::Decision {
                decision: DecisionKind::AcceptFindingNotReal,
                choice,
                who: Writer::Human,
                ..
            } => Some(Accept {
                decision_seq: s.seq,
                session: s.session_id.clone(),
                choice: AcceptChoice::decode(choice),
            }),
            _ => None,
        })
        .collect()
}

/// The findings of one session (the latest record per id, oldest first) with their status.
///
/// A decision that names an exact occurrence applies wherever that occurrence is; an older
/// id-only decision applies in its own session, or everywhere when it was recorded for no session.
fn finding_views(
    all: &[crate::ledger::Stored],
    session: Option<&str>,
    accepts: &[Accept],
) -> Vec<FindingView> {
    let mut latest: BTreeMap<String, (u64, Finding)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut corrected: BTreeMap<u64, u64> = BTreeMap::new();
    for s in all.iter().filter(|s| s.session_id.as_deref() == session) {
        match &s.record {
            Record::Finding(f) => {
                if !latest.contains_key(&f.id) {
                    order.push(f.id.clone());
                }
                latest.insert(f.id.clone(), (s.seq, f.clone()));
            }
            // A finding is resolved by the checker that wrote it, or by the core, never by a desk,
            // an agent or an adapter. (The human closes a finding with a decision instead.)
            Record::Correction {
                target_seq,
                replacement: None,
                ..
            } if matches!(s.writer, Writer::Core | Writer::Checker) => {
                corrected.entry(*target_seq).or_insert(s.seq);
            }
            _ => {}
        }
    }
    order
        .iter()
        .filter_map(|id| latest.get(id))
        .map(|(seq, f)| {
            let accepted = accepts.iter().find(|a| a.applies(*seq, f, session));
            let status = if let Some(c) = corrected.get(seq) {
                FindingStatus::Resolved { correction_seq: *c }
            } else if let Some(a) = accepted {
                FindingStatus::AcceptedNotReal {
                    decision_seq: a.decision_seq,
                }
            } else {
                FindingStatus::Open
            };
            FindingView {
                seq: *seq,
                finding: f.clone(),
                status,
            }
        })
        .collect()
}

/// Whether the finding id a commit-anyway override went past is still open anywhere in the
/// project. Its latest occurrence decides: open unless it was resolved, accepted as not real, or
/// its checker ran again later (in any session) without reporting it.
fn override_finding_open(
    all: &[crate::ledger::Stored],
    accepts: &[Accept],
    id: &str,
    cache: &mut BTreeMap<Option<String>, Vec<FindingView>>,
) -> bool {
    let Some(last) = all
        .iter()
        .rev()
        .find(|s| matches!(&s.record, Record::Finding(f) if f.id == id))
    else {
        return false;
    };
    let views = cache
        .entry(last.session_id.clone())
        .or_insert_with(|| finding_views(all, last.session_id.as_deref(), accepts));
    let Some(view) = views.iter().find(|v| v.finding.id == id) else {
        return false;
    };
    if !view.is_open() {
        return false;
    }
    if let Some(checker) = view.checker() {
        let is_check_of = |s: &crate::ledger::Stored, ran_only: bool| {
            matches!(&s.record, Record::Check { name, outcome, .. }
                if name == checker && (!ran_only || *outcome == CheckOutcome::Ran))
        };
        // The run that reported it writes its check record right after its findings.
        let reporting = all
            .iter()
            .find(|s| s.seq > view.seq && s.session_id == last.session_id && is_check_of(s, false));
        if let Some(r) = reporting
            && all.iter().any(|s| s.seq > r.seq && is_check_of(s, true))
        {
            return false;
        }
    }
    true
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

    let mut hypotheses = Vec::new();
    let mut checks: BTreeMap<String, CheckState> = BTreeMap::new();
    let mut plumbing = Vec::new();
    let mut counts: BTreeMap<Tier, u64> = BTreeMap::new();
    // A later successful check for the same component supersedes an earlier unavailability, so a
    // re-run after fixing the cause does not leave the old gap in the verdict for ever.
    let latest_ran: BTreeMap<&str, u64> = all
        .iter()
        .filter(|s| in_scope(session, s))
        .filter_map(|s| match &s.record {
            Record::Check {
                name,
                outcome: CheckOutcome::Ran,
                ..
            } => Some((name.as_str(), s.seq)),
            _ => None,
        })
        .collect();
    // Commit-anyway overrides from every session: an override stays open project-wide.
    let mut raw_overrides: Vec<(u64, Option<String>, CommitAnywayChoice, String)> = Vec::new();

    for s in &all {
        match &s.record {
            Record::Finding(f) if in_scope(session, s) => {
                *counts.entry(f.tier).or_default() += 1;
            }
            Record::Hypothesis(f) if in_scope(session, s) => {
                hypotheses.push((s.seq, f.clone()));
            }
            Record::Check {
                name,
                tier,
                outcome,
                detail,
                tree_state,
                ..
            } if in_scope(session, s) => {
                *counts.entry(*tier).or_default() += 1;
                let state = match outcome {
                    CheckOutcome::Ran => CheckState::Ran {
                        tier: *tier,
                        seq: s.seq,
                        tree: tree_state.clone(),
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
                if latest_ran
                    .get(component.as_str())
                    .is_some_and(|seq| *seq > s.seq)
                {
                    continue;
                }
                if is_checker(component) {
                    checks.insert(
                        component.clone(),
                        CheckState::Unavailable {
                            reason: reason.clone(),
                            seq: s.seq,
                        },
                    );
                } else if !component.starts_with("verdict_")
                    && component != crate::lane::LANE_COMPONENT
                {
                    // (Lane classification is judged live, from `VerdictInput::diff_unavailable`.)
                    // Each component is listed once: its latest record.
                    plumbing.retain(|(_, c, _): &(u64, String, String)| c != component);
                    plumbing.push((s.seq, component.clone(), reason.clone()));
                }
            }
            Record::Decision {
                decision: DecisionKind::CommitAnyway,
                choice,
                reason,
                who: Writer::Human,
            } => {
                raw_overrides.push((
                    s.seq,
                    s.session_id.clone(),
                    CommitAnywayChoice::decode(choice),
                    reason.clone().unwrap_or_default(),
                ));
            }
            _ => {}
        }
    }

    let accepts = accepts_of(&all);
    let findings = finding_views(&all, Some(session), &accepts);

    let mut cache: BTreeMap<Option<String>, Vec<FindingView>> = BTreeMap::new();
    cache.insert(Some(session.to_string()), findings.clone());
    let overrides = raw_overrides
        .into_iter()
        .filter_map(|(seq, sid, choice, reason)| {
            let open = choice
                .finding_ids
                .iter()
                .any(|id| override_finding_open(&all, &accepts, id, &mut cache));
            // This session's own (and project-wide) overrides always show; another session's
            // only while it is open.
            let here = sid.is_none() || sid.as_deref() == Some(session);
            (here || open).then_some(CommitAnywayView {
                seq,
                session: sid,
                tree: choice.tree,
                finding_ids: choice.finding_ids,
                reason,
                open,
            })
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
    is_blocking_with(view, lane, Bar::ZeroAboveNit)
}

/// [`is_blocking`] under a lane's declared bar.
pub fn is_blocking_with(view: &FindingView, lane: Lane, bar: Bar) -> bool {
    if !view.is_open() {
        return false;
    }
    let f = &view.finding;
    let serious = severity_rank(f.severity) <= severity_rank(Severity::Major);
    if lane == Lane::Release {
        return match bar {
            Bar::ZeroAboveNit => serious,
            Bar::BlockersOnly => f.severity == Severity::Blocker,
        };
    }
    let desk = f.judgment || f.tier == Tier::Judgment;
    let soft = view
        .checker()
        .is_some_and(|c| NON_BLOCKING_CHECKERS.contains(&c));
    serious && !desk && !soft
}

/// True if this open, non-blocking finding is the human's to settle (anything above a nit).
pub fn needs_attention(view: &FindingView, lane: Lane) -> bool {
    needs_attention_with(view, lane, Bar::ZeroAboveNit)
}

/// [`needs_attention`] under a lane's declared bar.
pub fn needs_attention_with(view: &FindingView, lane: Lane, bar: Bar) -> bool {
    view.is_open() && !is_blocking_with(view, lane, bar) && view.finding.severity != Severity::Nit
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
    sections_with(a, lane, Bar::ZeroAboveNit)
}

/// [`sections`] under a lane's declared bar.
pub fn sections_with(a: &Analysis, lane: Lane, bar: Bar) -> Sections {
    Sections {
        fix_first: a
            .findings
            .iter()
            .filter(|f| is_blocking_with(f, lane, bar))
            .map(fix_item)
            .collect(),
        settle: a
            .findings
            .iter()
            .filter(|f| needs_attention_with(f, lane, bar))
            .map(fix_item)
            .collect(),
    }
}

/// Open desk findings that disagree about the same claim. Desks are independent: both stay, and
/// the report shows the tension rather than averaging them.
pub fn dissent(a: &Analysis) -> Vec<String> {
    let open: Vec<&FindingView> = a
        .findings
        .iter()
        .filter(|f| f.is_open() && (f.finding.judgment || f.finding.tier == Tier::Judgment))
        .collect();
    let mut out = Vec::new();
    for (i, x) in open.iter().enumerate() {
        for y in open.iter().skip(i + 1) {
            let same_claim =
                x.finding.claim_seq.is_some() && x.finding.claim_seq == y.finding.claim_seq;
            let disagree = x.finding.severity != y.finding.severity;
            if same_claim && disagree {
                let desk = |f: &FindingView| {
                    checker_of(&f.finding)
                        .unwrap_or("a desk")
                        .trim_start_matches("desk:")
                        .to_string()
                };
                out.push(format!(
                    "{} desk: \"{}\" (record {}) and {} desk: \"{}\" (record {}) disagree about claim record {}; both are kept",
                    desk(x),
                    x.finding.title,
                    x.seq,
                    desk(y),
                    y.finding.title,
                    y.seq,
                    x.finding.claim_seq.unwrap_or(0)
                ));
            }
        }
    }
    out
}

/// Compute the verdict of `session` from the ledger. See the [module docs](self) for the rules.
pub fn compute(
    ledger: &Ledger,
    session: &str,
    input: &VerdictInput,
) -> Result<Computed, LedgerError> {
    let mut a = analyze(ledger, session)?;
    let lane = effective_lane(ledger, session, input.implied_lane)?;
    let bar = input
        .lane_bars
        .get(&lane)
        .copied()
        .unwrap_or(Bar::ZeroAboveNit);
    // A checker and a required desk are both requirements of the lane. A missing desk receipt is
    // exactly as serious as a missing checker: nothing can be computed as clean around it.
    let mut required: Vec<String> = input.lane_checkers.get(&lane).cloned().unwrap_or_default();
    for desk in input.lane_desks.get(&lane).cloned().unwrap_or_default() {
        required.push(format!("desk:{desk}"));
    }
    let sec = sections_with(&a, lane, bar);
    let integrity = ledger.verify()?;

    // A check counts only for the code it ran on. One that ran on code that has changed since, or
    // that does not say what code it ran on, proves nothing about the code now; from here on it is
    // treated as not run (and the verdict can be no better than PARTIAL).
    let current = input.current_tree.as_deref();
    let stale: Vec<String> = a
        .checks
        .iter()
        .filter_map(|(name, c)| match c {
            CheckState::Ran { tier, tree, .. }
                if (proof_rank(*tier).is_some() || required.contains(name))
                    && (current.is_none() || tree.as_deref() != current) =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    a.checks.retain(|name, _| !stale.contains(name));

    // Which required checkers are not satisfied, and why.
    let mut missing: Vec<(String, String, bool)> = Vec::new(); // name, why, unavailable
    for name in &required {
        match a.checks.get(name) {
            Some(CheckState::Ran { .. }) | Some(CheckState::Skipped { .. }) => {}
            Some(CheckState::Unavailable { reason, .. }) => {
                missing.push((name.clone(), format!("could not run: {reason}"), true))
            }
            None if stale.contains(name) => missing.push((name.clone(), STALE_CHECK.into(), true)),
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
    if let Some(why) = &input.diff_unavailable {
        settle.push(format!(
            "lane classification could not run: {why}. The lane was set to {} so nothing is under-checked; look at what changed yourself",
            crate::lane::lane_name(input.implied_lane)
        ));
    }
    if let Some(why) = &input.policy_changed {
        settle.push(why.clone());
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
            "commit-anyway override open (record {}{}, reason: {}): resolve {} or accept it as not real",
            o.seq,
            override_origin(o, session),
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
        // The project declares whether its first run walks a page (`firstrun.web`, the default) or
        // is command evidence only. A web first run must reach the browsed tier; a declared
        // non-web first run is satisfied by its commands' `ran` evidence.
        let firstrun_ok = match a.checks.get("firstrun") {
            Some(CheckState::Ran {
                tier: Tier::Browsed,
                ..
            }) => true,
            Some(CheckState::Ran {
                tier: Tier::Ran, ..
            }) => !input.firstrun_web,
            _ => false,
        };
        let release_ok = matches!(a.checks.get("release"), Some(CheckState::Ran { .. }));
        if !firstrun_ok {
            withheld.push(if input.firstrun_web {
                "firstrun was not attested at the browsed tier (this project's first run is a web walk)".to_string()
            } else {
                "firstrun did not record its command evidence".to_string()
            });
        }
        if !release_ok {
            withheld.push("release coherence did not run".to_string());
        }
        if withheld.is_empty() && !override_open {
            word = VerdictWord::ClearToShip;
        }
    }
    // Never clean on rules that changed under the session until a human accepts them.
    if input.policy_changed.is_some()
        && matches!(word, VerdictWord::Verified | VerdictWord::ClearToShip)
    {
        word = VerdictWord::NeedsYou;
    }
    // Never clean on a broken ledger, whatever else is true.
    if !integrity.is_ok() && matches!(word, VerdictWord::Verified | VerdictWord::ClearToShip) {
        word = VerdictWord::NeedsYou;
    }
    // A check that ran on other code caps the verdict at PARTIAL.
    if !stale.is_empty()
        && matches!(
            word,
            VerdictWord::Verified | VerdictWord::ClearToShip | VerdictWord::NeedsYou
        )
    {
        word = VerdictWord::Partial;
    }

    // What was not established.
    let mut ne: Vec<String> = Vec::new();
    for (name, why, _) in &missing {
        ne.push(format!("{name}: {why}"));
    }
    for name in stale.iter().filter(|n| !required.contains(n)) {
        ne.push(format!("{name}: {STALE_CHECK}"));
    }
    if let Some(why) = &input.diff_unavailable {
        ne.push(format!("lane classification: could not run: {why}"));
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
    for w in &input.policy_warnings {
        ne.push(format!("policy: {w}"));
    }
    for s in &input.stale_pointers {
        ne.push(s.clone());
    }
    if !a.hypotheses.is_empty() {
        ne.push(format!(
            "{} hypothesis(es) with no evidence pointer count for nothing",
            a.hypotheses.len()
        ));
    }
    if highest.is_none() && stale.is_empty() {
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
    if input.policy_changed.is_some() {
        ne.push(
            "whether the checks used the rules the session started with: the rules changed".into(),
        );
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

/// `", from session <id>"` for an override recorded in another session, else nothing.
pub(crate) fn override_origin(o: &CommitAnywayView, session: &str) -> String {
    match o.session.as_deref() {
        Some(s) if s != session => format!(", from session {s}"),
        _ => String::new(),
    }
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
    let touched = touched_paths_checked(root, ledger, session)?;
    let paths = normalize_paths(&policy, root, &touched.paths);
    let mut class = classify_lane(&policy, &paths);
    if let Some(why) = &touched.unreadable {
        // An unreadable diff never lowers the lane: classify at the highest lane there is.
        class.lane = Lane::Release;
        class.reasons.insert(
            0,
            format!(
                "the session's diff could not be read ({why}), so the lane is the highest: release"
            ),
        );
    }
    let warnings = class.warnings.clone();
    let mut input = VerdictInput::from_policy(&policy, class);
    input.current_tree = crate::baseline::tree_state(root);
    input.diff_unavailable = touched.unreadable;
    input.stale_pointers = stale_pointer_lines(ledger, session, root);
    input.policy_changed = policy_change(ledger, session, &policy)?;
    Ok(Live {
        input,
        policy,
        policy_warning,
        paths,
        warnings,
    })
}

/// Whether the rules changed during `session`: the session records the policy hash it started
/// with, and the agent must not weaken the rules after that (design.md: the agent cannot "edit the
/// policy"). Returns why, or `None` when the rules are the ones the session started with or a human
/// accepted exactly these rules with an `accept_policy` decision in this session.
pub fn policy_change(
    ledger: &Ledger,
    session: &str,
    policy: &Policy,
) -> Result<Option<String>, LedgerError> {
    let mut started: Option<String> = None;
    let mut accepted: Vec<String> = Vec::new();
    for s in ledger.records_for_session(session)? {
        match &s.record {
            Record::Session { policy_hash, .. } if started.is_none() => {
                started = Some(policy_hash.clone())
            }
            Record::Decision {
                decision: DecisionKind::AcceptPolicy,
                choice,
                who: crate::Writer::Human,
                ..
            } => accepted.push(choice.trim().to_string()),
            _ => {}
        }
    }
    let Some(started) = started else {
        return Ok(None);
    };
    if started == policy.hash || accepted.contains(&policy.hash) {
        return Ok(None);
    }
    let short = |h: &str| h.chars().take(12).collect::<String>();
    Ok(Some(format!(
        "the rules changed during this session (verdict.json or the global policy): the session \
         started with policy {} and the policy now is {}. Review the change, then accept it with \
         `vdt accept-policy`, or put the old rules back",
        short(&started),
        short(&policy.hash)
    )))
}

/// Evidence of this session's open findings that no longer matches the tree: the design requires
/// a stale pointer to be shown as stale, not silently displayed as if it still held.
fn stale_pointer_lines(ledger: &Ledger, session: &str, root: &Path) -> Vec<String> {
    let Ok(a) = analyze(ledger, session) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for v in a.findings.iter().filter(|v| v.is_open()) {
        for e in &v.finding.evidence {
            if let crate::pointer::PointerStatus::Stale(why) = e.check(root) {
                let line = format!(
                    "stale evidence: {} (finding {}, record {}) no longer matches the tree: {why}",
                    e.describe(),
                    v.finding.id,
                    v.seq
                );
                if !out.contains(&line) {
                    out.push(line);
                }
            }
        }
    }
    out
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
