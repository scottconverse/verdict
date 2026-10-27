//! Reads a source tree with tree-sitter and lists what it finds: routes, calls, data writes,
//! pages, buttons and forms, migrations, definitions. The wiring checker and `verdict_scan` read
//! these facts; nothing here decides a verdict.
//!
//! Layout: [`model`] is the fact model, [`http`] normalizes URL paths, [`syntax`] parses and
//! holds shared tree helpers, and [`py`], [`ts`] and [`rs`] hold the per-language extractors.
//! [`extract_tree`] lists a project's files, parses each once and runs every extractor that reads
//! its language.

pub mod http;
pub mod model;
pub mod py;
pub mod rs;
pub mod syntax;
pub mod ts;

pub use model::*;

use std::path::Path;
use std::process::Command;
use tree_sitter::Tree;

/// One parsed file, shared by every extractor that reads its language.
pub struct Source<'a> {
    /// Repo-relative path, `/` separators.
    pub path: &'a str,
    /// The file text.
    pub text: &'a str,
    /// Its language.
    pub lang: Lang,
    /// The parse tree.
    pub tree: &'a Tree,
}

/// Reads one framework (or one language's definitions) out of a parsed file.
pub trait Extractor: Send + Sync {
    /// A short stable name (`fastapi`, `react`, `python_symbols`).
    fn name(&self) -> &'static str;
    /// The languages it reads.
    fn langs(&self) -> &'static [Lang];
    /// Add what it finds to `out`. Must not panic on odd input; a file it cannot read adds
    /// nothing. When it finds anything framework-specific it adds its framework to
    /// `out.frameworks`.
    fn extract(&self, src: &Source, out: &mut FileFacts);
    /// Runs once after every file is extracted, in registry order, to join facts across files
    /// (router prefixes from mounts, a generated client's base path). Default: nothing.
    fn link(&self, _facts: &mut Facts) {}
}

/// Every extractor in this build.
pub fn registry() -> Vec<Box<dyn Extractor>> {
    let mut v = py::extractors();
    v.extend(ts::extractors());
    v.extend(rs::extractors());
    v
}

/// Limits for one scan.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Stop listing after this many files and mark the result truncated.
    pub max_files: usize,
    /// Skip (and list as incomplete) files larger than this.
    pub max_file_bytes: u64,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            max_files: 20_000,
            max_file_bytes: 1024 * 1024,
        }
    }
}

/// Directories never read when the project is not a git repository.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    ".next",
    ".output",
    ".turbo",
    ".mypy_cache",
    ".pytest_cache",
];

/// The files in the project: `git ls-files` (tracked and untracked, ignoring what git ignores)
/// when `root` is in a git work tree, else a walk that skips [`SKIP_DIRS`]. Paths are
/// repo-relative with `/` separators, sorted. The bool is true when the list stopped at
/// `max_files`.
pub fn list_files(root: &Path, opts: &ScanOptions) -> (Vec<String>, bool, Vec<Incomplete>) {
    let git = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "-c", "-o", "--exclude-standard"])
        .output();
    if let Ok(out) = git
        && out.status.success()
    {
        let mut files: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).replace('\\', "/"))
            .filter(|p| root.join(p).is_file())
            .collect();
        files.sort();
        files.dedup();
        let truncated = files.len() > opts.max_files;
        files.truncate(opts.max_files);
        return (files, truncated, Vec::new());
    }
    walk(root, opts)
}

