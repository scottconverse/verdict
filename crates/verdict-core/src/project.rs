//! Finding the project a command is running in, from any folder inside it.

use std::path::{Path, PathBuf};

/// Relative location of the ledger inside a project.
pub const LEDGER_REL: &str = ".verdict/ledger.db";

/// Where the ledger of the project at `root` lives.
pub fn ledger_path(root: &Path) -> PathBuf {
    root.join(".verdict").join("ledger.db")
}

/// Find the project root for `start`: the nearest ancestor holding `.verdict/ledger.db`, else the
/// nearest ancestor holding `.git` (a directory, or a file for worktrees), else `start` itself.
///
/// Only walks up through parents; it never scans the disk.
pub fn find_project_root(start: &Path) -> PathBuf {
    let start = std::path::absolute(start).unwrap_or_else(|_| start.to_path_buf());
    if let Some(found) = start.ancestors().find(|d| ledger_path(d).is_file()) {
        return found.to_path_buf();
    }
    if let Some(found) = start.ancestors().find(|d| d.join(".git").exists()) {
        return found.to_path_buf();
    }
    start
}
