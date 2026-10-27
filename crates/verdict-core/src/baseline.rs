//! The baseline snapshot of a session: a git tree hash plus a content hash of the uncommitted
//! files. Never a copy of the code. Works on Windows without WSL (it only runs `git`), and in a
//! folder that is not a git repository: the record says so and holds a content hash of the
//! folder's files instead (bounded, skipping build and tool folders).

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
    /// Content hash of the folder's files, when it is not a git repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Box<FolderBaseline>>,
}

/// Baseline of a folder that is not a git repository: a content hash of its files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderBaseline {
    /// blake3 over every file's relative path and content hash, in sorted order.
    pub content_hash: String,
    /// How many files went into that hash.
    pub files: u64,
    /// False when the walk stopped at the file or byte limit; the hash then covers only part of
    /// the folder.
    pub complete: bool,
    /// Folder names that were skipped wherever they appear.
    pub skipped: Vec<String>,
}

/// Folders never hashed: tool state, version control and build output.
const SKIP_DIRS: &[&str] = &[
    ".verdict",
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
];
/// Stop after this many files.
const MAX_FILES: u64 = 10_000;
/// Stop after reading this many bytes of file content.
const MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Hash the files under `root` (not following links), skipping [`SKIP_DIRS`], within the limits.
fn folder_snapshot(root: &Path) -> FolderBaseline {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut bytes = 0u64;
    let mut complete = true;
    let mut pending: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    'walk: while let Some((dir, rel)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let rel_path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    pending.push((path, rel_path));
                }
                continue;
            }
            if files.len() as u64 >= MAX_FILES || bytes.saturating_add(meta.len()) > MAX_BYTES {
                complete = false;
                break 'walk;
            }
            let content = if meta.file_type().is_symlink() {
                match std::fs::read_link(&path) {
                    Ok(target) => format!("link:{}", target.to_string_lossy()),
                    Err(_) => "link".to_string(),
                }
            } else {
                match std::fs::read(&path) {
                    Ok(data) => {
                        bytes += data.len() as u64;
                        blake3::hash(&data).to_hex().to_string()
                    }
                    Err(_) => "unreadable".to_string(),
                }
            };
            files.push((rel_path, content));
        }
    }
    files.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"verdict-folder-v1\n");
    for (path, content) in &files {
        hasher.update(format!("{path}\0{content}\n").as_bytes());
    }
    FolderBaseline {
        content_hash: hasher.finalize().to_hex().to_string(),
        files: files.len() as u64,
        complete,
        skipped: SKIP_DIRS.iter().map(|s| s.to_string()).collect(),
    }
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

/// The state of the code at `root` right now, as one comparable string. A check records it when it
/// runs; the verdict compares it with the state now, so a check counts only for the code it saw.
///
/// * In a git repository: `git:<blake3>` over every path git tracks or would track (index entries,
///   with modified and untracked files hashed the way `git add` would hash them, deleted files
///   dropped, ignored files and `.verdict/` left out). It does not depend on HEAD, so committing
///   the checked code does not make a check stale; staging it does not either.
/// * Elsewhere: `folder:<blake3>`, the folder content hash the baseline uses.
///
/// `None` when the state cannot be read; a check with no state never counts as current.
pub fn tree_state(root: &Path) -> Option<String> {
    match git(root, &["rev-parse", "--show-toplevel"]).map(text) {
        Some(top) if !top.is_empty() => git_tree_state(Path::new(&top)),
        _ => Some(format!("folder:{}", folder_snapshot(root).content_hash)),
    }
}

/// [`tree_state`] of the project whose ledger this is (`<root>/.verdict/ledger.db`). `None` for an
/// in-memory ledger or one stored anywhere else.
pub fn tree_state_of_ledger(ledger: &crate::ledger::Ledger) -> Option<String> {
    let dir = ledger.path()?.parent()?;
    if dir.file_name()? != ".verdict" {
        return None;
    }
    tree_state(dir.parent()?)
}

fn is_tool_state(path: &str) -> bool {
    path == ".verdict" || path.starts_with(".verdict/") || path.contains("/.verdict/")
}

fn nul_split(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string())
}

fn git_tree_state(top: &Path) -> Option<String> {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::process::Stdio;

    let mut entries: BTreeMap<String, String> = BTreeMap::new();
    for rec in nul_split(&git(top, &["ls-files", "-s", "-z"])?) {
        let Some((meta, path)) = rec.split_once('\t') else {
            continue;
        };
        if let Some(sha) = meta.split_whitespace().nth(1) {
            entries.insert(path.to_string(), sha.to_string());
        }
    }
    let mut changed: Vec<String> = nul_split(&git(
        top,
        &["diff", "--name-only", "--no-renames", "--no-relative", "-z"],
    )?)
    .collect();
    changed.extend(nul_split(&git(
        top,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?));
    changed.sort();
    changed.dedup();

    let mut to_hash: Vec<String> = Vec::new();
    for path in changed {
        match std::fs::symlink_metadata(top.join(&path)) {
            Err(_) => {
                entries.remove(&path);
            }
            Ok(m) if m.is_dir() => {
                // A submodule whose checkout moved: name it changed, nothing more to read here.
                entries.insert(path, "changed-directory".to_string());
            }
            Ok(_) if path.contains('\n') => {
                let content = std::fs::read(top.join(&path)).ok()?;
                entries.insert(path, format!("b3-{}", blake3::hash(&content).to_hex()));
            }
            Ok(_) => to_hash.push(path),
        }
    }
    if !to_hash.is_empty() {
        // `hash-object` without `-w` only computes; nothing is written to the repository.
        let mut child = Command::new("git")
            .arg("-C")
            .arg(top)
            .args(["hash-object", "--stdin-paths"])
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let input = to_hash.join("\n") + "\n";
        let mut stdin = child.stdin.take()?;
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let out = child.wait_with_output().ok()?;
        writer.join().ok()?.ok()?;
        if !out.status.success() {
            return None;
        }
        let shas: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        if shas.len() != to_hash.len() {
            return None;
        }
        for (path, sha) in to_hash.into_iter().zip(shas) {
            entries.insert(path, sha);
        }
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"verdict-tree-v1\n");
    for (path, sha) in entries.iter().filter(|(p, _)| !is_tool_state(p)) {
        hasher.update(format!("{path}\0{sha}\n").as_bytes());
    }
    Some(format!("git:{}", hasher.finalize().to_hex()))
}

/// Take the baseline snapshot of the project at `root`. Never fails: when git cannot be used the
/// baseline says so.
pub fn snapshot(root: &Path) -> Baseline {
    let Some(top) = git(root, &["rev-parse", "--show-toplevel"]).map(text) else {
        let folder = folder_snapshot(root);
        let mut combined = blake3::Hasher::new();
        combined.update(b"verdict-baseline-v1:folder\n");
        combined.update(folder.content_hash.as_bytes());
        let note = if folder.complete {
            format!(
                "not a git repository (or git is not installed); content hash of {} files",
                folder.files
            )
        } else {
            format!(
                "not a git repository (or git is not installed); PARTIAL content hash: stopped at {} files",
                folder.files
            )
        };
        return Baseline {
            hash: combined.finalize().to_hex().to_string(),
            is_git_repo: false,
            git: None,
            note: Some(note),
            folder: Some(Box::new(folder)),
        };
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
        folder: None,
    }
}
