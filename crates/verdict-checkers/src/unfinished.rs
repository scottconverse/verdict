//! unfinished: work left half done, in lines added this session.
//!
//! Rules (the rule name is part of each finding id):
//!
//! * `called-stub`: a not-implemented body (`todo!()`, `unimplemented!()`, `raise
//!   NotImplementedError`, `throw new Error("not implemented")`) in a function that other code
//!   calls. The only blocking rule here (severity `Blocker`). "Called" is a name match over the
//!   tree, not a call graph: no parser yet.
//! * `stub`: the same body in a function nothing calls (Major).
//! * `todo`: TODO / FIXME / XXX (Minor).
//! * `sample-data`: lorem ipsum, example addresses, "John Doe" (Minor, not in tests).
//! * `debug-log`: `console.log`, `dbg!`, a debug-looking `print` (Minor, not in tests).
//! * `commented-out-code`: a comment that is code (Nit).
//! * `skipped-test`: `.skip(`, `xit(`, `@pytest.mark.skip`, `#[ignore]` (Minor).

use std::sync::LazyLock;

use regex::Regex;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::changes::Status;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::paths;

/// The unfinished checker.
pub struct Unfinished;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(
    STUB_BODY,
    r#"(?i)\b(?:todo|unimplemented)!\s*\(|raise\s+NotImplementedError\b|throw\s+new\s+Error\(\s*["'`][^"'`]*not\s+(?:yet\s+)?implemented|panic!\(\s*"not (?:yet )?implemented"#
);
re!(TODO, r"\b(?:TODO|FIXME|XXX)\b");
re!(
    SAMPLE,
    r"(?i)lorem ipsum|\b(?:foo|bar|test|user|john|jane)@example\.(?:com|org)\b|\bjohn doe\b|\bjane doe\b|\b(?:sample|dummy|fake|mock)[ _-]?data\b"
);
re!(
    DEBUG_LOG,
    r#"\bconsole\.(?:log|debug)\s*\(|\bdbg!\s*\(|\bprintln!\s*\(\s*"(?i:debug|here|xxx)|^\s*print\s*\(\s*f?["'][^"']*(?i:debug|\bhere\b|xxx)"#
);
re!(
    SKIPPED,
    r"\b(?:describe|it|test|context)\.skip\s*\(|(?:^|[^\w.$])(?:xit|xdescribe|xtest)\s*\(|@pytest\.mark\.skip\b|@unittest\.skip\b|#\[ignore\b|\bt\.Skip\s*\(|\bpytest\.skip\s*\("
);
re!(COMMENT_LEAD, r"^\s*(?://+|#+)\s?(.*)$");
re!(
    CODE_SHAPE,
    r"^(?:if|for|while|else|return|let|const|var|fn|def|class|import|from|use|pub|print|console\.|self\.|this\.|await|async|try|catch|except|match|switch)\b.*[;{}:)]\s*$|^[\w.\[\]]+\s*(?:\+|-)?=\s*[^=].*;\s*$|^[\w.]+\([^)]*\)\s*;\s*$|^(?:import\s+[\w.]+|from\s+[\w.]+\s+import\s+.+)\s*$"
);
re!(FN_DEF, r"\b(?:fn|def|function)\s+([A-Za-z_]\w*)");
re!(
    JS_METHOD_DEF,
    r"^\s*(?:(?:public|private|protected|static|async|pub)\s+)*([A-Za-z_]\w*)\s*\([^)]*\)\s*(?::\s*[\w<>\[\], |]+)?\s*\{"
);
re!(
    JS_ARROW_DEF,
    r"\b(?:const|let|var)\s+([A-Za-z_]\w*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|\w+)\s*(?::\s*[\w<>\[\], |]+)?\s*=>"
);

const NOT_NAMES: &[&str] = &[
    "if", "for", "while", "switch", "catch", "match", "return", "else",
];

impl Checker for Unfinished {
    fn name(&self) -> &'static str {
        "unfinished"
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
            let testy = paths::is_test_path(&f.path);
            let ext = paths::ext(&f.path);
            let full = ctx.read_text(&f.path);

