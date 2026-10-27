//! The small per-host-session state file that maps a host's raw session id to one ledger session.
//!
//! It lives at `<project>/.verdict/hosts/<host>-<hash of the raw id>.json`. It holds nothing the
//! ledger needs for its integrity: the ledger has the session, prompt and tool records. This file
//! only remembers which ledger session a host session became, the prompt ordinal, which prompt
//! each host turn id belongs to, and when each tool call started (Codex sends no duration).
//! If it is lost, the next event of that host session simply opens a new ledger session.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::payload::Host;

/// What is remembered for one host session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// The ledger session id.
    pub ledger_session: String,
    /// The model recorded at session start.
    pub model: String,
    /// Prompts recorded so far.
    pub ordinal: u32,
    /// Host turn id to prompt ordinal.
    #[serde(default)]
    pub turns: BTreeMap<String, u32>,
    /// Tool-use id to start time (milliseconds since the epoch).
    #[serde(default)]
    pub pending: BTreeMap<String, u64>,
}

const MAX_TURNS: usize = 256;
const MAX_PENDING: usize = 128;

impl SessionState {
    /// Remember that `turn_id` belongs to prompt `ordinal`.
    pub fn note_turn(&mut self, turn_id: &str, ordinal: u32) {
        self.turns.insert(turn_id.to_string(), ordinal);
        while self.turns.len() > MAX_TURNS {
            let Some(k) = self
                .turns
                .iter()
                .min_by_key(|(_, v)| **v)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.turns.remove(&k);
        }
    }

    /// Remember that a tool call started at `ms`.
    pub fn note_pending(&mut self, tool_use_id: &str, ms: u64) {
        self.pending.insert(tool_use_id.to_string(), ms);
        while self.pending.len() > MAX_PENDING {
            let Some(k) = self
                .pending
                .iter()
                .min_by_key(|(_, v)| **v)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.pending.remove(&k);
        }
    }
}

/// Where the state file for this host session lives.
pub fn state_path(root: &Path, host: Host, host_session_id: &str) -> PathBuf {
    let h = blake3::hash(host_session_id.as_bytes()).to_hex();
    root.join(".verdict")
        .join("hosts")
        .join(format!("{}-{}.json", host.as_str(), &h[..16]))
}

/// Read the state; `None` when there is none (or it cannot be read, which means the same).
pub fn load(path: &Path) -> Option<SessionState> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write the state atomically (temporary file, then rename).
pub fn save(path: &Path, state: &SessionState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(
        &tmp,
        serde_json::to_vec(state).map_err(std::io::Error::other)?,
    )?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// An exclusive lock next to the state file, so two hooks of one session (parallel tool calls)
/// do not both read the old ordinal. Held for milliseconds. Released on drop.
pub struct Lock(PathBuf);

impl Lock {
    /// Take the lock, waiting up to `wait`. A lock file older than 15 seconds is a dead hook's
    /// and is taken over.
    pub fn acquire(state: &Path, wait: Duration) -> Result<Lock, String> {
        let path = state.with_extension("lock");
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let start = Instant::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Lock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|age| age > Duration::from_secs(15));
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if start.elapsed() >= wait {
                        return Err(format!("session state is locked ({})", path.display()));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(format!("cannot lock {}: {e}", path.display())),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
