//! baseline_diff: things that appeared this session that nothing the human wrote asked for.
//!
//! "Asked for" means the name shows up in a prompt or a claim of this session (the file, its
//! folder, the thing a test file tests, the dependency, the table, the route path). Never blocks:
//! every finding is severity `Major`, which forces NEEDS YOU.
//!
//! Without the tree-sitter extractor (Part 3) the rules are file-level and line-pattern rules:
//!
//! * `unplanned-file` / `orphan-file`: a new file nothing asked for (the second also has nothing
//!   in the tree that mentions it)
//! * `unplanned-migration`: a new migration or `.sql` file nothing asked for
//! * `unplanned-page`: a new file under `pages/`, `routes/`, `views/`, `templates/`, `screens/`
//! * `new-dependency`: a package added to `package.json`, `Cargo.toml`, `requirements*.txt` or `go.mod`
//! * `new-table`: `CREATE TABLE` / `create_table(` in an added line
//! * `new-route`: a route decorator or registration in an added line
//!
//! What it could not read is listed in the check's detail as `unverified`.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::changes::{FileChange, Status};
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::paths;

/// The baseline_diff checker.
pub struct BaselineDiff;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(
    CREATE_TABLE,
    r#"(?i)\bcreate\s+table\s+(?:if\s+not\s+exists\s+)?["`\[]?([\w.]+)|\bcreate_table\s*\(\s*["']([\w.]+)|\bcreateTable\s*\(\s*["'`]([\w.]+)"#
);
re!(
    ROUTE,
    r#"@\w+\.(?:get|post|put|patch|delete|route|websocket)\s*\(\s*["']([^"']+)|\b(?:app|router|server|api)\.(?:get|post|put|patch|delete)\s*\(\s*["'`]([^"'`]+)|#\[(?:get|post|put|patch|delete)\s*\(\s*"([^"]+)"#
);
re!(REQ_NAME, r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)");
re!(
    GO_REQ,
    r"^\s*(?:require\s+)?([\w.\-]+\.[\w.\-]+/[\w.\-/]+)\s+v[\w.\-+]+"
);
re!(CARGO_DEP, r"^\s*([A-Za-z0-9_-]+)\s*=");
re!(CARGO_SECTION, r"^\s*\[([^\]]+)\]");

const GENERIC_DIRS: &[&str] = &[
    "src",
    "lib",
    "app",
    "test",
    "tests",
    "docs",
    "packages",
    "crates",
    "include",
    "pkg",
    "scripts",
    "public",
    "static",
    "assets",
    "components",
];

