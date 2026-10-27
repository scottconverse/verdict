//! Every checker has a red fixture (it must report) and a green one (it must not). The `red_*`
//! tests run against `registry()`: with the `stub-checkers` feature that is the stubbed set, and
//! every one of them must then fail (see .github/workflows/fixtures-red-must-fail.yml).

mod common;

use common::*;
use verdict_checkers::{blocks, registry, stubbed_registry};
use verdict_core::Severity;

macro_rules! fixture_tests {
    ($($name:ident),*) => {
        mod red {
            use super::*;
            $(
                #[test]
                fn $name() {
                    check_red(&registry(), stringify!($name)).unwrap_or_else(|e| panic!("{e}"));
                }
            )*
        }
        mod green {
            use super::*;
            $(
                #[test]
                fn $name() {
                    check_green(&registry(), stringify!($name)).unwrap_or_else(|e| panic!("{e}"));
                }
            )*
        }
    };
}

fixture_tests!(
    baseline_diff,
    regressions,
    tests_weakened,
    claims_vs_code,
    unfinished,
    exec
);

#[test]
fn every_checker_has_both_fixtures() {
    for name in NAMES {
        for kind in ["red", "green"] {
            assert!(
                fixture_dir(name, kind).is_dir(),
                "fixtures/{name}/{kind} is missing"
            );
        }
        assert!(
            !expected(name).is_empty(),
            "fixtures/{name}/red/expect.txt lists no rule"
        );
    }
    assert_eq!(registry().len(), NAMES.len());
}

/// The proof that a red fixture depends on its checker: swap each checker for a stub that finds
/// nothing and the red fixture's check fails; the green one still passes.
#[test]
fn stubbed_checkers_fail_every_red_fixture() {
    let stubs = stubbed_registry();
    for name in NAMES {
        let r = check_red(&stubs, name);
        assert!(
            r.is_err(),
            "{name}: the red fixture still passed with the checker stubbed out"
        );
        assert!(
            check_green(&stubs, name).is_ok(),
            "{name}: a stub should leave the green fixture green"
        );
    }
}

/// Same inputs, same findings: ids, order and text, both on one tree run twice and on two
/// separately built copies of the fixture.
#[test]
fn findings_are_deterministic() {
    let real = verdict_checkers::real_registry();
    for name in NAMES {
        for kind in ["red", "green"] {
            let a = build(name, kind);
            let first =
                run_checker(&real, name, &a).unwrap_or_else(|u| panic!("{name}: {}", u.reason));
            let second = run_checker(&real, name, &a).unwrap();
            assert_eq!(first, second, "{name}/{kind}: two runs on one tree differ");
            let b = build(name, kind);
            let third = run_checker(&real, name, &b).unwrap();
            assert_eq!(
                first, third,
                "{name}/{kind}: two builds of the fixture differ"
            );
        }
    }
}

#[test]
fn only_blockers_block_and_the_design_table_holds() {
    let real = verdict_checkers::real_registry();
    let sev = |name: &str| -> Vec<(String, Severity)> {
        check_red(&real, name)
            .unwrap()
            .into_iter()
            .map(|f| (rule_of(&f.id).to_string(), f.severity))
            .collect()
    };
    // regressions always block
    assert!(
        sev("regressions")
            .iter()
            .all(|(_, s)| *s == Severity::Blocker)
    );
    // claims_vs_code: a contradiction blocks
    assert!(
        sev("claims_vs_code")
            .iter()
            .all(|(_, s)| *s == Severity::Blocker)
    );
    // baseline_diff and tests_weakened never block
    for name in ["baseline_diff", "tests_weakened"] {
        let findings = check_red(&real, name).unwrap();
        assert!(
            findings.iter().all(|f| !blocks(f)),
            "{name} must never block"
        );
    }
    // unfinished: only a called stub blocks
    let un = sev("unfinished");
    for (rule, s) in &un {
        assert_eq!(*s == Severity::Blocker, rule == "called-stub", "{rule}");
    }
    // exec: breaking the project blocks
    assert!(
        sev("exec")
            .iter()
            .any(|(r, s)| r == "broken-by-session" && *s == Severity::Blocker)
    );
}

#[test]
fn every_red_finding_has_a_checkable_pointer() {
    let real = verdict_checkers::real_registry();
    for name in NAMES {
        let b = build(name, "red");
        let findings = run_checker(&real, name, &b).unwrap();
        assert!(!findings.is_empty());
        for f in &findings {
            assert!(!f.evidence.is_empty(), "{}", f.id);
            assert!(f.id.starts_with(&format!("{name}:")), "{}", f.id);
            // A file:line pointer must be current right after the run.
            for p in &f.evidence {
                if let verdict_core::EvidencePointer::FileLine { .. } = p {
                    assert_eq!(
                        p.check(b.root()),
                        verdict_core::PointerStatus::Current,
                        "{}",
                        f.id
                    );
                }
            }
        }
    }
}
