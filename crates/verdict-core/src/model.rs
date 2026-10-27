//! The record types of the ledger and the rules about who may write which.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

use crate::baseline::Baseline;
use crate::pointer::EvidencePointer;

/// Who is writing. Enforced by [`Writer::may_write`] on every append.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Writer {
    /// A per-host hook adapter (prompt, tool call, session, done moment).
    Adapter,
    /// An agent, through the MCP server. Claims only.
    Agent,
    /// The Verdict core itself. The only writer of checks and verdicts.
    Core,
    /// A deterministic checker.
    Checker,
    /// An LLM desk from another vendor.
    Desk,
    /// The human, through the GUI or `vdt`. The only writer of decisions.
    Human,
}

impl Writer {
    /// Every writer, for tests and tables.
    pub const ALL: [Writer; 6] = [
        Writer::Adapter,
        Writer::Agent,
        Writer::Core,
        Writer::Checker,
        Writer::Desk,
        Writer::Human,
    ];

    /// Stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Writer::Adapter => "adapter",
            Writer::Agent => "agent",
            Writer::Core => "core",
            Writer::Checker => "checker",
            Writer::Desk => "desk",
            Writer::Human => "human",
        }
    }

    /// Parse the stable name.
    pub fn parse(s: &str) -> Option<Writer> {
        Writer::ALL.into_iter().find(|w| w.as_str() == s)
    }

    /// May this writer append a record of this kind? (A correction is also checked against the
    /// record it corrects, see [`crate::Ledger::append`].)
    pub fn may_write(self, kind: RecordKind) -> bool {
        use RecordKind::*;
        use Writer::*;
        match kind {
            Session => matches!(self, Adapter | Core),
            Prompt | ToolCall => matches!(self, Adapter),
            Claim => matches!(self, Agent | Adapter),
            Check | Verdict | RefusedWrite => matches!(self, Core),
            Finding | Hypothesis => matches!(self, Checker | Desk),
            Decision => matches!(self, Human),
            CheckUnavailable => matches!(self, Adapter | Core | Checker | Desk),
            // Anyone may correct, but only their own records (checked by the ledger).
            Correction => true,
        }
    }
}

impl fmt::Display for Writer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Proof tier, carried by every check and finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// A file was read.
    Static,
    /// A command the project defines was executed; output hash recorded.
    Ran,
    /// A page on a server the tool started was loaded; screenshot hash recorded.
    Browsed,
    /// A release artifact was listed or hashed.
    Asset,
    /// A desk's reasoning, citing ledger evidence.
    Judgment,
    /// Required, could not run, reason recorded.
    HonestRed,
    /// Not required in this lane, or not possible; nothing checked.
    Unverified,
}

impl Tier {
    /// Stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Static => "static",
            Tier::Ran => "ran",
            Tier::Browsed => "browsed",
            Tier::Asset => "asset",
            Tier::Judgment => "judgment",
            Tier::HonestRed => "honest_red",
            Tier::Unverified => "unverified",
        }
    }
}

/// Finding severity (the stack's five levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Blocks everything.
    Blocker,
    /// Critical.
    Critical,
    /// Major.
    Major,
    /// Minor.
    Minor,
    /// Nit.
    Nit,
}

/// Blast-radius lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// Low.
    Low,
    /// Standard.
    Standard,
    /// High.
    High,
    /// Release.
    Release,
}

/// Where a claim came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimSource {
    /// A line of the plan file.
    PlanLine,
    /// The agent's checkable translation of a prompt.
    PromptTranslation,
    /// How the agent says the human will know it is done.
    DoneWhen,
    /// A sentence of the agent's final message.
    FinalMessage,
    /// A commit message.
    CommitMessage,
    /// A notes file.
    NotesFile,
}

impl ClaimSource {
    /// Stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimSource::PlanLine => "plan_line",
            ClaimSource::PromptTranslation => "prompt_translation",
            ClaimSource::DoneWhen => "done_when",
            ClaimSource::FinalMessage => "final_message",
            ClaimSource::CommitMessage => "commit_message",
            ClaimSource::NotesFile => "notes_file",
        }
    }
}

/// Outcome of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    /// It ran.
    Ran,
    /// It was required but could not run.
    FailedToRun,
    /// Policy said not to run it.
    SkippedByPolicy,
}

