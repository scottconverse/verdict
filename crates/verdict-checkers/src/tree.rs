//! A read-only index of the project's files, built by walking only the folder the checker was
//! handed (never above it) and skipping tool and build folders.
//!
//! The index says what it could not see. [`TreeIndex::incomplete`] lists reasons the list of
//! paths may be short (a walk or read error, the file limit); [`TreeIndex::text_gaps`] lists files
//! whose text is missing (over the size limit, unreadable, not valid text). A lookup that finds
//! nothing may only call that "absent" when the part of the index it relies on is complete.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use walkdir::WalkDir;

use crate::paths::SKIP_DIRS;

/// Stop after this many files.
pub const MAX_FILES: usize = 20_000;
/// Do not read a file bigger than this.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Stop reading file text after this many bytes in all.
pub const MAX_TEXT_BYTES: usize = 96 * 1024 * 1024;

/// The files of a project.
#[derive(Debug, Clone, Default)]
pub struct TreeIndex {
    /// Every file path (relative, `/` separators), text or not.
    pub paths: BTreeSet<String>,
    /// Text of every readable text file.
    pub text: BTreeMap<String, String>,
    /// True when a limit stopped the walk early; lookups then cannot prove absence.
    pub truncated: bool,
    /// Why the list of paths may be short: the walk stopped at a limit or hit an error.
    pub incomplete: Vec<String>,
    /// Files that are in `paths` but whose text is not in `text` because it could not be read:
    /// `(path, why)`. Binary files that are not source are not listed.
    pub text_gaps: Vec<(String, String)>,
}

impl TreeIndex {
    /// Walk `root`.
    pub fn build(root: &Path) -> TreeIndex {
        Self::build_with_limits(root, MAX_FILES, MAX_FILE_BYTES, MAX_TEXT_BYTES)
    }

    /// Walk `root` with explicit limits (tests use small ones).
    pub fn build_with_limits(
        root: &Path,
        max_files: usize,
        max_file_bytes: u64,
        max_text_bytes: usize,
    ) -> TreeIndex {
        let mut idx = TreeIndex::default();
        let mut bytes = 0usize;
        let walker = WalkDir::new(root)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                !(e.file_type().is_dir()
                    && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
            });
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    let at = e
                        .path()
                        .map(|p| {
                            p.strip_prefix(root)
                                .unwrap_or(p)
                                .to_string_lossy()
                                .replace('\\', "/")
                        })
                        .unwrap_or_else(|| "?".into());
                    idx.incomplete
                        .push(format!("the folder walk failed at {at}: {e}"));
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            if idx.paths.len() >= max_files {
                idx.truncated = true;
                idx.incomplete.push(format!(
                    "the project has more than {max_files} files; the walk stopped there"
                ));
                break;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            idx.paths.insert(rel.clone());
            let len = match entry.metadata() {
                Ok(m) => m.len(),
                Err(e) => {
                    idx.text_gaps
                        .push((rel, format!("its size could not be read: {e}")));
                    continue;
                }
            };
            if len > max_file_bytes {
                idx.text_gaps.push((
                    rel,
                    format!("it is over the {max_file_bytes}-byte reading limit"),
                ));
                continue;
            }
            if bytes + len as usize > max_text_bytes {
                idx.text_gaps
                    .push((rel, "the total reading limit was reached".into()));
                continue;
            }
            match std::fs::read(entry.path()) {
                Ok(data) => {
                    if data.contains(&0) {
                        idx.text_gaps.push((rel, "it is not plain text".into()));
                    } else {
                        bytes += data.len();
                        idx.text
                            .insert(rel, String::from_utf8_lossy(&data).to_string());
                    }
                }
                Err(e) => idx
                    .text_gaps
                    .push((rel, format!("it could not be read: {e}"))),
            }
        }
        idx
    }

    /// Does a file or folder with this relative path exist?
    pub fn has_path(&self, path: &str) -> bool {
        let p = path.trim_end_matches('/');
        self.paths.contains(p) || self.paths.iter().any(|f| f.starts_with(&format!("{p}/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_walk_reports_nothing_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        let idx = TreeIndex::build(dir.path());
        assert!(idx.incomplete.is_empty() && idx.text_gaps.is_empty() && !idx.truncated);
        assert!(idx.text.contains_key("a.rs"));
    }

    #[test]
    fn the_file_limit_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(dir.path().join(format!("f{i}.rs")), "x\n").unwrap();
        }
        let idx = TreeIndex::build_with_limits(dir.path(), 3, 1000, 1000);
        assert!(idx.truncated);
        assert!(!idx.incomplete.is_empty());
    }

    #[test]
    fn a_file_over_the_size_limit_is_a_text_gap_not_a_silent_skip() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.rs"), "x".repeat(50)).unwrap();
        let idx = TreeIndex::build_with_limits(dir.path(), 100, 10, 1000);
        assert!(idx.paths.contains("big.rs"));
        assert!(!idx.text.contains_key("big.rs"));
        assert_eq!(idx.text_gaps.len(), 1);
        assert!(idx.incomplete.is_empty());
    }
}
