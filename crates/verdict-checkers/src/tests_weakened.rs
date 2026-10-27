//! tests_weakened: a session that made the tests easier to pass.
//!
//! Rules (never block; severity `Major`, so the verdict reads NEEDS YOU):
//!
//! * `deleted-test`: a test file was deleted. Not reported when the file it tests was deleted in
//!   the same session (clean-up, not weakening); the check's detail says so.
//! * `tests-removed` / `assertions-removed`: a changed test file has fewer test cases (or
//!   assertions) than at the baseline.
//! * `commented-out-test`: an assertion or test declaration was commented out. Many lines in one
//!   file fold into `file-commented-out`.
//! * `constant-assertion`: an added assertion that cannot fail (`assert True`, `assertEqual(1, 1)`,
//!   `expect(x).toBe(x)`).

use std::sync::LazyLock;

use regex::Regex;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::changes::{FileChange, Status};
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::paths;

/// The tests_weakened checker.
pub struct TestsWeakened;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).expect("static regex"));
    };
}

re!(
    ASSERT,
    r"\bassert\w*\s*!?\s*\(|\bassert\s+\S|(?:^|[^.\w])expect\s*\(|\.should\b|\bXCTAssert\w*\(|\bt\.(?:Error|Errorf|Fatal|Fatalf)\(|\bEXPECT_\w+\(|\bASSERT_\w+\(|\bAssert\.\w+\("
);
re!(
    TEST_DECL,
    r"(?:^|[^.\w])(?:it|test|describe)\s*\(|\bdef\s+test_\w+|#\[(?:\w+::)?test\]|\bfunc\s+Test\w+|@Test\b"
);
re!(
    TWO_ARG,
    r"\bassert\w*\s*!?\s*\(\s*([^,()]+?)\s*,\s*([^,()]+?)\s*\)"
);
re!(PY_EQ, r"^\s*assert\s+(\S+?)\s*==\s*(\S+?)\s*(?:,.*)?$");
re!(
    ONE_ARG_TRUE,
    r"\bassert\w*\s*!?\s*\(\s*(?:True|true|1)\s*\)|^\s*assert\s+(?:True|1)\s*(?:,.*)?$"
);
re!(
    EXPECT_EQ,
    r"\bexpect\s*\(\s*(.+?)\s*\)\s*\.\s*(?:toBe|toEqual|toStrictEqual)\s*\(\s*(.+?)\s*\)"
);
re!(
    EXPECT_TRUE,
    r"\bexpect\s*\(\s*(?:true|1)\s*\)\s*\.\s*(?:toBeTruthy|toBe\(\s*true\s*\))"
);
re!(COMMENT_BODY, r"^\s*(?://+|#+)\s?(.*)$");

fn comment_body(line: &str) -> Option<&str> {
    // COMMENT_BODY always captures group 1 when it matches.
    COMMENT_BODY
        .captures(line)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
}

/// True when `line` is an assertion that cannot fail.
pub fn is_constant_assertion(line: &str) -> bool {
    if comment_body(line).is_some() {
        return false;
    }
    if ONE_ARG_TRUE.is_match(line) || EXPECT_TRUE.is_match(line) {
        return true;
    }
    for re in [&*TWO_ARG, &*PY_EQ, &*EXPECT_EQ] {
        if let Some(c) = re.captures(line) {
            let (a, b) = (c[1].trim(), c[2].trim());
            if !a.is_empty() && a == b {
                return true;
            }
        }
    }
    false
}

/// The name of the thing a test file tests: `test_foo.py`, `foo_test.go`, `foo.spec.ts`,
/// `FooTest.java` all give `foo`.
pub(crate) fn subject_stem(path: &str) -> String {
    let mut s = paths::stem(path);
    // `foo.test` / `foo.spec` (the extension was `.ts`)
    for suffix in [".test", ".spec"] {
        if let Some(x) = s.strip_suffix(suffix) {
            s = x.to_string();
        }
    }
    let lower = s.to_ascii_lowercase();
    let lower = lower
        .strip_prefix("test_")
        .or_else(|| lower.strip_suffix("_test"))
        .or_else(|| lower.strip_suffix("_spec"))
        .or_else(|| lower.strip_suffix("tests"))
        .or_else(|| lower.strip_suffix("test"))
        .unwrap_or(&lower);
    lower.trim_matches('_').to_string()
}