/// The decisions only the human may make (the design's decisions table, plus pause).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Which file is the plan.
    PlanFile,
    /// Run the project's commands.
    RunProjectCommands,
    /// Install hooks in this project.
    InstallHooks,
    /// Shadow or enforce (a mode switch).
    ModeSwitch,
    /// Accept a prompt translation about something the human did not type.
    AcceptPromptTranslation,
    /// Withdraw a prompt the human cancelled.
    WithdrawPrompt,
    /// Waive words no check can cover.
    WaiveWords,
    /// Accept a finding as not real.
    AcceptFindingNotReal,
    /// Commit anyway past a FAILED pre-commit gate (needs a typed reason).
    CommitAnyway,
    /// Raise the lane.
    RaiseLane,
    /// Tag a release.
    TagRelease,
    /// Pause the gate for a stated duration.
    Pause,
}

impl DecisionKind {
    /// Stable lower-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::PlanFile => "plan_file",
            DecisionKind::RunProjectCommands => "run_project_commands",
            DecisionKind::InstallHooks => "install_hooks",
            DecisionKind::ModeSwitch => "mode_switch",
            DecisionKind::AcceptPromptTranslation => "accept_prompt_translation",
            DecisionKind::WithdrawPrompt => "withdraw_prompt",
            DecisionKind::WaiveWords => "waive_words",
            DecisionKind::AcceptFindingNotReal => "accept_finding_not_real",
            DecisionKind::CommitAnyway => "commit_anyway",
            DecisionKind::RaiseLane => "raise_lane",
            DecisionKind::TagRelease => "tag_release",
            DecisionKind::Pause => "pause",
        }
    }
}

/// The word a verdict record holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictWord {
    /// A blocking finding exists.
    Failed,
    /// Something is the human's to settle.
    NeedsYou,
    /// Verified, always with the highest tier reached.
    Verified,
    /// Release lane only.
    ClearToShip,
    /// A required checker did not run.
    Partial,
    /// Nothing was checked.
    Unverified,
}

/// One ledger record. The `type` field in JSON is the [`RecordKind`] name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Record {
    /// A session opened.
    Session {
        /// Session id (see [`crate::session::new_session_id`]).
        id: String,
        /// Start time, RFC 3339 UTC.
        started_at: String,
        /// The baseline snapshot (never a copy of the code).
        baseline: Baseline,
        /// Agent name, for example `claude`.
        agent: String,
        /// Model name.
        model: String,
        /// Vendor of the model.
        vendor: String,
        /// Policy version in force.
        policy_version: u32,
        /// blake3 of the effective (merged) policy.
        policy_hash: String,
    },
    /// A prompt, verbatim.
    Prompt {
        /// 1-based ordinal within the session.
        ordinal: u32,
        /// The prompt word for word.
        text: String,
        /// Agent name.
        agent: String,
        /// Model name.
        model: String,
    },
    /// Something the agent says is true.
    Claim {
        /// Where it came from.
        source: ClaimSource,
        /// The claim text.
        text: String,
        /// The URL, file or symbol it names.
        names: Vec<String>,
        /// The prompt ordinal it translates, if any.
        prompt_ordinal: Option<u32>,
    },
    /// A tool call an adapter saw.
    ToolCall {
        /// Tool name.
        tool: String,
        /// Target file or command.
        target: String,
        /// blake3 of the output (never the output itself).
        output_hash: String,
        /// Duration in milliseconds.
        duration_ms: u64,
    },
    /// A check the core ran, skipped or could not run.
    Check {
        /// Checker or desk name.
        name: String,
        /// Proof tier.
        tier: Tier,
        /// Hash of the inputs.
        inputs_hash: String,
        /// Start time.
        started_at: String,
        /// Finish time.
        finished_at: String,
        /// Outcome.
        outcome: CheckOutcome,
        /// Reason or detail.
        detail: Option<String>,
    },
    /// A finding with at least one evidence pointer. Without one it becomes a hypothesis.
    Finding(Finding),
    /// A finding that has no evidence pointer. Never counts toward or against a verdict.
    Hypothesis(Finding),
    /// A decision. Human only.
    Decision {
        /// Which decision.
        decision: DecisionKind,
        /// The choice made, for example `enforce`, or the finding id accepted.
        choice: String,
        /// The reason the human typed (required for commit anyway).
        reason: Option<String>,
        /// Always `human`.
        who: Writer,
    },
    /// A verdict, computed by the core from the ledger.
    Verdict {
        /// The word.
        word: VerdictWord,
        /// Highest tier reached (the mandatory suffix of `verified`).
        highest_tier: Option<Tier>,
        /// Counts by tier.
        counts_by_tier: BTreeMap<Tier, u64>,
        /// What was not established.
        not_established: Vec<String>,
        /// Policy version.
        policy_version: u32,
        /// The lane the verdict was computed for (absent in verdicts written before Part 2b).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lane: Option<Lane>,
    },
    /// An attempted write that was refused.
    RefusedWrite {
        /// Who tried.
        attempted_by: Writer,
        /// The kind they tried to write.
        attempted_kind: RecordKind,
        /// For a decision: which one (so commit anyway and mode switch are visible).
        attempted_decision: Option<DecisionKind>,
        /// Why it was refused.
        reason: String,
        /// What they tried to write.
        attempted: serde_json::Value,
    },
    /// A component could not run. The result reads unverified, never clean.
    CheckUnavailable {
        /// Which component (adapter, core, a checker, policy).
        component: String,
        /// Why.
        reason: String,
    },
    /// A correction of an earlier record. The earlier record stays; this one points at it.
    Correction {
        /// Sequence number of the record corrected.
        target_seq: u64,
        /// Why.
        reason: String,
        /// The corrected content, if any. It must itself be writable by the writer.
        replacement: Option<Box<Record>>,
    },
}