impl Checker for BaselineDiff {
    fn name(&self) -> &'static str {
        "baseline_diff"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Static]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let changes = ctx.changes()?;
        if !changes.baseline_known {
            return Err(Unavailable::new(
                "the baseline is a folder hash: it cannot say which files, dependencies, tables \
                 or routes are new",
            ));
        }
        let mut corpus = String::new();
        for p in &ctx.prompts {
            corpus.push_str(p);
            corpus.push('\n');
        }
        for c in &ctx.claims {
            corpus.push_str(&c.text);
            corpus.push('\n');
            for n in &c.names {
                corpus.push_str(n);
                corpus.push('\n');
            }
        }
        if corpus.trim().is_empty() {
            return Err(Unavailable::new(
                "this session has no prompt or claim, so nothing says what was asked for",
            ));
        }
        let asked = corpus.to_ascii_lowercase().replace('\\', "/");
        for c in &changes.caveats {
            ctx.note(c.clone());
        }
        ctx.note(
            "unverified: routes, pages and tables are recognized by file location and line \
             patterns only (no tree-sitter extractor yet); data-changing buttons are not read",
        );
        let exclude = ctx.exclude();
        let tree = ctx.tree();
        let mut drafts: Vec<Draft> = Vec::new();
        let mut odd_manifests: BTreeSet<String> = BTreeSet::new();

        for f in &changes.files {
            if paths::is_noise_path(&f.path) || paths::is_excluded(&f.path, &exclude) {
                continue;
            }
            let name = paths::file_name(&f.path);
            if matches!(
                name,
                "pyproject.toml"
                    | "Gemfile"
                    | "pom.xml"
                    | "build.gradle"
                    | "composer.json"
                    | "Pipfile"
            ) {
                odd_manifests.insert(f.path.clone());
            }
            if !matches!(f.status, Status::Deleted) && !f.binary {
                self.line_rules(ctx, f, &asked, &mut drafts);
                self.dependency_rules(ctx, f, &asked, &mut drafts);
            }
            if matches!(f.status, Status::Added) && !is_exempt(&f.path) {
                if mentioned(&asked, &f.path) {
                    continue;
                }
                let referenced = {
                    let stem = paths::stem(&f.path);
                    stem.len() >= 3
                        && tree.text.iter().any(|(p, t)| {
                            *p != f.path && !paths::is_noise_path(p) && t.contains(&stem)
                        })
                };
                let (rule, what) = classify_new_file(&f.path, referenced);
                let extra = if rule == "orphan-file" {
                    ", and nothing in the project points at it"
                } else {
                    ""
                };
                drafts.push(Draft {
                    rule,
                    severity: Severity::Major,
                    title: format!(
                        "{what} {} that nothing in this session's prompts or claims asked for{extra}",
                        f.path
                    ),
                    claim_seq: None,
                    evidence: vec![if f.added.is_empty() {
                        ctx.diff_pointer(&format!("A\t{}", f.path))
                    } else {
                        ctx.line_pointer(&f.path, 1)
                    }],
                    tier: Tier::Static,
                    fix_hint: Some("keep it and say so, or remove it".into()),
                    path: f.path.clone(),
                    line: 0,
                });
            }
        }
        if !odd_manifests.is_empty() {
            ctx.note(format!(
                "unverified: new dependencies in {} were not read (only package.json, Cargo.toml, requirements*.txt and go.mod are)",
                odd_manifests.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        Ok(finish(self.name(), drafts))
    }
}

fn is_exempt(path: &str) -> bool {
    let name = paths::file_name(path);
    matches!(
        name,
        ".gitignore" | ".gitattributes" | "verdict.json" | ".editorconfig"
    )
}

fn classify_new_file(path: &str, referenced: bool) -> (&'static str, &'static str) {
    let lower = path.to_ascii_lowercase();
    let comps: Vec<&str> = lower.split('/').collect();
    let dirs = &comps[..comps.len().saturating_sub(1)];
    if paths::ext(&lower) == "sql"
        || dirs
            .iter()
            .any(|c| matches!(*c, "migrations" | "migrate" | "alembic" | "versions"))
    {
        return ("unplanned-migration", "new migration");
    }
    if dirs
        .iter()
        .any(|c| matches!(*c, "pages" | "routes" | "views" | "templates" | "screens"))
        && (paths::is_code(&lower) || matches!(paths::ext(&lower).as_str(), "html" | "htm"))
    {
        return ("unplanned-page", "new page or route file");
    }
    if referenced {
        ("unplanned-file", "new file")
    } else {
        ("orphan-file", "new file")
    }
}

/// Is the file, its folder, or the thing it tests named in what the human wrote?
fn mentioned(asked: &str, path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if asked.contains(&lower) || asked.contains(&paths::file_name(&lower).to_string()) {
        return true;
    }
    let stem = paths::stem(&lower);
    if stem.len() >= 4 && asked.contains(&stem) {
        return true;
    }
    if paths::is_test_path(path) {
        let subject = crate::tests_weakened::subject_stem(path);
        if subject.len() >= 3 && asked.contains(&subject) {
            return true;
        }
    }
    // A folder the human named: `src/auth` covers `src/auth/login.ts`.
    let comps: Vec<&str> = lower.split('/').collect();
    for end in 1..comps.len() {
        let dir = comps[..end].join("/");
        let generic = end == 1 && GENERIC_DIRS.contains(&comps[0]);
        if !generic && dir.len() >= 3 && asked.contains(&format!("{dir}/")) {
            return true;
        }
        if !generic && dir.len() >= 3 && end == comps.len() - 1 && asked.contains(&dir) {
            return true;
        }
    }
    false
}

impl BaselineDiff {
    fn line_rules(&self, ctx: &CheckContext, f: &FileChange, asked: &str, drafts: &mut Vec<Draft>) {
        if !paths::is_code(&f.path) {
            return;
        }
        // A route or table already in the file at the baseline is not new, even if its line
        // changed.
        let before = match &f.status {
            Status::Added => String::new(),
            Status::Renamed(from) => ctx.baseline_text(from).unwrap_or_default(),
            _ => ctx.baseline_text(&f.path).unwrap_or_default(),
        }
        .to_ascii_lowercase();
        for l in &f.added {
            if let Some(c) = CREATE_TABLE.captures(&l.text) {
                let table = c
                    .get(1)
                    .or_else(|| c.get(2))
                    .or_else(|| c.get(3))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                if !table.is_empty()
                    && !asked.contains(&table.to_ascii_lowercase())
                    && !before.contains(&table.to_ascii_lowercase())
                {
                    drafts.push(Draft {
                        rule: "new-table",
                        severity: Severity::Major,
                        title: format!(
                            "new table {table} at {}:{} that nothing in this session's prompts or claims asked for",
                            f.path, l.no
                        ),
                        claim_seq: None,
                        evidence: vec![ctx.line_pointer(&f.path, l.no)],
                        tier: Tier::Static,
                        fix_hint: Some("keep it and say so, or remove it".into()),
                        path: f.path.clone(),
                        line: l.no,
                    });
                }
            }
            if let Some(c) = ROUTE.captures(&l.text) {
                let route = c
                    .get(1)
                    .or_else(|| c.get(2))
                    .or_else(|| c.get(3))
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                if route.len() > 1
                    && !asked.contains(&route.to_ascii_lowercase())
                    && !before.contains(&route.to_ascii_lowercase())
                {
                    drafts.push(Draft {
                        rule: "new-route",
                        severity: Severity::Major,
                        title: format!(
                            "new route {route} at {}:{} that nothing in this session's prompts or claims asked for",
                            f.path, l.no
                        ),
                        claim_seq: None,
                        evidence: vec![ctx.line_pointer(&f.path, l.no)],
                        tier: Tier::Static,
                        fix_hint: Some("keep it and say so, or remove it".into()),
                        path: f.path.clone(),
                        line: l.no,
                    });
                }
            }
        }
    }

    fn dependency_rules(
        &self,
        ctx: &CheckContext,
        f: &FileChange,
        asked: &str,
        drafts: &mut Vec<Draft>,
    ) {
        let name = paths::file_name(&f.path);
        let mut added: Vec<(String, u64)> = Vec::new(); // (package, line in the working file)
        if name == "package.json" {
            let now = ctx.read_text(&f.path).and_then(|t| parse_package_deps(&t));
            let Some(now) = now else {
                ctx.note(format!(
                    "unverified: {} is not valid JSON, dependencies not read",
                    f.path
                ));
                return;
            };
            let before = match &f.status {
                Status::Added => BTreeSet::new(),
                _ => ctx
                    .baseline_text(&f.path)
                    .and_then(|t| parse_package_deps(&t))
                    .unwrap_or_default(),
            };
            let text = ctx.read_text(&f.path).unwrap_or_default();
            for dep in now.difference(&before) {
                let needle = format!("\"{dep}\"");
                let line = text
                    .lines()
                    .position(|l| l.contains(&needle))
                    .map_or(1, |i| i as u64 + 1);
                added.push((dep.clone(), line));
            }
        } else if name == "Cargo.toml" {
            let text = ctx.read_text(&f.path).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let mut section_at: Vec<String> = Vec::with_capacity(lines.len());
            let mut cur = String::new();
            for l in &lines {
                if let Some(c) = CARGO_SECTION.captures(l) {
                    cur = c[1].trim().to_string();
                }
                section_at.push(cur.clone());
            }
            for l in &f.added {
                let idx = l.no as usize - 1;
                let Some(section) = section_at.get(idx) else {
                    continue;
                };
                let header = CARGO_SECTION
                    .captures(&l.text)
                    .map(|c| c[1].trim().to_string());
                if let Some(h) = header {
                    if let Some(rest) = h
                        .strip_prefix("dependencies.")
                        .or_else(|| h.strip_prefix("dev-dependencies."))
                        .or_else(|| h.strip_prefix("build-dependencies."))
                        .or_else(|| h.strip_prefix("workspace.dependencies."))
                    {
                        added.push((rest.to_string(), l.no));
                    }
                } else if section.contains("dependencies")
                    && !section.contains("dependencies.")
                    && let Some(c) = CARGO_DEP.captures(&l.text)
                {
                    added.push((c[1].to_string(), l.no));
                }
            }
        } else if name.starts_with("requirements") && name.ends_with(".txt") {
            for l in &f.added {
                let t = l.text.trim();
                if t.is_empty() || t.starts_with('#') || t.starts_with('-') {
                    continue;
                }
                if let Some(c) = REQ_NAME.captures(t) {
                    added.push((c[1].to_ascii_lowercase(), l.no));
                }
            }
        } else if name == "go.mod" {
            for l in &f.added {
                if let Some(c) = GO_REQ.captures(&l.text) {
                    added.push((c[1].to_string(), l.no));
                }
            }
        } else {
            return;
        }
        for (dep, line) in added {
            if asked.contains(&dep.to_ascii_lowercase()) {
                continue;
            }
            drafts.push(Draft {
                rule: "new-dependency",
                severity: Severity::Major,
                title: format!(
                    "new dependency {dep} in {}:{line} that nothing in this session's prompts or claims asked for",
                    f.path
                ),
                claim_seq: None,
                evidence: vec![ctx.line_pointer(&f.path, line)],
                tier: Tier::Static,
                fix_hint: Some("keep it and say why, or remove it".into()),
                path: f.path.clone(),
                line,
            });
        }
    }
}

fn parse_package_deps(text: &str) -> Option<BTreeSet<String>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut out = BTreeSet::new();
    for key in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(o) = v.get(key).and_then(|d| d.as_object()) {
            out.extend(o.keys().cloned());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_rules() {
        let asked = "add a login page in src/auth and update billing.rs".to_string();
        assert!(mentioned(&asked, "src/auth/form.ts"));
        assert!(mentioned(&asked, "src/billing.rs"));
        assert!(!mentioned(&asked, "src/other/thing.ts"));
        // `src` alone is too generic to cover a whole tree.
        assert!(!mentioned("work in src", "src/x/y.ts"));
    }

    #[test]
    fn new_file_classes() {
        assert_eq!(
            classify_new_file("db/migrations/001.py", false).0,
            "unplanned-migration"
        );
        assert_eq!(
            classify_new_file("schema.sql", false).0,
            "unplanned-migration"
        );
        assert_eq!(
            classify_new_file("src/pages/home.tsx", true).0,
            "unplanned-page"
        );
        assert_eq!(classify_new_file("src/a.ts", true).0, "unplanned-file");
        assert_eq!(classify_new_file("src/a.ts", false).0, "orphan-file");
    }

    #[test]
    fn package_json_deps_are_read() {
        let d = parse_package_deps(r#"{"dependencies":{"a":"1"},"devDependencies":{"b":"2"}}"#)
            .unwrap();
        assert!(d.contains("a") && d.contains("b"));
        assert!(parse_package_deps("not json").is_none());
    }
}
