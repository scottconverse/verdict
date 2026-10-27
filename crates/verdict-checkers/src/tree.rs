//! A read-only index of the project's files, built by walking only the folder the checker was
//! handed (never above it) and skipping tool and build folders.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use walkdir::WalkDir;

use crate::paths::SKIP_DIRS;

/// Stop after this many files.
const MAX_FILES: usize = 20_000;
/// Do not read a file bigger than this.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Stop reading file text after this many bytes in all.
const MAX_TEXT_BYTES: usize = 96 * 1024 * 1024;

/// The files of a project.
#[derive(Debug, Clone, Default)]
pub struct TreeIndex {
    /// Every file path (relative, `/` separators), text or not.
    pub paths: BTreeSet<String>,
    /// Text of every readable text file.
    pub text: BTreeMap<String, String>,
    /// True when a limit stopped the walk early; lookups then cannot prove absence.
    pub truncated: bool,
}

impl TreeIndex {
    /// Walk `root`.
    pub fn build(root: &Path) -> TreeIndex {
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
        for entry in walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            if idx.paths.len() >= MAX_FILES {
                idx.truncated = true;
                break;
            }
            let Ok(rel) = entry.path().strip_prefix(root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            idx.paths.insert(rel.clone());
            let len = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
            if len > MAX_FILE_BYTES {
                continue;
            }
            if bytes + len as usize > MAX_TEXT_BYTES {
                idx.truncated = true;
                continue;
            }
            if let Ok(data) = std::fs::read(entry.path())
                && !data.contains(&0)
            {
                bytes += data.len();
                idx.text
                    .insert(rel, String::from_utf8_lossy(&data).to_string());
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
