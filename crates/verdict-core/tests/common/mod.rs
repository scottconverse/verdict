#![allow(dead_code)]
//! Shared helpers for the integration tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::*;

static N: AtomicU64 = AtomicU64::new(0);

/// A scratch folder, removed on drop. Created and owned by the test that asks for it.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!(
            "verdict-test-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A clock that ticks one second per record from a fixed start, so ledgers are reproducible.
pub fn fixed_clock() -> impl Fn() -> String + Send + 'static {
    let n = AtomicU64::new(0);
    move || {
        let s = n.fetch_add(1, Ordering::Relaxed);
        format!("2026-10-26T12:{:02}:{:02}.000Z", s / 60, s % 60)
    }
}

pub fn finding(id: &str, evidence: Vec<EvidencePointer>) -> Finding {
    Finding {
        id: id.into(),
        severity: Severity::Major,
        title: format!("finding {id}"),
        claim_seq: None,
        evidence,
        tier: Tier::Static,
        blast: None,
        judgment: false,
        fix_hint: None,
    }
}

pub fn claim(text: &str) -> Record {
    Record::Claim {
        source: ClaimSource::FinalMessage,
        text: text.into(),
        names: vec!["src/lib.rs".into()],
        prompt_ordinal: Some(1),
    }
}

pub fn prompt(ordinal: u32, text: &str) -> Record {
    Record::Prompt {
        ordinal,
        text: text.into(),
        agent: "claude".into(),
        model: "model-x".into(),
    }
}

pub fn tool_call(tool: &str, target: &str) -> Record {
    Record::ToolCall {
        tool: tool.into(),
        target: target.into(),
        output_hash: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
        duration_ms: 12,
    }
}

pub fn session_record(id: &str) -> Record {
    Record::Session {
        id: id.into(),
        started_at: "2026-10-26T12:00:00.000Z".into(),
        baseline: Baseline {
            hash: "b".repeat(64),
            is_git_repo: false,
            git: None,
            note: Some("not a git repository".into()),
        },
        agent: "claude".into(),
        model: "model-x".into(),
        vendor: "anthropic".into(),
        policy_version: 1,
        policy_hash: "p".repeat(64),
    }
}

/// A small, complete session in a fresh file ledger with a fixed clock.
pub fn seeded(root: &Path) -> Ledger {
    let mut l = Ledger::open(root).unwrap();
    l.set_clock(fixed_clock());
    let sid = "s-test000001";
    l.append(Writer::Adapter, Some(sid), session_record(sid))
        .unwrap();
    l.append(Writer::Adapter, Some(sid), prompt(1, "add a login page"))
        .unwrap();
    l.append(Writer::Agent, Some(sid), claim("login route exists"))
        .unwrap();
    l.append(
        Writer::Adapter,
        Some(sid),
        tool_call("Edit", "src/login.rs"),
    )
    .unwrap();
    l.append(Writer::Adapter, Some(sid), tool_call("Bash", "cargo test"))
        .unwrap();
    l
}
