//! regressions: the named bug-pattern catalog in lines added this session, and relative imports
//! that resolved at the baseline and do not now. Always blocks (severity `Blocker`).
//!
//! The catalog (each is a rule name in the finding id):
//!
//! * `empty-catch`: a `catch` block with nothing in it, or a Python `except` whose body is `pass`
//! * `bare-except`: Python `except:` with no exception type
//! * `debugger`: a leftover `debugger;`, `pdb.set_trace()`, `breakpoint()`
//! * `focused-test`: `.only(`, `fit(`, `fdescribe(` and friends
//! * `hard-coded-secret`: a known key shape, a private-key header, or a secret-looking name
//!   assigned a string literal
//! * `broken-import`: a relative import (JS/TS, Python, Rust `mod`) that resolved at the baseline
//!   and does not now
//! * `route-call`: a call (fetch, generated client, server-function or Tauri call, ...) whose
//!   target was served at the baseline and is served by nothing now
//!
//! `unwrap` on user input is not in the catalog: the design does not list it.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::changes::Status;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::paths;

/// The regressions checker.
pub struct Regressions;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(
    EMPTY_CATCH_MULTI,
    r"(?s)\bcatch\b\s*(?:\([^)]*\))?\s*\{(?:\s|//[^\n]*)*\}"
);
re!(PY_EXCEPT_PASS_SAME_LINE, r"^\s*except\b[^:]*:\s*pass\b");
re!(PY_EXCEPT_HEAD, r"^\s*except\b[^:]*:\s*$");
re!(PY_PASS, r"^\s*pass\b\s*(#.*)?$");
re!(BARE_EXCEPT, r"^\s*except\s*:");
re!(JS_DEBUGGER, r"(?:^|[;{}])\s*debugger\s*;?\s*$");
re!(
    PY_DEBUGGER,
    r"(?:^|[^\w.])(?:i?pdb\.set_trace\(\)|breakpoint\(\))"
);
re!(
    FOCUSED,
    r"\b(?:describe|it|test|context)\.only\s*\(|(?:^|[^\w.$])(?:fit|fdescribe|fcontext)\s*\("
);
re!(
    SECRET_SHAPE,
    r"AKIA[0-9A-Z]{16}|-----BEGIN (?:RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9]{36,}|xox[baprs]-[A-Za-z0-9-]{10,}|sk-[A-Za-z0-9_-]{24,}"
);
re!(
    SECRET_ASSIGN,
    r#"(?i)\b[\w-]*(?:password|passwd|secret|api[_-]?key|auth[_-]?token|access[_-]?token|private[_-]?key)[\w-]*["']?\s*[:=]\s*["']([^"'\s]{8,})["']"#
);
re!(
    PLACEHOLDER,
    r"(?i)example|dummy|fake|placeholder|changeme|change_me|your[_-]|xxxx|<[^>]+>|\$\{|process\.env|os\.environ|getenv|env\(|\*{4,}|redacted|todo"
);
re!(
    JS_IMPORT,
    r#"(?:\bfrom\s*|\bimport\s*\(?\s*|\brequire\s*\(\s*)["'](\.{1,2}/[^"']*|\.{1,2})["']"#
);
re!(PY_REL_IMPORT, r"^\s*from\s+(\.+)([\w.]*)\s+import\s+(.+)$");
re!(RS_MOD, r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;");

impl Checker for Regressions {
    fn name(&self) -> &'static str {
        "regressions"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Static]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let changes = ctx.changes()?;
        for c in &changes.caveats {
            ctx.note(c.clone());
        }
        let exclude = ctx.exclude();
        let suffix = changes.title_suffix();
        let mut drafts: Vec<Draft> = Vec::new();

        for f in &changes.files {
            if matches!(f.status, Status::Deleted)
                || f.binary
                || paths::is_noise_path(&f.path)
                || paths::is_excluded(&f.path, &exclude)
                || !paths::is_code(&f.path)
            {
                continue;
            }
            let added: BTreeSet<u64> = f.added.iter().map(|l| l.no).collect();
            let full = ctx.read_text(&f.path);
            let ext = paths::ext(&f.path);
            let is_py = ext == "py";
            let js = paths::is_js_like(&f.path);
            let testy = paths::is_test_path(&f.path);

            let mut push = |rule: &'static str, line: u64, title: &str, hint: &str| {
                drafts.push(Draft {
                    rule,
                    severity: Severity::Blocker,
                    title: format!("{title} in {}:{line}{suffix}", f.path),
                    claim_seq: None,
                    evidence: vec![ctx.line_pointer(&f.path, line)],
                    tier: Tier::Static,
                    fix_hint: Some(hint.to_string()),
                    path: f.path.clone(),
                    line,
                });
            };

            // Multi-line: empty catch blocks (start line must be an added line).
            if !is_py && let Some(text) = &full {
                for m in EMPTY_CATCH_MULTI.find_iter(text) {
                    let line = text[..m.start()].matches('\n').count() as u64 + 1;
                    if added.contains(&line) && !is_comment(line_text(text, line)) {
                        push(
                            "empty-catch",
                            line,
                            "empty catch block swallows the error",
                            "handle or rethrow the error, or log it with the reason it is safe to ignore",
                        );
                    }
                }
            }
            if is_py && let Some(text) = &full {
                let lines: Vec<&str> = text.lines().collect();
                for l in &f.added {
                    let idx = l.no as usize - 1;
                    let Some(&cur) = lines.get(idx) else { continue };
                    let swallow = PY_EXCEPT_PASS_SAME_LINE.is_match(cur)
                        || (PY_EXCEPT_HEAD.is_match(cur)
                            && lines.get(idx + 1).is_some_and(|n| PY_PASS.is_match(n)));
                    if swallow {
                        push(
                            "empty-catch",
                            l.no,
                            "except block that only passes swallows the error",
                            "handle or re-raise the error, or log it",
                        );
                    }
                }
            }

            for l in &f.added {
                let t = l.text.as_str();
                if is_py && BARE_EXCEPT.is_match(t) {
                    push(
                        "bare-except",
                        l.no,
                        "bare `except:` catches everything, including exit signals",
                        "name the exception type",
                    );
                }
                if (js && JS_DEBUGGER.is_match(t)) || (is_py && PY_DEBUGGER.is_match(t)) {
                    push(
                        "debugger",
                        l.no,
                        "leftover debugger statement",
                        "remove the debugger statement",
                    );
                }
                if (js || is_py || testy) && FOCUSED.is_match(t) && !is_comment(t) {
                    // `fit(` is a normal method name in non-test code; only count it in tests.
                    let only = t.contains(".only");
                    if only || testy {
                        push(
                            "focused-test",
                            l.no,
                            "focused test (.only / fit / fdescribe) makes the rest of the suite silently not run",
                            "remove the focus marker",
                        );
                    }
                }
                if SECRET_SHAPE.is_match(t) {
                    push(
                        "hard-coded-secret",
                        l.no,
                        "hard-coded secret (a key or private-key shape)",
                        "remove it, rotate it, and read it from the environment",
                    );
                } else if let Some(c) = SECRET_ASSIGN.captures(t)
                    && !PLACEHOLDER.is_match(t)
                    && !is_comment(t)
                    && c.get(1).is_some_and(|v| looks_secret(v.as_str()))
                {
                    push(
                        "hard-coded-secret",
                        l.no,
                        "hard-coded secret (a secret-named value set to a string literal)",
                        "read it from the environment or a secret store",
                    );
                }
            }
        }

        drafts.extend(self.broken_imports(ctx)?);
        drafts.extend(self.broken_route_calls(ctx)?);
        Ok(finish(self.name(), drafts))
    }
}

