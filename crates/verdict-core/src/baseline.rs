//! The baseline snapshot of a session: a git tree hash plus a content hash of the uncommitted
//! files. Never a copy of the code. Works on Windows without WSL (it only runs `git`), and in a
//! folder that is not a git repository (the record says so).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The baseline recorded in a session record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    /// blake3 over the tree hash and the uncommitted-files hash. The one value to compare.
    pub hash: String,
    /// False when the folder is not a git repository (or git is not installed).
    pub is_git_repo: bool,
    /// Git details, when it is a repository.
    pub git: Option<GitBaseline>,
    /// Human-readable note, for example why there is no git data.
    pub note: Option<String>,
}

/// Git part of the baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBaseline {
    /// Tree hash of HEAD, or `None` in a repository with no commits yet.
    pub tree_hash: Option<String>,
    /// Commit hash of HEAD, if any.
    pub head_commit: Option<String>,
    /// Content hash of the uncommitted (modified, staged, deleted, untracked) files.
    pub uncommitted_hash: String,
    /// How many uncommitted paths went into that hash.
    pub uncommitted_files: u64,
}

const NO_GIT_MARK: &str = "verdict-baseline-v1:not-a-git-repo";

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Take the baseline snapshot of the project at `root`. Never fails: when git cannot be used the
/// baseline says so.
pub fn snapshot(root: &Path) -> Baseline {
    let not_git = |note: &str| Baseline {
        hash: blake3::hash(NO_GIT_MARK.as_bytes()).to_hex().to_string(),
        is_git_repo: false,
        git: None,
        note: Some(note.to_string()),
    };
    let Some(top) = git(root, &["rev-parse", "--show-toplevel"]).map(text) else {
        return not_git("not a git repository (or git is not installed); no baseline snapshot");
    };
    let top = PathBuf::from(top);
    let head_commit = git(root, &["rev-parse", "--verify", "-q", "HEAD"]).map(text);
    let tree_hash = git(root, &["rev-parse", "--verify", "-q", "HEAD^{tree}"]).map(text);

    let status = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .unwrap_or_default();
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut parts = status.split(|b| *b == 0).filter(|p| !p.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = String::from_utf8_lossy(&entry[..2]).to_string();
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        if code.contains('R') || code.contains('C') {
            parts.next(); // the original name of a rename or copy
        }
        entries.push((path, code));
    }
    entries.sort();

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"verdict-uncommitted-v1\n");
    let mut counted = 0u64;
    for (path, code) in &entries {
        if path.starts_with(".verdict/") {
            continue;
        }
        counted += 1;
        let content = match std::fs::read(top.join(path)) {
            Ok(bytes) => blake3::hash(&bytes).to_hex().to_string(),
            Err(_) if path.ends_with('/') => "directory".to_string(),
            Err(_) => "missing".to_string(),
        };
        hasher.update(format!("{code}\0{path}\0{content}\n").as_bytes());
    }
    let uncommitted_hash = hasher.finalize().to_hex().to_string();

    let mut combined = blake3::Hasher::new();
    combined.update(b"verdict-baseline-v1\n");
    combined.update(tree_hash.as_deref().unwrap_or("no-commits").as_bytes());
    combined.update(b"\n");
    combined.update(uncommitted_hash.as_bytes());
    Baseline {
        hash: combined.finalize().to_hex().to_string(),
        is_git_repo: true,
        git: Some(GitBaseline {
            tree_hash,
            head_commit,
            uncommitted_hash,
            uncommitted_files: counted,
        }),
        note: None,
    }
}