fn walk(root: &Path, opts: &ScanOptions) -> (Vec<String>, bool, Vec<Incomplete>) {
    let mut files = Vec::new();
    let mut incomplete = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut truncated = false;
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                incomplete.push(Incomplete {
                    path: rel(root, &dir),
                    reason: format!("could not list the folder: {e}"),
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let p = entry.path();
            if ft.is_dir() {
                let name = entry.file_name();
                if !SKIP_DIRS.contains(&name.to_string_lossy().as_ref()) {
                    stack.push(p);
                }
            } else if ft.is_file() {
                if files.len() >= opts.max_files {
                    truncated = true;
                    break;
                }
                files.push(rel(root, &p));
            }
        }
        if truncated {
            break;
        }
    }
    files.sort();
    (files, truncated, incomplete)
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Parse one file's text and run every extractor that reads its language.
pub fn extract_source(
    path: &str,
    text: &str,
    lang: Lang,
    extractors: &[Box<dyn Extractor>],
) -> FileFacts {
    let mut out = FileFacts::new(path, lang);
    let Some(tree) = syntax::parse(lang, text) else {
        out.parse_errors = true;
        return out;
    };
    out.parse_errors = tree.root_node().has_error();
    let src = Source {
        path,
        text,
        lang,
        tree: &tree,
    };
    for x in extractors.iter().filter(|x| x.langs().contains(&lang)) {
        x.extract(&src, &mut out);
    }
    out
}

/// What reading one listed path came to.
enum Read {
    /// Not a source file any extractor reads, and not one to report.
    Skip,
    /// A source file in a language no extractor parses.
    Unsupported(String),
    /// A file that could not be read completely.
    Incomplete(Incomplete),
    /// A parsed file.
    File(Box<FileFacts>),
}

fn read_one(
    root: &Path,
    path: &str,
    opts: &ScanOptions,
    extractors: &[Box<dyn Extractor>],
) -> Read {
    let Some(lang) = syntax::lang_of(path) else {
        return if syntax::is_unsupported_source(path) {
            Read::Unsupported(path.to_string())
        } else {
            Read::Skip
        };
    };
    let incomplete = |reason: String| {
        Read::Incomplete(Incomplete {
            path: path.to_string(),
            reason,
        })
    };
    let full = root.join(path);
    match std::fs::metadata(&full) {
        Ok(m) if m.len() > opts.max_file_bytes => {
            return incomplete(format!(
                "skipped: {} bytes is over the {} byte limit",
                m.len(),
                opts.max_file_bytes
            ));
        }
        Err(e) => return incomplete(format!("could not read: {e}")),
        _ => {}
    }
    let bytes = match std::fs::read(&full) {
        Ok(b) => b,
        Err(e) => return incomplete(format!("could not read: {e}")),
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return incomplete("not UTF-8 text".into());
    };
    Read::File(Box::new(extract_source(path, &text, lang, extractors)))
}

/// Read and extract the given repo-relative files.
///
/// Files are read on several threads, each taking the next unread path; the results are put back
/// in the order of `paths` and then sorted, so the facts are the same as reading the files one at
/// a time.
pub fn extract_files(
    root: &Path,
    paths: &[String],
    opts: &ScanOptions,
    extractors: &[Box<dyn Extractor>],
) -> Facts {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8)
        .min(paths.len().max(1));
    let next = AtomicUsize::new(0);
    let work = || {
        let mut done: Vec<(usize, Read)> = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(path) = paths.get(i) else { break };
            done.push((i, read_one(root, path, opts, extractors)));
        }
        done
    };
    let mut results: Vec<(usize, Read)> = if threads <= 1 {
        work()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads).map(|_| s.spawn(work)).collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                .collect()
        })
    };
    results.sort_by_key(|(i, _)| *i);

    let mut facts = Facts::default();
    for (_, r) in results {
        match r {
            Read::Skip => {}
            Read::Unsupported(p) => facts.unsupported.push(p),
            Read::Incomplete(i) => facts.incomplete.push(i),
            Read::File(f) => facts.files.push(*f),
        }
    }
    facts.files.sort_by(|a, b| a.path.cmp(&b.path));
    facts.unsupported.sort();
    facts.incomplete.sort();
    for x in extractors {
        x.link(&mut facts);
    }
    facts
}

/// List, read and extract a whole project.
pub fn extract_tree(root: &Path, opts: &ScanOptions) -> Facts {
    let (paths, truncated, mut walk_incomplete) = list_files(root, opts);
    let mut facts = extract_files(root, &paths, opts, &registry());
    facts.truncated = truncated;
    facts.incomplete.append(&mut walk_incomplete);
    facts.incomplete.sort();
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_lists_and_marks_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/x")).unwrap();
        std::fs::write(dir.path().join("app/main.py"), "def f():\n    pass\n").unwrap();
        std::fs::write(
            dir.path().join("app/ui.tsx"),
            "export const A = () => <b/>;\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("svc.go"), "package main\n").unwrap();
        std::fs::write(dir.path().join("node_modules/x/i.js"), "x()\n").unwrap();
        std::fs::write(dir.path().join("big.py"), "x = 1\n".repeat(100)).unwrap();

        let opts = ScanOptions {
            max_files: 100,
            max_file_bytes: 200,
        };
        let facts = extract_tree(dir.path(), &opts);
        let paths: Vec<&str> = facts.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["app/main.py", "app/ui.tsx"]);
        assert_eq!(facts.unsupported, ["svc.go"]);
        assert_eq!(facts.incomplete.len(), 1);
        assert_eq!(facts.incomplete[0].path, "big.py");
        assert!(!facts.truncated);
        assert!(!facts.is_complete());
        assert_eq!(facts.file("app/ui.tsx").unwrap().lang, Lang::Tsx);

        let tight = ScanOptions {
            max_files: 1,
            max_file_bytes: 1 << 20,
        };
        assert!(extract_tree(dir.path(), &tight).truncated);
    }
}
