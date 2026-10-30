//! Verdict core: the append-only evidence ledger and everything that reads it.
//!
//! The one rule this crate enforces structurally: **the agent writes claims, never verdicts.**
//!
//! # Map
//!
//! * [`Ledger`] is the store: SQLite at `<project>/.verdict/ledger.db` (WAL), one row per record,
//!   a monotonic sequence number and a blake3 hash chain. Rows cannot be updated or deleted
//!   (database triggers), and [`Ledger::verify`] finds any tampering, reordering or truncation.
//! * [`Writer`] and [`Record`] say who may write what. [`Ledger::append`] refuses a forbidden write
//!   and records the refusal as a `refused_write`.
//! * [`EvidencePointer`] ties a finding to a line, a command output or an asset by hash.
//!   A finding with no pointer is stored as a [`Record::Hypothesis`].
//! * [`Tier`] is the proof tier carried by every check and finding.
//! * [`start_session`] writes the session record (baseline snapshot plus policy version).
//! * [`Policy`] loads and validates the layered policy files.
//! * [`render_report`] renders the plain-text report for one session.
//!
//! Fail open on the tool, never on the verdict: nothing here ever reports "clean". When something
//! cannot run, record a [`Record::CheckUnavailable`] and the result reads unverified.

pub mod baseline;
pub mod exec_approval;
pub mod export;
pub mod gate;
pub mod githook;
pub mod human;
pub mod lane;
pub mod ledger;
pub mod model;
pub mod part1c;
pub mod pointer;
pub mod policy;
pub mod project;
pub mod report;
pub mod report_head;
pub mod seal;
pub mod session;
pub mod sha256;
pub mod timefmt;
pub mod verdict;
pub mod verify;

pub use baseline::{
    Baseline, FolderBaseline, GitBaseline, snapshot as baseline_snapshot, tree_state,
    tree_state_of_ledger,
};
pub use ledger::{Appended, Ledger, LedgerError, Stored};
pub use model::{
    CheckOutcome, ClaimSource, DecisionKind, Finding, Lane, Record, RecordKind, Severity, Tier,
    VerdictWord, Writer,
};
pub use pointer::{EvidencePointer, PointerStatus};
pub use policy::{Policy, PolicyError, PolicySource};
pub use project::{find_project_root, ledger_path};
pub use report::{render_report, render_report_with};
pub use session::{SessionInfo, SessionParams, SessionStarted, list_sessions, start_session};
pub use verify::{Problem, VerifyReport};
