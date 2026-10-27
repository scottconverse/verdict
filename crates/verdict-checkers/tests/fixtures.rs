//! Every checker has a red fixture (it must report) and a green one (it must not). The `red_*`
//! tests run against `registry()`: with the `stub-checkers` feature that is the stubbed set, and
//! every one of them must then fail (see .github/workflows/fixtures-red-must-fail.yml).

mod common;

use common::*;
use verdict_checkers::{blocks, registry, stubbed_registry};
use verdict_core::Severity;

/// Run every case of a checker whose fixtures are `fixtures/<checker>/<case>/{red,green}`. Prints
/// one line per case (`<checker>-case <case> red: FAILED (...)` or `... passed`), which the
/// stubbed-checkers workflow reads, and fails if any case failed.
fn run_cases(checker: &str, kind: &str) {
    let cases = fixtures_of(checker);
    assert!(!cases.is_empty(), "fixtures/{checker}/ holds no case");
    let mut failed = Vec::new();
    for fixture in &cases {
        let case = fixture.split_once('/').map_or(fixture.as_str(), |(_, c)| c);
        let r = match kind {
            "red" => check_red(&registry(), fixture)
                .map(|f| f.into_iter().map(|f| f.title).collect::<Vec<_>>()),
            _ => check_green(&registry(), fixture).map(|()| Vec::new()),
        };
        match r {
            Ok(titles) => {
                println!("{checker}-case {case} {kind}: passed");
                for t in titles {
                    println!("    {t}");
                }
            }
            Err(e) => {
                println!("{checker}-case {case} {kind}: FAILED ({e})");
                failed.push(format!("{case}: {e}"));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} {checker} {kind} case(s) failed:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}

macro_rules! fixture_tests {
    (flat: $($name:ident),*; cases: $($cased:ident),*) => {
        mod red {
            use super::*;
            $(
                #[test]
                fn $name() {
                    check_red(&registry(), stringify!($name)).unwrap_or_else(|e| panic!("{e}"));
                }
            )*
            $(
                #[test]
                fn $cased() {
                    run_cases(stringify!($cased), "red");
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
            $(
                #[test]
                fn $cased() {
                    run_cases(stringify!($cased), "green");
                }
            )*
        }
    };
}

fixture_tests!(
    flat: baseline_diff,
    regressions,
    tests_weakened,
    claims_vs_code,
    unfinished,
    exec,
    mutation,
    firstrun,
    release;
    cases: wiring
);

#[test]
fn every_checker_has_both_fixtures() {
    for name in NAMES {
        assert!(!fixtures_of(name).is_empty(), "fixtures/{name} is missing");
    }
    for fixture in all_fixtures() {
        for kind in ["red", "green"] {
            assert!(
                fixture_dir(&fixture, kind).is_dir(),
                "fixtures/{fixture}/{kind} is missing"
            );
        }
        assert!(
            !expected(&fixture).is_empty(),
            "fixtures/{fixture}/red/expect.txt lists no rule"
        );
    }
    assert_eq!(registry().len(), NAMES.len());
}

/// The wiring cases the design asks for: one per framework and one per bad word.
#[test]
fn wiring_has_every_required_case() {
    let cases = fixtures_of("wiring");
    for want in [
        "fetch",
        "openapi_ts",
        "tanstack_start",
        "tanstack_router",
        "kysely",
        "mcp_ts",
        "tauri",
        "react",
        "word_broken",
        "word_not_built",
        "word_half_built",
        "word_skipped",
    ] {
        assert!(
            cases.contains(&format!("wiring/{want}")),
            "fixtures/wiring/{want} is missing (have {cases:?})"
        );
    }
}

/// The proof that a red fixture depends on its checker: swap each checker for a stub that finds
/// nothing and the red fixture's check fails; the green one still passes.
#[test]
fn stubbed_checkers_fail_every_red_fixture() {
    let stubs = stubbed_registry();
    for name in all_fixtures() {
        let name = name.as_str();
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
    for fixture in all_fixtures() {
        let name = fixture.as_str();
        let checker = checker_of(name);
        for kind in ["red", "green"] {
            let a = build(name, kind);
            let first = run_checker(&real, checker, &a).map_err(|u| u.reason);
            let second = run_checker(&real, checker, &a).map_err(|u| u.reason);
            assert_eq!(first, second, "{name}/{kind}: two runs on one tree differ");
            let b = build(name, kind);
            let third = run_checker(&real, checker, &b).map_err(|u| u.reason);
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
    // wiring: every red case's claim is ticked, so its finding blocks
    for fixture in fixtures_of("wiring") {
        if expected(&fixture) == [EXPECT_UNAVAILABLE] {
            continue;
        }
        for f in check_red(&real, &fixture).unwrap() {
            assert_eq!(f.severity, Severity::Blocker, "{fixture}: {}", f.title);
            assert!(
                ["broken", "not_built", "half_built"].contains(&rule_of(&f.id)),
                "{}",
                f.id
            );
        }
    }
}

#[test]
fn every_red_finding_has_a_checkable_pointer() {
    let real = verdict_checkers::real_registry();
    for fixture in all_fixtures() {
        let name = fixture.as_str();
        if expected(name) == [EXPECT_UNAVAILABLE] {
            continue;
        }
        let checker = checker_of(name);
        let b = build(name, "red");
        let findings = run_checker(&real, checker, &b).unwrap();
        assert!(!findings.is_empty());
        for f in &findings {
            assert!(!f.evidence.is_empty(), "{}", f.id);
            assert!(f.id.starts_with(&format!("{checker}:")), "{}", f.id);
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