fn count(re: &Regex, lines: &[crate::changes::Line]) -> usize {
    lines
        .iter()
        .filter(|l| re.is_match(&l.text) && comment_body(&l.text).is_none())
        .count()
}

impl Checker for TestsWeakened {
    fn name(&self) -> &'static str {
        "tests_weakened"
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
                "the baseline is a folder hash: it cannot say which tests existed before, so \
                 deleted or weakened tests cannot be told apart from new ones",
            ));
        }
        for c in &changes.caveats {
            ctx.note(c.clone());
        }
        ctx.note(
            "unverified: block comments and docstrings around tests are not read; \
             test frameworks are recognized by line patterns, not a parser",
        );
        let exclude = ctx.exclude();
        let deleted_subjects: Vec<String> = changes
            .files
            .iter()
            .filter(|f| matches!(f.status, Status::Deleted) && !paths::is_test_path(&f.path))
            .map(|f| paths::stem(&f.path).to_ascii_lowercase())
            .collect();
        let mut drafts: Vec<Draft> = Vec::new();

        for f in &changes.files {
            if paths::is_noise_path(&f.path) || paths::is_excluded(&f.path, &exclude) {
                continue;
            }
            match &f.status {
                Status::Deleted => {
                    if !paths::is_test_path(&f.path) || f.binary {
                        continue;
                    }
                    let subject = subject_stem(&f.path);
                    if !subject.is_empty() && deleted_subjects.contains(&subject) {
                        ctx.note(format!(
                            "clean-up, not weakening: {} was deleted together with the file it tests",
                            f.path
                        ));
                        continue;
                    }
                    drafts.push(Draft {
                        rule: "deleted-test",
                        severity: Severity::Major,
                        title: format!(
                            "test file {} was deleted and the file it tests still exists",
                            f.path
                        ),
                        claim_seq: None,
                        evidence: vec![ctx.diff_pointer(&format!("D\t{}", f.path))],
                        tier: Tier::Static,
                        fix_hint: Some(
                            "restore the test, or say why the behavior it covered is gone".into(),
                        ),
                        path: f.path.clone(),
                        line: 0,
                    });
                }
                Status::Added | Status::Modified | Status::Renamed(_) => {
                    self.changed_file(ctx, f, &mut drafts);
                }
                Status::Unknown => {}
            }
        }
        Ok(finish(self.name(), drafts))
    }
}