fn line_text(text: &str, line: u64) -> &str {
    text.lines().nth(line as usize - 1).unwrap_or("")
}

fn is_comment(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("//") || t.starts_with('#') || t.starts_with('*') || t.starts_with("/*")
}

/// A value that has the look of a real secret: not all one repeated character, not a plain word.
fn looks_secret(v: &str) -> bool {
    let distinct: BTreeSet<char> = v.chars().collect();
    let mixed = v
        .chars()
        .any(|c| c.is_ascii_digit() || !c.is_alphanumeric());
    (distinct.len() >= 4 && mixed) || (distinct.len() >= 6 && v.len() >= 12)
}

impl Regressions {
    /// Relative imports that resolved at the baseline and do not now. Needs a git baseline;
    /// without one the part that needs the baseline's file list is noted as not checked.
    fn broken_imports(&self, ctx: &CheckContext) -> Result<Vec<Draft>, Unavailable> {
        let changes = ctx.changes()?;
        let base = match ctx.baseline_paths() {
            Ok(b) => b,
            Err(u) => {
                ctx.note(format!(
                    "unverified: imports that resolved at the baseline were not checked ({})",
                    u.reason
                ));
                return Ok(Vec::new());
            }
        };
        ctx.note(
            "unverified: only relative imports (JS/TS `./x`, Python `from .x`, Rust `mod x;`) \
             are resolved; package and alias imports are not read until the tree-sitter extractor exists",
        );
        let tree = ctx.tree();
        let exclude = ctx.exclude();
        let mut out = Vec::new();
        // Files this session may have broken: any file that existed at the baseline, changed or
        // not, whose import target is gone. Only files that exist at both ends can regress.
        for path in tree.text.keys() {
            if !base.contains(path)
                || paths::is_noise_path(path)
                || paths::is_excluded(path, &exclude)
            {
                continue;
            }
            // An unchanged file can still lose its target; a changed one can too.
            let text = &tree.text[path];
            let dir = paths::parent(path);
            let ext = paths::ext(path);
            for (i, line) in text.lines().enumerate() {
                let line_no = i as u64 + 1;
                let candidates = import_candidates(path, dir, &ext, line);
                if candidates.is_empty() {
                    continue;
                }
                let resolved_then = candidates.iter().any(|c| base.contains(c));
                let resolved_now = candidates.iter().any(|c| tree.paths.contains(c));
                if resolved_then && !resolved_now {
                    let gone = candidates
                        .iter()
                        .find(|c| base.contains(*c))
                        .cloned()
                        .unwrap_or_default();
                    let moved = changes
                        .files
                        .iter()
                        .any(|f| matches!(&f.status, Status::Renamed(from) if *from == gone));
                    out.push(Draft {
                        rule: "broken-import",
                        severity: Severity::Blocker,
                        title: format!(
                            "import in {path}:{line_no} resolved to {gone} at the baseline and resolves to nothing now{}",
                            if moved { " (the target was renamed)" } else { "" }
                        ),
                        claim_seq: None,
                        evidence: vec![ctx.line_pointer(path, line_no)],
                        tier: Tier::Static,
                        fix_hint: Some("restore the target or update the import".into()),
                        path: path.clone(),
                        line: line_no,
                    });
                }
            }
        }
        Ok(out)
    }
}