            for l in &f.added {
                let t = l.text.as_str();
                let mut push =
                    |rule: &'static str,
                     severity: Severity,
                     title: String,
                     hint: &str,
                     extra: Vec<verdict_core::EvidencePointer>| {
                        let mut evidence = vec![ctx.line_pointer(&f.path, l.no)];
                        evidence.extend(extra);
                        drafts.push(Draft {
                            rule,
                            severity,
                            title: format!("{title} at {}:{}{suffix}", f.path, l.no),
                            claim_seq: None,
                            evidence,
                            tier: Tier::Static,
                            fix_hint: Some(hint.to_string()),
                            path: f.path.clone(),
                            line: l.no,
                        });
                    };

                if STUB_BODY.is_match(t) && !is_comment_line(t) {
                    let func = full
                        .as_deref()
                        .and_then(|text| enclosing_function(text, l.no));
                    let caller = func
                        .as_deref()
                        .and_then(|name| find_caller(ctx, name, &f.path, l.no));
                    match (func, caller) {
                        (Some(name), Some((cpath, cline))) => push(
                            "called-stub",
                            Severity::Blocker,
                            format!("stub body in `{name}`, which {cpath}:{cline} calls"),
                            "implement it, or stop calling it",
                            vec![ctx.line_pointer(&cpath, cline)],
                        ),
                        (Some(name), None) => push(
                            "stub",
                            Severity::Major,
                            format!("stub body in `{name}` (no caller found by name)"),
                            "implement it or delete it",
                            vec![],
                        ),
                        (None, _) => push(
                            "stub",
                            Severity::Major,
                            "stub body (enclosing function not found)".to_string(),
                            "implement it or delete it",
                            vec![],
                        ),
                    }
                    continue;
                }
                if TODO.is_match(t) {
                    push(
                        "todo",
                        Severity::Minor,
                        "TODO/FIXME left in added code".to_string(),
                        "finish it or record it as a task",
                        vec![],
                    );
                }
                if !testy && SAMPLE.is_match(t) && !is_comment_line(t) {
                    push(
                        "sample-data",
                        Severity::Minor,
                        "sample or placeholder data in added code".to_string(),
                        "replace it with real data or remove it",
                        vec![],
                    );
                }
                if !testy && DEBUG_LOG.is_match(t) && !is_comment_line(t) && ext != "sh" {
                    push(
                        "debug-log",
                        Severity::Minor,
                        "debug logging left in added code".to_string(),
                        "remove it",
                        vec![],
                    );
                }
                if SKIPPED.is_match(t) && !is_comment_line(t) {
                    push(
                        "skipped-test",
                        Severity::Minor,
                        "test skipped or ignored in added code".to_string(),
                        "enable it or delete it with a reason",
                        vec![],
                    );
                }
                if let Some(c) = COMMENT_LEAD.captures(t) {
                    let body = c[1].trim();
                    if !body.is_empty()
                        && !TODO.is_match(body)
                        && !body.starts_with('!')
                        && !body.starts_with("include")
                        && !body.starts_with("region")
                        && !body.starts_with("endregion")
                        && CODE_SHAPE.is_match(body)
                        && !(ext == "py" && t.trim_start().starts_with("#!"))
                    {
                        push(
                            "commented-out-code",
                            Severity::Nit,
                            "commented-out code".to_string(),
                            "delete it; version control remembers",
                            vec![],
                        );
                    }
                }
            }
        }
        Ok(finish(self.name(), drafts))
    }
}

fn is_comment_line(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("//") || t.starts_with('#') || t.starts_with('*') || t.starts_with("/*")
}

/// The name of the function whose body holds `line` (nearest definition above, within 80 lines).
fn enclosing_function(text: &str, line: u64) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let end = (line as usize).min(lines.len());
    let start = end.saturating_sub(80);
    for i in (start..end).rev() {
        let l = lines[i];
        if is_comment_line(l) {
            continue;
        }
        if let Some(c) = FN_DEF.captures(l) {
            return Some(c[1].to_string());
        }
        if let Some(c) = JS_ARROW_DEF.captures(l) {
            return Some(c[1].to_string());
        }
        if let Some(c) = JS_METHOD_DEF.captures(l)
            && !NOT_NAMES.contains(&&c[1])
        {
            return Some(c[1].to_string());
        }
    }
    None
}

/// A line of production code that calls `name(`, other than its definition and the stub line.
fn find_caller(
    ctx: &CheckContext,
    name: &str,
    stub_path: &str,
    stub_line: u64,
) -> Option<(String, u64)> {
    if name.len() < 3 {
        return None;
    }
    let call = Regex::new(&format!(r"\b{}\s*\(", regex::escape(name))).ok()?;
    for (path, text) in &ctx.tree().text {
        if !paths::is_code(path) || paths::is_test_path(path) || paths::is_noise_path(path) {
            continue;
        }
        for (i, l) in text.lines().enumerate() {
            let no = i as u64 + 1;
            if path == stub_path && no == stub_line {
                continue;
            }
            if !call.is_match(l) || is_comment_line(l) {
                continue;
            }
            let is_def = FN_DEF.captures(l).is_some_and(|c| &c[1] == name)
                || JS_ARROW_DEF.captures(l).is_some_and(|c| &c[1] == name)
                || JS_METHOD_DEF
                    .captures(l)
                    .is_some_and(|c| &c[1] == name && !l.contains('.'));
            if !is_def {
                return Some((path.clone(), no));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_enclosing_function_in_rust_python_and_js() {
        assert_eq!(
            enclosing_function("fn a() {}\nfn charge(x: i32) {\n    todo!()\n}", 3).as_deref(),
            Some("charge")
        );
        assert_eq!(
            enclosing_function("def charge(x):\n    raise NotImplementedError", 2).as_deref(),
            Some("charge")
        );
        assert_eq!(
            enclosing_function(
                "const charge = async (x) => {\n  throw new Error('not implemented');\n}",
                2
            )
            .as_deref(),
            Some("charge")
        );
        assert_eq!(
            enclosing_function(
                "class A {\n  charge(x) {\n    throw new Error('Not implemented');\n  }\n}",
                3
            )
            .as_deref(),
            Some("charge")
        );
    }

    #[test]
    fn stub_regex_matches_the_listed_forms() {
        for s in [
            "todo!()",
            "unimplemented!(\"x\")",
            "raise NotImplementedError",
            "throw new Error(\"not implemented\")",
            "throw new Error('Not implemented yet')",
        ] {
            assert!(STUB_BODY.is_match(s), "{s}");
        }
        assert!(!STUB_BODY.is_match("fn todo_list() {}"));
    }

    #[test]
    fn commented_out_code_is_told_from_prose() {
        let code = |s: &str| {
            COMMENT_LEAD
                .captures(s)
                .is_some_and(|c| CODE_SHAPE.is_match(c[1].trim()))
        };
        assert!(code("    // let x = compute(1);"));
        assert!(code("    # import os"));
        assert!(code("// return total;"));
        assert!(!code("// return the total to the caller"));
        assert!(!code("# this is why we wait"));
    }
}