impl TestsWeakened {
    fn changed_file(&self, ctx: &CheckContext, f: &FileChange, drafts: &mut Vec<Draft>) {
        if f.binary || !paths::is_code(&f.path) {
            return;
        }
        let is_rust_inline = paths::ext(&f.path) == "rs"
            && ctx
                .read_text(&f.path)
                .is_some_and(|t| t.contains("#[cfg(test)]"));
        if !paths::is_test_path(&f.path) && !is_rust_inline {
            return;
        }

        // 1. Assertions on constants, in lines added now.
        for l in &f.added {
            if is_constant_assertion(&l.text) {
                drafts.push(Draft {
                    rule: "constant-assertion",
                    severity: Severity::Major,
                    title: format!("assertion that cannot fail at {}:{}", f.path, l.no),
                    claim_seq: None,
                    evidence: vec![ctx.line_pointer(&f.path, l.no)],
                    tier: Tier::Static,
                    fix_hint: Some("assert on a value the code under test produces".into()),
                    path: f.path.clone(),
                    line: l.no,
                });
            }
        }

        // 2. Test declarations or assertions commented out (the same text was removed).
        let removed_trim: Vec<&str> = f.removed.iter().map(|l| l.text.trim()).collect();
        let mut commented: Vec<u64> = Vec::new();
        for l in &f.added {
            if let Some(body) = comment_body(&l.text) {
                let body = body.trim();
                if (ASSERT.is_match(body) || TEST_DECL.is_match(body))
                    && removed_trim.contains(&body)
                {
                    commented.push(l.no);
                }
            }
        }
        if commented.len() > 3 {
            let first = commented[0];
            drafts.push(Draft {
                rule: "file-commented-out",
                severity: Severity::Major,
                title: format!(
                    "{} test or assertion lines were commented out in {} (first at line {first})",
                    commented.len(),
                    f.path
                ),
                claim_seq: None,
                evidence: vec![ctx.line_pointer(&f.path, first)],
                tier: Tier::Static,
                fix_hint: Some(
                    "uncomment them and fix the failure, or delete them with a reason".into(),
                ),
                path: f.path.clone(),
                line: first,
            });
        } else {
            for no in &commented {
                drafts.push(Draft {
                    rule: "commented-out-test",
                    severity: Severity::Major,
                    title: format!("test or assertion commented out at {}:{no}", f.path),
                    claim_seq: None,
                    evidence: vec![ctx.line_pointer(&f.path, *no)],
                    tier: Tier::Static,
                    fix_hint: Some(
                        "uncomment it and fix the failure, or delete it with a reason".into(),
                    ),
                    path: f.path.clone(),
                    line: *no,
                });
            }
        }

        // 3. Fewer test cases or assertions than before. Lines commented out were counted above.
        let live_removed_tests = count(&TEST_DECL, &f.removed);
        let live_added_tests = count(&TEST_DECL, &f.added);
        let live_removed_asserts = count(&ASSERT, &f.removed);
        let live_added_asserts = count(&ASSERT, &f.added);
        let commented_tests = commented
            .iter()
            .filter(|n| {
                f.added
                    .iter()
                    .find(|l| l.no == **n)
                    .and_then(|l| comment_body(&l.text))
                    .is_some_and(|b| TEST_DECL.is_match(b))
            })
            .count();
        let commented_asserts = commented.len().saturating_sub(commented_tests);
        let tests_gone = live_removed_tests.saturating_sub(live_added_tests + commented_tests);
        let asserts_gone =
            live_removed_asserts.saturating_sub(live_added_asserts + commented_asserts);
        let at = f
            .removed
            .first()
            .map(|l| l.no)
            .or_else(|| f.added.first().map(|l| l.no))
            .unwrap_or(1);
        let removed_text = f
            .removed
            .iter()
            .find(|l| ASSERT.is_match(&l.text) || TEST_DECL.is_match(&l.text))
            .map(|l| l.text.clone())
            .unwrap_or_default();
        if tests_gone > 0 {
            drafts.push(Draft {
                rule: "tests-removed",
                severity: Severity::Major,
                title: format!(
                    "{} has {tests_gone} fewer test case(s) than at the baseline ({asserts_gone} fewer assertion(s))",
                    f.path
                ),
                claim_seq: None,
                evidence: vec![
                    ctx.line_pointer(&f.path, at),
                    ctx.diff_pointer(&format!("-{removed_text}")),
                ],
                tier: Tier::Static,
                fix_hint: Some("restore the test cases, or say why the behavior they covered is gone".into()),
                path: f.path.clone(),
                line: at,
            });
        } else if asserts_gone > 0 {
            drafts.push(Draft {
                rule: "assertions-removed",
                severity: Severity::Major,
                title: format!(
                    "{} has {asserts_gone} fewer assertion(s) than at the baseline",
                    f.path
                ),
                claim_seq: None,
                evidence: vec![
                    ctx.line_pointer(&f.path, at),
                    ctx.diff_pointer(&format!("-{removed_text}")),
                ],
                tier: Tier::Static,
                fix_hint: Some("restore the assertions, or say why they no longer apply".into()),
                path: f.path.clone(),
                line: at,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_assertions_are_found() {
        for l in [
            "    assert True",
            "assertTrue(true);",
            "self.assertEqual(1, 1)",
            "assert_eq!(2, 2);",
            "assert!(true);",
            "expect(true).toBe(true)",
            "expect(value).toBe(value)",
            "assert x == x",
        ] {
            assert!(is_constant_assertion(l), "{l}");
        }
        for l in [
            "assert result == 5",
            "assert_eq!(add(1, 2), 3);",
            "expect(add(1,2)).toBe(3)",
            "// assert True",
            "assert!(ready);",
        ] {
            assert!(!is_constant_assertion(l), "{l}");
        }
    }

    #[test]
    fn subject_stems() {
        for (p, s) in [
            ("tests/test_billing.py", "billing"),
            ("pkg/billing_test.go", "billing"),
            ("src/billing.test.ts", "billing"),
            ("src/Billing.spec.js", "billing"),
            ("a/BillingTest.java", "billing"),
        ] {
            assert_eq!(subject_stem(p), s, "{p}");
        }
    }
}
