//! Sessions: opening one (baseline snapshot plus policy version) and listing them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::baseline::{Baseline, snapshot};
use crate::ledger::{Ledger, LedgerError};
use crate::model::{Record, Writer};
use crate::policy::Policy;
use crate::timefmt::now_rfc3339;

/// What an adapter knows when a session starts.
#[derive(Debug, Clone, Default)]
pub struct SessionParams {
    /// Agent name, for example `claude`.
    pub agent: String,
    /// Model name.
    pub model: String,
    /// Model vendor.
    pub vendor: String,
    /// Folder holding `.verdict/policy.json`; `None` means the user's home folder.
    pub home: Option<PathBuf>,
}

/// The result of [`start_session`].
#[derive(Debug, Clone)]
pub struct SessionStarted {
    /// The new session id.
    pub id: String,
    /// Sequence number of the session record.
    pub seq: u64,
    /// The baseline that was recorded.
    pub baseline: Baseline,
    /// Policy version recorded.
    pub policy_version: u32,
    /// Set when the policy files could not be used and the built-in default was used instead
    /// (a `check_unavailable` record was written; the session reads unverified).
    pub policy_warning: Option<String>,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh session id such as `s-3fa9c01b7d`.
pub fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut h = blake3::Hasher::new();
    h.update(&nanos.to_le_bytes());
    h.update(&u64::from(std::process::id()).to_le_bytes());
    h.update(&n.to_le_bytes());
    format!("s-{}", &h.finalize().to_hex()[..10])
}

/// Open a session for the project at `root`: take the baseline snapshot, load the policy, write
/// the `session` record (as the adapter).
///
/// Fails open: if the policy files are unusable the built-in default is recorded and a
/// `check_unavailable` record explains why. If git is missing the baseline says it is not a repo.
pub fn start_session(
    ledger: &mut Ledger,
    root: &Path,
    params: &SessionParams,
) -> Result<SessionStarted, LedgerError> {
    let baseline = snapshot(root);
    let (policy, policy_warning) = match Policy::load(root, params.home.as_deref()) {
        Ok(p) => (p, None),
        Err(e) => match Policy::builtin() {
            Ok(p) => (p, Some(e.to_string())),
            Err(e2) => {
                return Err(LedgerError::Invalid(format!("no usable policy: {e}; {e2}")));
            }
        },
    };
    let id = new_session_id();
    let appended = ledger.append(
        Writer::Adapter,
        Some(&id),
        Record::Session {
            id: id.clone(),
            started_at: now_rfc3339(),
            baseline: baseline.clone(),
            agent: params.agent.clone(),
            model: params.model.clone(),
            vendor: params.vendor.clone(),
            policy_version: policy.version,
            policy_hash: policy.hash.clone(),
        },
    )?;
    if let Some(reason) = &policy_warning {
        ledger.append(
            Writer::Core,
            Some(&id),
            Record::CheckUnavailable {
                component: "policy".into(),
                reason: format!("{reason}; the built-in default policy was used"),
            },
        )?;
    }
    Ok(SessionStarted {
        id,
        seq: appended.seq,
        baseline,
        policy_version: policy.version,
        policy_warning,
    })
}

/// A session as listed by `vdt sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// Session id.
    pub id: String,
    /// Sequence number of its session record.
    pub seq: u64,
    /// Start time.
    pub started_at: String,
    /// Agent.
    pub agent: String,
    /// Model.
    pub model: String,
    /// Vendor.
    pub vendor: String,
    /// Policy version.
    pub policy_version: u32,
    /// Baseline hash.
    pub baseline_hash: String,
    /// Records carrying this session id.
    pub record_count: u64,
}

/// Every session in the ledger, oldest first.
pub fn list_sessions(ledger: &Ledger) -> Result<Vec<SessionInfo>, LedgerError> {
    let records = ledger.records()?;
    let mut out: Vec<SessionInfo> = Vec::new();
    for s in &records {
        if let Record::Session {
            id,
            started_at,
            baseline,
            agent,
            model,
            vendor,
            policy_version,
            ..
        } = &s.record
        {
            out.push(SessionInfo {
                id: id.clone(),
                seq: s.seq,
                started_at: started_at.clone(),
                agent: agent.clone(),
                model: model.clone(),
                vendor: vendor.clone(),
                policy_version: *policy_version,
                baseline_hash: baseline.hash.clone(),
                record_count: 0,
            });
        }
    }
    for s in &records {
        if let Some(sid) = &s.session_id
            && let Some(info) = out.iter_mut().find(|i| &i.id == sid)
        {
            info.record_count += 1;
        }
    }
    Ok(out)
}
