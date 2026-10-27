//! "Lines added this session": the working tree compared with the session's baseline.
//!
//! * Baseline is a git commit: `git diff -U0` against it, plus every untracked file (all its lines
//!   count as added). Line numbers are exact.
//! * Baseline is a folder hash: the baseline holds no per-file data, so a line-level diff is not
//!   possible. If the folder still hashes the same, nothing changed. Otherwise every readable file
//!   is treated as wholly new and [`Changes::line_level`] is false, so a checker can say so.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// How a file differs from the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// New since the baseline.
    Added,
    /// Changed.
    Modified,
    /// Gone.
    Deleted,
    /// Moved here from the named path.
    Renamed(String),
    /// Folder baseline: nothing says whether it changed.
    Unknown,
}

/// One line with its 1-based number (new file for added lines, old file for removed lines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// 1-based line number.
    pub no: u64,
    /// The text without its line ending.
    pub text: String,
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path relative to the project root, `/` separators. For a deleted file, the old path.
    pub path: String,
    /// How it changed.
    pub status: Status,
    /// Lines added (new numbering).
    pub added: Vec<Line>,
    /// Lines removed (old numbering).
    pub removed: Vec<Line>,
    /// True when git or the reader says the file is not text.
    pub binary: bool,
}

/// Everything that changed since the baseline.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes {
    /// Changed files, sorted by path.
    pub files: Vec<FileChange>,
    /// False when the baseline could not give a line-level diff.
    pub line_level: bool,
    /// False when the baseline cannot say which files are new or gone.
    pub baseline_known: bool,
    /// Plain-words caveats to put in the check's detail.
    pub caveats: Vec<String>,
}

impl Changes {
    /// The change for one path.
    pub fn file(&self, path: &str) -> Option<&FileChange> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Text to append to a finding title when the diff was not line-level.
    pub fn title_suffix(&self) -> &'static str {
        if self.line_level {
            ""
        } else {
            " (whole-file scan: the baseline is a folder hash, so a line-level diff was not possible)"
        }
    }

    /// A stable fingerprint of what changed, for the check's inputs hash.
    pub fn fingerprint(&self) -> String {
        let mut h = blake3::Hasher::new();
        for f in &self.files {
            h.update(f.path.as_bytes());
            h.update(format!("{:?}", f.status).as_bytes());
            for l in f.added.iter().chain(f.removed.iter()) {
                h.update(&l.no.to_le_bytes());
                h.update(l.text.as_bytes());
                h.update(b"\n");
            }
            h.update(b"\0");
        }
        h.finalize().to_hex().to_string()
    }
}

/// Run git in `root`. Output as bytes, or the reason it could not be had.
pub fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| format!("git could not be run: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// The empty tree, for a repository with no commits yet.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn strip_cr(s: &str) -> &str {
    s.strip_suffix('\r').unwrap_or(s)
}

