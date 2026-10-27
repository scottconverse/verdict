//! wiring: what a finding weighs depends on whether the item is ticked, and a CONNECTED item
//! writes its path into the check's detail. Built on the wiring fixtures, in temp repos.

mod common;

use common::*;
use verdict_checkers::{blocks, real_registry};
use verdict_core::{ClaimSource, Record, Severity, Writer};

fn add_claim(b: &mut Built, source: ClaimSource, text: &str) {
    let names = verdict_core::part1c::extract_names(text);
    b.ledger
        .append(
            Writer::Agent,
            Some(&b.session),
            Record::Claim {
                source,
                text: text.to_string(),
                names,
                prompt_ordinal: None,
            },
        )
        .unwrap();
}

/// An unticked plan line that names something missing is a `Nit`: it shows, it does not block.
/// The same line with a checked box blocks.
#[test]
fn unticked_plan_lines_do_not_block_and_ticked_ones_do() {
    let real = real_registry();
    let mut b = build("wiring/word_not_built", "green");
    add_claim(
        &mut b,
        ClaimSource::PlanLine,
        "- [ ] Add `importCsv` to `src/import.ts`",
    );
    let findings = run_checker(&real, "wiring", &b).unwrap_or_else(|u| panic!("{}", u.reason));
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(rule_of(&findings[0].id), "not_built");
    assert_eq!(findings[0].severity, Severity::Nit);
    assert!(!blocks(&findings[0]));

    let mut ticked = build("wiring/word_not_built", "green");
    add_claim(
        &mut ticked,
        ClaimSource::PlanLine,
        "- [x] Add `importCsv` to `src/import.ts`",
    );
    let findings = run_checker(&real, "wiring", &ticked).unwrap();
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].severity, Severity::Blocker);
    assert!(blocks(&findings[0]));
}

/// CONNECTED adds no finding; its path (the call, the route, the handler) is in the detail.
#[test]
fn connected_items_write_their_path_into_the_detail() {
    let real = real_registry();
    let b = build("wiring/fetch", "green");
    let ctx = context(&b);
    let checker = find(&real, "wiring");
    let findings = checker.run(&ctx).unwrap_or_else(|u| panic!("{}", u.reason));
    assert!(findings.is_empty(), "{findings:#?}");
    let notes = ctx.notes().join("\n");
    assert!(notes.contains("1 CONNECTED"), "{notes}");
    assert!(notes.contains("CONNECTED - /api/notes"), "{notes}");
    assert!(notes.contains("route POST /api/notes"), "{notes}");
    assert!(notes.contains("handler createNote"), "{notes}");
    assert!(notes.contains("src/api.ts"), "{notes}");
}

/// SKIPPED is never clean: with nothing blocking, the check is unavailable and says why.
#[test]
fn skipped_alone_makes_the_check_unavailable() {
    let real = real_registry();
    let b = build("wiring/word_skipped", "red");
    let u = run_checker(&real, "wiring", &b).expect_err("SKIPPED must not be a clean run");
    assert!(u.reason.contains("SKIPPED"), "{}", u.reason);
    assert!(u.reason.contains("svc.go"), "{}", u.reason);
}

/// A claim that names nothing to trace leaves the check unavailable, never clean.
#[test]
fn no_names_is_unavailable_not_clean() {
    let real = real_registry();
    let mut b = build("wiring/word_not_built", "green");
    // Replace the session's claims with one that names nothing: a fresh build with no
    // session.json claims is not possible, so the run uses a new session over the same tree.
    let mut ledger = verdict_core::Ledger::open_in_memory().unwrap();
    let started = verdict_core::start_session(
        &mut ledger,
        b.root(),
        &verdict_core::SessionParams {
            agent: "claude".into(),
            model: "fixture-model".into(),
            vendor: "anthropic".into(),
            home: Some(b.home.path().to_path_buf()),
        },
    )
    .unwrap();
    b.ledger = ledger;
    b.session = started.id;
    add_claim(&mut b, ClaimSource::FinalMessage, "Everything works now");
    let u = run_checker(&real, "wiring", &b).expect_err("nothing named must not be clean");
    assert!(
        u.reason.contains("names a URL, file or symbol"),
        "{}",
        u.reason
    );
}
