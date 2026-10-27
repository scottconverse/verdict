//! Evidence pointers: how a finding proves where it came from.
//!
//! Three shapes, all tied to a hash so they can later be checked against the tree:
//! `file:line` plus the hash of that line, a command plus the hash of its output, or an asset hash.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// A pointer to the evidence behind a finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidencePointer {
    /// A line of a file, with the blake3 of that line (line ending stripped).
    FileLine {
        /// Path relative to the project root, always with `/` separators.
        path: String,
        /// 1-based line number.
        line: u64,
        /// blake3 hex of the line's bytes without its line ending.
        line_hash: String,
    },
    /// A command and the blake3 of its output. Cannot be re-checked against the tree.
    Command {
        /// The command line.
        command: String,
        /// blake3 hex of the output.
        output_hash: String,
    },
    /// A release artifact or screenshot, by blake3 of its bytes.
    Asset {
        /// Path relative to the project root, always with `/` separators.
        path: String,
        /// blake3 hex of the file's bytes.
        asset_hash: String,
    },
}

/// The result of checking a pointer against the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerStatus {
    /// The hash still matches.
    Current,
    /// The hash no longer matches (or the file or line is gone). Shown as stale.
    Stale(String),
    /// The pointer is a command output; there is nothing in the tree to compare.
    NotCheckable,
}

/// blake3 hex of bytes.
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}

fn line_bytes(contents: &[u8], line: u64) -> Option<&[u8]> {
    if line == 0 {
        return None;
    }
    let mut lines: Vec<&[u8]> = contents.split(|b| *b == b'\n').collect();
    // A trailing newline produces one empty final element that is not a real line.
    if lines.len() > 1 && contents.last() == Some(&b'\n') {
        lines.pop();
    }
    let raw = lines.get((line - 1) as usize)?;
    Some(raw.strip_suffix(b"\r").unwrap_or(raw))
}

impl EvidencePointer {
    /// Build a `file:line` pointer by reading the line from the tree now.
    pub fn file_line(root: &Path, path: &str, line: u64) -> std::io::Result<Self> {
        let path = normalize(path);
        let bytes = std::fs::read(root.join(&path))?;
        let text = line_bytes(&bytes, line).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{path} has no line {line}"),
            )
        })?;
        Ok(EvidencePointer::FileLine {
            path,
            line,
            line_hash: hash_bytes(text),
        })
    }

    /// Build a command pointer from the command and its captured output.
    pub fn command(command: &str, output: &[u8]) -> Self {
        EvidencePointer::Command {
            command: command.to_string(),
            output_hash: hash_bytes(output),
        }
    }

    /// Build an asset pointer by hashing the file now.
    pub fn asset(root: &Path, path: &str) -> std::io::Result<Self> {
        let path = normalize(path);
        let bytes = std::fs::read(root.join(&path))?;
        Ok(EvidencePointer::Asset {
            path,
            asset_hash: hash_bytes(&bytes),
        })
    }

    /// Check the pointer against the tree under `root`.
    pub fn check(&self, root: &Path) -> PointerStatus {
        match self {
            EvidencePointer::FileLine {
                path,
                line,
                line_hash,
            } => match std::fs::read(root.join(normalize(path))) {
                Err(e) => PointerStatus::Stale(format!("{path} cannot be read: {e}")),
                Ok(bytes) => match line_bytes(&bytes, *line) {
                    None => PointerStatus::Stale(format!("{path} no longer has line {line}")),
                    Some(text) if hash_bytes(text) == *line_hash => PointerStatus::Current,
                    Some(_) => PointerStatus::Stale(format!("{path}:{line} changed")),
                },
            },
            EvidencePointer::Asset { path, asset_hash } => {
                match std::fs::read(root.join(normalize(path))) {
                    Err(e) => PointerStatus::Stale(format!("{path} cannot be read: {e}")),
                    Ok(bytes) if hash_bytes(&bytes) == *asset_hash => PointerStatus::Current,
                    Ok(_) => PointerStatus::Stale(format!("{path} changed")),
                }
            }
            EvidencePointer::Command { .. } => PointerStatus::NotCheckable,
        }
    }

    /// Short human text, for the report.
    pub fn describe(&self) -> String {
        match self {
            EvidencePointer::FileLine { path, line, .. } => format!("{path}:{line}"),
            EvidencePointer::Command { command, .. } => format!("command `{command}`"),
            EvidencePointer::Asset { path, .. } => format!("asset {path}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_extraction_handles_crlf_and_trailing_newline() {
        assert_eq!(line_bytes(b"a\r\nb\r\n", 1), Some(&b"a"[..]));
        assert_eq!(line_bytes(b"a\r\nb\r\n", 2), Some(&b"b"[..]));
        assert_eq!(line_bytes(b"a\r\nb\r\n", 3), None);
        assert_eq!(line_bytes(b"a\nb", 2), Some(&b"b"[..]));
        assert_eq!(line_bytes(b"a\nb", 0), None);
        assert_eq!(line_bytes(b"", 1), Some(&b""[..]));
    }
}