/// The payload of a finding or hypothesis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// Finding id, unique within the session.
    pub id: String,
    /// Severity.
    pub severity: Severity,
    /// Title.
    pub title: String,
    /// Sequence number of the claim record it bears on.
    pub claim_seq: Option<u64>,
    /// Evidence pointers. Empty means hypothesis.
    pub evidence: Vec<EvidencePointer>,
    /// Proof tier.
    pub tier: Tier,
    /// Blast lane, if classified.
    pub blast: Option<Lane>,
    /// True if it comes from a desk's judgment.
    pub judgment: bool,
    /// Fix hint.
    pub fix_hint: Option<String>,
}

/// The kind of a record: the `kind` column in SQLite and in the JSONL export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// session
    Session,
    /// prompt
    Prompt,
    /// claim
    Claim,
    /// tool_call
    ToolCall,
    /// check
    Check,
    /// finding
    Finding,
    /// hypothesis
    Hypothesis,
    /// decision
    Decision,
    /// verdict
    Verdict,
    /// refused_write
    RefusedWrite,
    /// check_unavailable
    CheckUnavailable,
    /// correction
    Correction,
}

impl RecordKind {
    /// Every kind.
    pub const ALL: [RecordKind; 12] = [
        RecordKind::Session,
        RecordKind::Prompt,
        RecordKind::Claim,
        RecordKind::ToolCall,
        RecordKind::Check,
        RecordKind::Finding,
        RecordKind::Hypothesis,
        RecordKind::Decision,
        RecordKind::Verdict,
        RecordKind::RefusedWrite,
        RecordKind::CheckUnavailable,
        RecordKind::Correction,
    ];

    /// Stable snake_case name (also the `type` tag in JSON).
    pub fn as_str(self) -> &'static str {
        match self {
            RecordKind::Session => "session",
            RecordKind::Prompt => "prompt",
            RecordKind::Claim => "claim",
            RecordKind::ToolCall => "tool_call",
            RecordKind::Check => "check",
            RecordKind::Finding => "finding",
            RecordKind::Hypothesis => "hypothesis",
            RecordKind::Decision => "decision",
            RecordKind::Verdict => "verdict",
            RecordKind::RefusedWrite => "refused_write",
            RecordKind::CheckUnavailable => "check_unavailable",
            RecordKind::Correction => "correction",
        }
    }
}

impl fmt::Display for RecordKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Record {
    /// The kind of this record.
    pub fn kind(&self) -> RecordKind {
        match self {
            Record::Session { .. } => RecordKind::Session,
            Record::Prompt { .. } => RecordKind::Prompt,
            Record::Claim { .. } => RecordKind::Claim,
            Record::ToolCall { .. } => RecordKind::ToolCall,
            Record::Check { .. } => RecordKind::Check,
            Record::Finding(_) => RecordKind::Finding,
            Record::Hypothesis(_) => RecordKind::Hypothesis,
            Record::Decision { .. } => RecordKind::Decision,
            Record::Verdict { .. } => RecordKind::Verdict,
            Record::RefusedWrite { .. } => RecordKind::RefusedWrite,
            Record::CheckUnavailable { .. } => RecordKind::CheckUnavailable,
            Record::Correction { .. } => RecordKind::Correction,
        }
    }
}