impl Regressions {
    /// Calls whose target was served at the baseline and is served by nothing now. Needs a git
    /// baseline (the baseline's own endpoint facts come from the changed files' old text plus the
    /// current tree) and a complete read of the tree.
    fn broken_route_calls(&self, ctx: &CheckContext) -> Result<Vec<Draft>, Unavailable> {
        let base = match ctx.baseline_paths() {
            Ok(b) => b.clone(),
            Err(u) => {
                ctx.note(format!(
                    "unverified: calls whose route disappeared were not checked ({})",
                    u.reason
                ));
                return Ok(Vec::new());
            }
        };
        let now =
            verdict_extract::extract_tree(&ctx.root, &verdict_extract::ScanOptions::default());
        if !now.is_complete() {
            ctx.note(
                "unverified: calls whose route disappeared were not checked, because the tree \
                 could not be read completely"
                    .to_string(),
            );
            return Ok(Vec::new());
        }
        let now_keys: BTreeSet<verdict_extract::EndpointKey> =
            now.endpoints().map(|e| e.key.clone()).collect();
        // What the baseline served: everything served now, plus what the changed (or deleted)
        // files used to serve. A target that existed at the baseline and exists nowhere now is
        // the regression this looks for.
        let changes = ctx.changes()?;
        let extractors = verdict_extract::registry();
        let mut base_keys = now_keys.clone();
        for f in &changes.files {
            if matches!(f.status, Status::Added) || f.binary || !base.contains(&f.path) {
                continue;
            }
            let Some(old) = ctx.baseline_text(&f.path) else {
                continue;
            };
            let Some(lang) = verdict_extract::syntax::lang_of(&f.path) else {
                continue;
            };
            let facts = verdict_extract::extract_source(&f.path, &old, lang, &extractors);
            base_keys.extend(facts.endpoints.into_iter().map(|e| e.key));
        }
        let mut out = Vec::new();
        for file in &now.files {
            let in_baseline = base.contains(&file.path);
            for call in &file.calls {
                if call.dynamic || !in_baseline {
                    continue;
                }
                let served_now = now_keys.iter().any(|k| endpoint_fits(&call.target, k));
                if served_now {
                    continue;
                }
                let served_then = base_keys.iter().any(|k| endpoint_fits(&call.target, k));
                if !served_then {
                    continue;
                }
                out.push(Draft {
                    rule: "route-call",
                    severity: Severity::Blocker,
                    title: format!(
                        "call {} {} ({}:{}) reached a route at the baseline and reaches nothing now",
                        call.target.method.as_deref().unwrap_or("*"),
                        call.target.name,
                        file.path,
                        call.loc.line
                    ),
                    claim_seq: None,
                    evidence: vec![ctx.line_pointer(&file.path, call.loc.line as u64)],
                    tier: Tier::Static,
                    fix_hint: Some(
                        "restore the route or point the call at the one that serves it now".into(),
                    ),
                    path: file.path.clone(),
                    line: call.loc.line as u64,
                });
            }
        }
        Ok(out)
    }
}