fn unquote(p: &str) -> String {
    let p = p.trim_end_matches('\t');
    if p.len() >= 2 && p.starts_with('"') && p.ends_with('"') {
        let inner = &p[1..p.len() - 1];
        let mut out = String::new();
        let mut it = inner.chars();
        while let Some(c) = it.next() {
            if c == '\\' {
                match it.next() {
                    Some('t') => out.push('\t'),
                    Some('n') => out.push('\n'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        p.to_string()
    }
}

fn diff_path(raw: &str) -> Option<String> {
    let p = unquote(raw);
    if p == "/dev/null" {
        return None;
    }
    Some(
        p.strip_prefix("a/")
            .or_else(|| p.strip_prefix("b/"))
            .unwrap_or(&p)
            .to_string(),
    )
}

fn hunk_starts(header: &str) -> Option<(u64, u64)> {
    // @@ -a[,b] +c[,d] @@
    let rest = header.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let first = |s: &str| s.split(',').next().and_then(|n| n.parse::<u64>().ok());
    Some((first(old)?, first(new)?))
}

type Hunks = (Vec<Line>, Vec<Line>);

/// Parse `git diff -U0` output into added and removed lines per path (the new path, or the old
/// path for a deletion).
fn parse_unified(text: &str) -> BTreeMap<String, Hunks> {
    let mut out: BTreeMap<String, Hunks> = BTreeMap::new();
    let mut old: Option<String> = None;
    let mut new: Option<String> = None;
    let mut in_hunk = false;
    let mut next_old = 0u64;
    let mut next_new = 0u64;
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            old = None;
            new = None;
            in_hunk = false;
            continue;
        }
        if !in_hunk {
            if let Some(p) = line.strip_prefix("--- ") {
                old = diff_path(p);
                continue;
            }
            if let Some(p) = line.strip_prefix("+++ ") {
                new = diff_path(p);
                continue;
            }
        }
        if line.starts_with("@@ ") {
            if let Some((o, n)) = hunk_starts(line) {
                next_old = o;
                next_new = n;
                in_hunk = true;
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        let key = match new.clone().or_else(|| old.clone()) {
            Some(k) => k,
            None => continue,
        };
        if let Some(t) = line.strip_prefix('+') {
            out.entry(key).or_default().0.push(Line {
                no: next_new,
                text: strip_cr(t).to_string(),
            });
            next_new += 1;
        } else if let Some(t) = line.strip_prefix('-') {
            out.entry(key).or_default().1.push(Line {
                no: next_old,
                text: strip_cr(t).to_string(),
            });
            next_old += 1;
        }
    }
    out
}

/// Split text into numbered lines.
pub fn numbered_lines(text: &str) -> Vec<Line> {
    text.lines()
        .enumerate()
        .map(|(i, l)| Line {
            no: i as u64 + 1,
            text: strip_cr(l).to_string(),
        })
        .collect()
}

/// Changes since a git baseline. `base` is a commit (or [`EMPTY_TREE`]).
pub fn git_changes(root: &Path, base: &str) -> Result<Changes, String> {
    let raw = git(
        root,
        &[
            "diff",
            "--raw",
            "-z",
            "-M",
            "--no-abbrev",
            "--relative",
            "--no-ext-diff",
            base,
            "--",
        ],
    )?;
    let patch = git(
        root,
        &[
            "diff",
            "-U0",
            "-M",
            "--relative",
            "--no-color",
            "--no-ext-diff",
            base,
            "--",
        ],
    )?;
    let patch_text = String::from_utf8_lossy(&patch).to_string();
    let mut hunks = parse_unified(&patch_text);

    let mut files: Vec<FileChange> = Vec::new();
    let fields: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    let mut i = 0;
    while i < fields.len() {
        let meta = String::from_utf8_lossy(fields[i]).to_string();
        i += 1;
        if !meta.starts_with(':') {
            continue;
        }
        let code = meta.split_whitespace().last().unwrap_or("");
        let letter = code.chars().next().unwrap_or('M');
        let take = |i: &mut usize| -> String {
            let v = fields
                .get(*i)
                .map(|b| String::from_utf8_lossy(b).to_string())
                .unwrap_or_default();
            *i += 1;
            v
        };
        let (path, status) = match letter {
            'R' | 'C' => {
                let from = take(&mut i);
                let to = take(&mut i);
                (to, Status::Renamed(from))
            }
            'A' => (take(&mut i), Status::Added),
            'D' => (take(&mut i), Status::Deleted),
            'U' => {
                take(&mut i);
                continue;
            }
            _ => (take(&mut i), Status::Modified),
        };
        let path = path.replace('\\', "/");
        let (added, removed) = hunks.remove(&path).unwrap_or_default();
        files.push(FileChange {
            path,
            status,
            added,
            removed,
            binary: false,
        });
    }
    // Hunks left over belong to paths the raw list did not name (should not happen); keep them
    // rather than lose lines.
    for (path, (added, removed)) in hunks {
        files.push(FileChange {
            path,
            status: Status::Modified,
            added,
            removed,
            binary: false,
        });
    }
    // Files git sees as changed but with no hunk and not deleted: binary (or mode-only).
    let binary_paths: BTreeSet<String> = patch_text
        .lines()
        .filter_map(|l| {
            let l = l.strip_prefix("Binary files ")?.strip_suffix(" differ")?;
            let (_, new) = l.rsplit_once(" and ")?;
            diff_path(new)
        })
        .collect();
    for f in &mut files {
        if binary_paths.contains(&f.path) {
            f.binary = true;
        }
    }

    // Untracked files: every line is new.
    let untracked = git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
    )?;
    let seen: BTreeSet<String> = files.iter().map(|f| f.path.clone()).collect();
    for p in untracked.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(p).replace('\\', "/");
        if seen.contains(&path) {
            continue;
        }
        let (added, binary) = match std::fs::read(root.join(&path)) {
            Ok(bytes) if bytes.len() > 2 * 1024 * 1024 || bytes.contains(&0) => (Vec::new(), true),
            Ok(bytes) => (numbered_lines(&String::from_utf8_lossy(&bytes)), false),
            Err(_) => (Vec::new(), false),
        };
        files.push(FileChange {
            path,
            status: Status::Added,
            added,
            removed: Vec::new(),
            binary,
        });
    }
    files.retain(|f| !f.path.starts_with(".verdict/"));
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Changes {
        files,
        line_level: true,
        baseline_known: true,
        caveats: Vec::new(),
    })
}

/// `git show <base>:./<path>` as text; `None` when the file did not exist or is not text.
pub fn baseline_text(root: &Path, base: &str, path: &str) -> Option<String> {
    let bytes = git(root, &["show", &format!("{base}:./{path}")]).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).to_string())
}

/// Every path in the baseline commit, relative to `root`.
pub fn baseline_paths(root: &Path, base: &str) -> Result<BTreeSet<String>, String> {
    let out = git(root, &["ls-tree", "-r", "--name-only", "-z", base])?;
    Ok(out
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).replace('\\', "/"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_numbers_lines() {
        let d = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -3,1 +3,2 @@\n-old\n+new1\n+new2\n@@ -10,0 +12,1 @@\n+tail\n";
        let m = parse_unified(d);
        let (added, removed) = &m["x.rs"];
        assert_eq!(
            added.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![3, 4, 12]
        );
        assert_eq!(removed[0].no, 3);
        assert_eq!(removed[0].text, "old");
    }

    #[test]
    fn deleted_file_hunks_key_on_old_path() {
        let d = "diff --git a/g.rs b/g.rs\ndeleted file mode 100644\n--- a/g.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        let m = parse_unified(d);
        assert_eq!(m["g.rs"].1.len(), 2);
    }

    #[test]
    fn a_removed_line_that_looks_like_a_header_is_content() {
        let d = "diff --git a/s.sql b/s.sql\n--- a/s.sql\n+++ b/s.sql\n@@ -1,1 +1,1 @@\n--- comment\n+++ comment2\n";
        let m = parse_unified(d);
        assert_eq!(m["s.sql"].1[0].text, "-- comment");
        assert_eq!(m["s.sql"].0[0].text, "++ comment2");
    }
}
