#![allow(dead_code)]
//! Helpers shared by the adapter tests: temp folders, fixture playback, ledger reading.

use serde_json::Value;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use verdict_adapters::{Options, run_hook};
use verdict_core::{Ledger, Record, Stored};

static N: AtomicU64 = AtomicU64::new(0);

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> TempDir {
        let p = std::env::temp_dir().join(format!(
            "vdt-adapters-{label}-{}-{}",
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

pub const CLAUDE: &str = include_str!("../fixtures/claude-session.json");
pub const CODEX: &str = include_str!("../fixtures/codex-session.json");

/// A project folder plus an empty "home" (so no real policy file is read).
pub struct Project {
    pub dir: TempDir,
    pub home: TempDir,
}

impl Project {
    pub fn new() -> Project {
        Project {
            dir: TempDir::new("proj"),
            home: TempDir::new("home"),
        }
    }
    pub fn opts(&self) -> Options {
        Options {
            home: Some(self.home.path().to_path_buf()),
        }
    }
    pub fn ledger(&self) -> Ledger {
        Ledger::open_existing(self.dir.path())
            .unwrap()
            .expect("a ledger should exist")
    }
    pub fn records(&self) -> Vec<Stored> {
        self.ledger().records().unwrap()
    }
}

/// The fixture's payloads with `__CWD__` replaced by `cwd` (JSON-escaped).
pub fn playback(fixture: &str, cwd: &Path) -> Vec<(String, String)> {
    let escaped = serde_json::to_string(&cwd.to_string_lossy().to_string()).unwrap();
    let escaped = escaped.trim_matches('"').to_string();
    let v: Value = serde_json::from_str(fixture).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|step| {
            let event = step["event"].as_str().unwrap().to_string();
            let payload = serde_json::to_string(&step["payload"])
                .unwrap()
                .replace("__CWD__", &escaped);
            (event, payload)
        })
        .collect()
}

pub fn run_bytes(p: &Project, host: &str, event: &str, bytes: &[u8]) {
    run_hook(
        host,
        event,
        Box::new(Cursor::new(bytes.to_vec())),
        p.opts(),
        Duration::from_secs(20),
    );
}

pub fn run_fixture(p: &Project, host: &str, fixture: &str) {
    for (event, payload) in playback(fixture, p.dir.path()) {
        run_bytes(p, host, &event, payload.as_bytes());
    }
}

pub fn kinds(records: &[Stored]) -> Vec<&'static str> {
    records.iter().map(|s| s.record.kind().as_str()).collect()
}

pub fn unavailable_reasons(records: &[Stored]) -> Vec<String> {
    records
        .iter()
        .filter_map(|s| match &s.record {
            Record::CheckUnavailable { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub struct CwdGuard {
    prev: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

/// A payload with no `cwd` makes the hook use its own working folder. Point the process at the
/// project for those cases (and back afterwards), one test at a time, so nothing is ever written
/// outside a temp folder. Declare the guard after the project so it drops first.
pub fn enter(p: &Project) -> CwdGuard {
    let lock = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(p.dir.path()).unwrap();
    CwdGuard { prev, _lock: lock }
}
