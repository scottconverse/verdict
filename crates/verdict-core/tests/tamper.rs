mod common;

use common::*;
use verdict_core::*;

fn raw(t: &TempDir) -> rusqlite::Connection {
    rusqlite::Connection::open(t.path().join(".verdict").join("ledger.db")).unwrap()
}

fn drop_triggers(c: &rusqlite::Connection) {
    c.execute_batch(
        "DROP TRIGGER records_no_update; DROP TRIGGER records_no_delete; DROP TRIGGER records_chain;",
    )
    .unwrap();
}

fn verify(t: &TempDir) -> VerifyReport {
    Ledger::open(t.path()).unwrap().verify().unwrap()
}

#[test]
fn intact_ledger_verifies() {
    let t = TempDir::new("ok");
    let l = seeded(t.path());
    let r = l.verify().unwrap();
    assert!(r.is_ok(), "{}", r.summary());
    assert_eq!(r.records, 5);
    assert!(r.summary().starts_with("ledger OK: 5 records"));
}

#[test]
fn edited_content_names_the_record() {
    let t = TempDir::new("edit");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    c.execute(
        "UPDATE records SET body = replace(body, 'login route exists', 'login route is fully tested') WHERE seq = 3",
        [],
    )
    .unwrap();
    drop(c);
    let r = verify(&t);
    assert!(!r.is_ok());
    assert_eq!(r.first_broken_seq(), Some(3), "{}", r.summary());
    assert!(r.summary().contains("at record 3"));
}

#[test]
fn recomputed_hash_after_edit_breaks_the_next_record() {
    let t = TempDir::new("rehash");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    // The attacker edits record 2 and also fixes its own hash; record 3 no longer chains to it.
    let (ts, writer, kind, prev, sid): (String, String, String, String, Option<String>) = c
        .query_row(
            "SELECT ts, writer, kind, prev_hash, session_id FROM records WHERE seq = 2",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    let body: String = c
        .query_row("SELECT body FROM records WHERE seq = 2", [], |r| r.get(0))
        .unwrap();
    let body = body.replace("add a login page", "add nothing");
    let hash =
        verdict_core::ledger::record_hash(2, &ts, &writer, sid.as_deref(), &kind, &prev, &body);
    c.execute(
        "UPDATE records SET body = ?1, hash = ?2 WHERE seq = 2",
        rusqlite::params![body, hash],
    )
    .unwrap();
    drop(c);
    let r = verify(&t);
    assert_eq!(r.first_broken_seq(), Some(3), "{}", r.summary());
}

#[test]
fn deleted_middle_record_is_detected() {
    let t = TempDir::new("mid");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    c.execute("DELETE FROM records WHERE seq = 3", []).unwrap();
    drop(c);
    let r = verify(&t);
    assert!(!r.is_ok());
    assert_eq!(r.first_broken_seq(), Some(4), "{}", r.summary());
}

#[test]
fn truncated_tail_is_detected() {
    let t = TempDir::new("tail");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    c.execute("DELETE FROM records WHERE seq >= 4", []).unwrap();
    drop(c);
    let r = verify(&t);
    assert!(!r.is_ok());
    assert_eq!(r.first_broken_seq(), Some(4), "{}", r.summary());
    assert!(r.summary().contains("truncated"));
}

#[test]
fn truncated_tail_is_detected_even_if_the_sequence_counter_is_reset() {
    let t = TempDir::new("tail2");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    c.execute("DELETE FROM records WHERE seq >= 4", []).unwrap();
    c.execute("DELETE FROM sqlite_sequence WHERE name = 'records'", [])
        .unwrap();
    drop(c);
    let r = verify(&t);
    assert!(!r.is_ok(), "ledger.head must catch this");
    assert_eq!(r.first_broken_seq(), Some(4), "{}", r.summary());
}

#[test]
fn reordered_records_are_detected() {
    let t = TempDir::new("reorder");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    // Swap the positions of records 2 and 3 by swapping their sequence numbers.
    c.execute_batch(
        "UPDATE records SET seq = 99 WHERE seq = 2;
         UPDATE records SET seq = 2 WHERE seq = 3;
         UPDATE records SET seq = 3 WHERE seq = 99;",
    )
    .unwrap();
    drop(c);
    let r = verify(&t);
    assert!(!r.is_ok());
    assert_eq!(r.first_broken_seq(), Some(2), "{}", r.summary());
}

#[test]
fn removed_protection_triggers_are_reported() {
    let t = TempDir::new("trig");
    drop(seeded(t.path()));
    drop_triggers(&raw(&t));
    let r = verify(&t);
    assert!(!r.is_ok());
    assert!(r.summary().contains("trigger"), "{}", r.summary());
}

#[test]
fn a_dropped_table_is_caught_by_the_head_file() {
    let t = TempDir::new("table");
    drop(seeded(t.path()));
    raw(&t).execute_batch("DROP TABLE records").unwrap();
    let r = verify(&t);
    assert!(!r.is_ok(), "{}", r.summary());
    assert!(r.summary().contains("truncated"), "{}", r.summary());
}

#[test]
fn appending_after_tampering_does_not_hide_it() {
    let t = TempDir::new("after");
    drop(seeded(t.path()));
    let c = raw(&t);
    drop_triggers(&c);
    c.execute("UPDATE records SET ts = 'yesterday' WHERE seq = 2", [])
        .unwrap();
    drop(c);
    let mut l = Ledger::open(t.path()).unwrap();
    l.append(Writer::Agent, None, claim("later")).unwrap();
    assert_eq!(l.verify().unwrap().first_broken_seq(), Some(2));
}
