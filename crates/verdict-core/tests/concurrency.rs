mod common;

use common::*;
use verdict_core::*;

#[test]
fn verify_during_concurrent_appends_never_reports_a_false_break() {
    // Codex review finding 3: a verify racing an append must not see "truncated".
    let t = TempDir::new("race");
    drop(seeded(t.path()));
    let root = t.path().to_path_buf();
    let writer = std::thread::spawn(move || {
        let mut l = Ledger::open(&root).unwrap();
        for i in 0..200 {
            l.append(Writer::Agent, None, claim(&format!("claim {i}")))
                .unwrap();
        }
    });
    let reader = Ledger::open(t.path()).unwrap();
    let mut checks = 0;
    while !writer.is_finished() {
        let r = reader.verify().unwrap();
        assert!(r.is_ok(), "false break during appends: {}", r.summary());
        checks += 1;
    }
    writer.join().unwrap();
    let r = reader.verify().unwrap();
    assert!(r.is_ok(), "{}", r.summary());
    assert_eq!(r.records, 205);
    assert!(checks > 0);
}

#[test]
fn checkers_and_desks_may_record_that_a_check_was_unavailable() {
    let mut l = Ledger::open_in_memory().unwrap();
    for w in [Writer::Adapter, Writer::Core, Writer::Checker, Writer::Desk] {
        l.append(
            w,
            None,
            Record::CheckUnavailable {
                component: "lint".into(),
                reason: "tool missing".into(),
            },
        )
        .unwrap_or_else(|e| panic!("{w}: {e}"));
    }
    let refused = l.append(
        Writer::Agent,
        None,
        Record::CheckUnavailable {
            component: "lint".into(),
            reason: "skip it".into(),
        },
    );
    assert!(matches!(refused, Err(LedgerError::Refused { .. })));
}