/// Does a call's target reach this endpoint? A call with no method (a server function, a command)
/// matches on kind and name; an HTTP call also has to agree on the method.
fn endpoint_fits(call: &verdict_extract::EndpointKey, ep: &verdict_extract::EndpointKey) -> bool {
    call.kind == ep.kind
        && call.name == ep.name
        && match (&call.method, &ep.method) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        }
}

/// Paths a relative import on this line could mean, relative to the project root. Empty when the
/// line is not a relative import.
fn import_candidates(file: &str, dir: &str, ext: &str, line: &str) -> Vec<String> {
    let join = |base: &str, rel: &str| -> String { normalize_join(base, rel) };
    match ext {
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "vue" | "svelte" => {
            let mut out = Vec::new();
            for c in JS_IMPORT.captures_iter(line) {
                let spec = &c[1];
                let target = join(dir, spec);
                let spec_has_ext = matches!(
                    paths::ext(&target).as_str(),
                    "js" | "jsx"
                        | "ts"
                        | "tsx"
                        | "mjs"
                        | "cjs"
                        | "json"
                        | "css"
                        | "vue"
                        | "svelte"
                        | "scss"
                );
                out.push(target.clone());
                if !spec_has_ext {
                    for e in [
                        "ts", "tsx", "js", "jsx", "mjs", "cjs", "json", "vue", "svelte",
                    ] {
                        out.push(format!("{target}.{e}"));
                        out.push(format!("{target}/index.{e}"));
                    }
                }
            }
            out
        }
        "py" => {
            let Some(c) = PY_REL_IMPORT.captures(line) else {
                return Vec::new();
            };
            let dots = c[1].len();
            let mut base = dir.to_string();
            for _ in 1..dots {
                base = paths::parent(&base).to_string();
            }
            let module = c[2].replace('.', "/");
            if module.is_empty() {
                // `from . import a, b`: each name is a module or a name in __init__.
                return Vec::new();
            }
            let target = join(&base, &module);
            vec![format!("{target}.py"), format!("{target}/__init__.py")]
        }
        "rs" => {
            let Some(c) = RS_MOD.captures(line) else {
                return Vec::new();
            };
            let name = &c[1];
            let fname = paths::file_name(file);
            let owner_dir = if matches!(fname, "mod.rs" | "lib.rs" | "main.rs") {
                dir.to_string()
            } else {
                join(dir, &paths::stem(file))
            };
            vec![
                join(&owner_dir, &format!("{name}.rs")),
                join(&owner_dir, &format!("{name}/mod.rs")),
            ]
        }
        _ => Vec::new(),
    }
}

/// `base/rel` with `.` and `..` resolved; stays inside the project (an escape yields a path that
/// matches nothing).
fn normalize_join(base: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_resolves_dots() {
        assert_eq!(normalize_join("src/a", "../b/c"), "src/b/c");
        assert_eq!(normalize_join("", "./x"), "x");
    }

    #[test]
    fn js_import_candidates() {
        let c = import_candidates("src/a.ts", "src", "ts", "import { x } from './util';");
        assert!(c.contains(&"src/util.ts".to_string()));
        assert!(c.contains(&"src/util/index.ts".to_string()));
        assert!(import_candidates("src/a.ts", "src", "ts", "import x from 'react';").is_empty());
    }

    #[test]
    fn rust_mod_candidates() {
        let c = import_candidates("src/lib.rs", "src", "rs", "pub mod gone;");
        assert_eq!(c, vec!["src/gone.rs", "src/gone/mod.rs"]);
        let c = import_candidates("src/a.rs", "src", "rs", "mod b;");
        assert_eq!(c, vec!["src/a/b.rs", "src/a/b/mod.rs"]);
    }

    #[test]
    fn secret_values_that_are_words_do_not_count() {
        assert!(!looks_secret("aaaaaaaaaa"));
        assert!(!looks_secret("password"));
        assert!(looks_secret("s3cr3t-Value-9f2"));
    }
}
