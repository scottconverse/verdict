mod common;

use common::*;
use verdict_core::*;

const SID: &str = "s-test000001";

#[test]
fn golden_report_with_no_checkers() {
    let t = TempDir::new("golden");
    let mut l = seeded(t.path());
    l.append(
        Writer::Adapter,
        Some(SID),
        Record::CheckUnavailable {
            component: "core".into(),
            reason: "timed out after 20s on Stop".into(),
        },
    )
    .unwrap();
    let _ = l.append(
        Writer::Agent,
        Some(SID),
        Record::Decision {
            decision: DecisionKind::CommitAnyway,
            choice: "commit".into(),
            reason: Some("x".into()),
            who: Writer::Human,
        },
    );
    l.append(
        Writer::Desk,
        Some(SID),
        Record::Finding(finding("h1", vec![])),
    )
    .unwrap();
    let text = render_report(&l, None).unwrap();
    // The ledger hash depends on the temp path only through nothing; hashes are fixed by the clock.
    insta::assert_snapshot!(text);
}

#[test]
fn report_never_says_verified_or_clean_and_ends_with_not_established() {
    let t = TempDir::new("honest");
    let l = seeded(t.path());
    let text = render_report(&l, Some(SID)).unwrap();
    assert!(text.starts_with("VERDICT: UNVERIFIED - nothing was checked"));
    assert!(!text.contains("VERIFIED (") && !text.contains("CLEAR TO SHIP"));
    let last_section = text.rfind("NOT ESTABLISHED").unwrap();
    assert!(
        !text[last_section..].contains("\n\n"),
        "NOT ESTABLISHED is the last section"
    );
    assert!(text.contains("login route exists") && text.contains("add a login page"));
}

#[test]
fn unknown_session_is_an_error() {
    let t = TempDir::new("nosess");
    let l = seeded(t.path());
    assert!(render_report(&l, Some("s-nope")).is_err());
}
