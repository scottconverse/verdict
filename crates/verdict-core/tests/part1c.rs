mod common;

use verdict_core::part1c::*;
use verdict_core::*;

#[test]
fn claim_lines_must_name_a_url_file_or_symbol() {
    let ok = [
        (
            "The login page at https://example.com/login shows an error.",
            "https://example.com/login",
        ),
        (
            "src/auth/session.rs rejects an expired token",
            "src/auth/session.rs",
        ),
        ("validate_token returns Err on expiry", "validate_token"),
        ("Ledger::append refuses agent findings", "Ledger::append"),
        ("`run_checks` is called from main", "run_checks"),
        ("README.md lists the install steps", "README.md"),
        ("C:\\work\\app\\main.py exists", "C:\\work\\app\\main.py"),
    ];
    for (line, expect) in ok {
        let names = check_claim_line(line, &[]).unwrap_or_else(|e| panic!("{line}: {e}"));
        assert!(names.iter().any(|n| n == expect), "{line}: {names:?}");
    }
    let refused = [
        "it works",
        "the app is better now, e.g. nicer and/or faster",
        "",
        "   ",
        "two lines\nsecond line src/a.rs",
    ];
    for line in refused {
        assert!(
            check_claim_line(line, &[]).is_err(),
            "should refuse {line:?}"
        );
    }
}

#[test]
fn given_names_must_appear_in_the_line() {
    assert!(check_claim_line("the footer shows the date", &["footer.html".into()]).is_err());
    let names = check_claim_line("footer.html shows the date", &["footer.html".into()]).unwrap();
    assert_eq!(names, vec!["footer.html"]);
}

#[test]
fn done_when_that_cannot_be_wrong_is_refused() {
    for vague in [
        "it works better",
        "it feels nicer",
        "everything is good",
        "src/app.rs is faster",
        "the page looks cleaner",
    ] {
        let e = check_done_when(vague).unwrap_err();
        assert!(e.contains("cannot be wrong"), "{vague}: {e}");
    }
    for fine in [
        "http://localhost:3000/ shows the text \"Welcome back\"",
        "`cargo test` prints 0 failed",
        "src/app.rs compiles and the page loads in under 2 seconds",
        "the export has 12 rows",
    ] {
        assert!(check_done_when(fine).is_ok(), "{fine}");
    }
}

#[test]
fn open_decisions_list_hooks_and_shrink_when_the_human_answers() {
    let mut l = Ledger::open_in_memory().unwrap();
    let before = open_decisions(&l, None).unwrap();
    assert_eq!(before.len(), 2);
    assert!(
        before
            .iter()
            .all(|d| d.decision == DecisionKind::InstallHooks)
    );
    l.append(
        Writer::Human,
        None,
        Record::Decision {
            decision: DecisionKind::InstallHooks,
            choice: "codex".into(),
            reason: None,
            who: Writer::Human,
        },
    )
    .unwrap();
    let after = open_decisions(&l, None).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].key, "install_hooks:claude_code");
    assert_eq!(current_mode(&l).unwrap(), "shadow");
}

#[test]
fn a_claim_naming_something_the_prompt_did_not_is_an_open_decision() {
    let mut l = Ledger::open_in_memory().unwrap();
    let s = "s-test000002".to_string();
    l.append(Writer::Adapter, Some(&s), common::session_record(&s))
        .unwrap();
    l.append(
        Writer::Adapter,
        Some(&s),
        Record::Prompt {
            ordinal: 1,
            text: "fix the login page".into(),
            agent: "claude".into(),
            model: "m".into(),
        },
    )
    .unwrap();
    l.append(
        Writer::Agent,
        Some(&s),
        Record::Claim {
            source: ClaimSource::PromptTranslation,
            text: "src/billing.rs charges the card".into(),
            names: vec!["src/billing.rs".into()],
            prompt_ordinal: Some(1),
        },
    )
    .unwrap();
    let open = open_decisions(&l, Some(&s)).unwrap();
    assert!(
        open.iter()
            .any(|d| d.decision == DecisionKind::AcceptPromptTranslation),
        "{open:?}"
    );
}

#[test]
fn given_names_cannot_hide_a_file_the_line_names() {
    let names = check_claim_line("src/billing.rs charges the card", &["charges".into()]).unwrap();
    assert!(names.iter().any(|n| n == "src/billing.rs"), "{names:?}");
}

fn ledger_with_two_prompts(claim_names: Vec<String>) -> (Ledger, String) {
    let mut l = Ledger::open_in_memory().unwrap();
    let s = "s-test000003".to_string();
    l.append(Writer::Adapter, Some(&s), common::session_record(&s))
        .unwrap();
    for (ordinal, text) in [
        (1, "fix login"),
        (2, "explain src/billing.rs; do not change it"),
    ] {
        l.append(
            Writer::Adapter,
            Some(&s),
            Record::Prompt {
                ordinal,
                text: text.into(),
                agent: "claude".into(),
                model: "m".into(),
            },
        )
        .unwrap();
    }
    l.append(
        Writer::Agent,
        Some(&s),
        Record::Claim {
            source: ClaimSource::PromptTranslation,
            text: "src/billing.rs charges the card".into(),
            names: claim_names,
            prompt_ordinal: Some(1),
        },
    )
    .unwrap();
    (l, s)
}

#[test]
fn a_translation_is_checked_against_its_own_prompt_only() {
    let (l, s) = ledger_with_two_prompts(vec!["src/billing.rs".into()]);
    let open = open_decisions(&l, Some(&s)).unwrap();
    assert!(
        open.iter()
            .any(|d| d.decision == DecisionKind::AcceptPromptTranslation),
        "{open:?}"
    );
}

#[test]
fn a_short_names_list_stored_in_the_ledger_cannot_hide_a_target() {
    let (l, s) = ledger_with_two_prompts(vec!["charges".into()]);
    let open = open_decisions(&l, Some(&s)).unwrap();
    assert!(
        open.iter()
            .any(|d| d.decision == DecisionKind::AcceptPromptTranslation),
        "{open:?}"
    );
}
