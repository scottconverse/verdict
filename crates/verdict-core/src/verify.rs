//! Ledger verification: detects a tampered, reordered or truncated ledger and says which
//! record broke.

use rusqlite::OptionalExtension;

use crate::ledger::{
    GENESIS_HASH, Ledger, LedgerError, RawRow, SQL_CHAIN, SQL_NO_DELETE, SQL_NO_UPDATE, parse_head,
    record_hash,
};
use crate::model::{Record, RecordKind, Writer};

/// One thing wrong with the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The record that broke, when the problem is about a specific record.
    pub seq: Option<u64>,
    /// What is wrong, in plain words.
    pub message: String,
}

/// The result of [`Ledger::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Records found.
    pub records: u64,
    /// Hash of the last record, if any.
    pub head_hash: Option<String>,
    /// Everything wrong, first break first. Empty means the ledger is intact.
    pub problems: Vec<Problem>,
}

impl VerifyReport {
    /// True when nothing is wrong.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// The first record that broke, if any.
    pub fn first_broken_seq(&self) -> Option<u64> {
        self.problems.iter().find_map(|p| p.seq)
    }

    /// Plain-text summary for the terminal.
    pub fn summary(&self) -> String {
        if self.is_ok() {
            let head = self.head_hash.as_deref().map_or("none", |h| &h[..12]);
            return format!(
                "ledger OK: {} records, hash chain unbroken, head {head}",
                self.records
            );
        }
        let mut out = String::new();
        for (i, p) in self.problems.iter().enumerate() {
            if i == 0 {
                out.push_str("LEDGER BROKEN");
            } else {
                out.push_str("also");
            }
            match p.seq {
                Some(seq) => out.push_str(&format!(" at record {seq}: {}\n", p.message)),
                None => out.push_str(&format!(": {}\n", p.message)),
            }
        }
        out.push_str(&format!("{} records read", self.records));
        out
    }
}

impl Ledger {
    /// Check the whole ledger: contiguous sequence numbers, every `prev_hash`, every record hash
    /// recomputed from its stored content, readable bodies whose type matches the kind, no missing
    /// tail (the database's own sequence counter and the `ledger.head` sidecar), that the
    /// append-only triggers are still in place, and that every row obeys the who-may-write-what
    /// rules and the hypothesis rule (so a row slipped in with raw SQL is caught too).
    ///
    /// All database reads happen in one read transaction, so a concurrent append cannot make a
    /// healthy ledger look truncated.
    pub fn verify(&self) -> Result<VerifyReport, LedgerError> {
        let mut problems = Vec::new();
        // Read the head file before the snapshot: it is only written after a commit, so whatever
        // it names is already inside the snapshot taken next.
        let head_text = self.head_file().map(std::fs::read_to_string);
        let conn = self.conn();
        let _snapshot = conn.unchecked_transaction()?;

        let has_table: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='records')",
            [],
            |r| r.get(0),
        )?;
        if !has_table {
            problems.push(Problem {
                seq: None,
                message: "the records table is missing".into(),
            });
            return Ok(VerifyReport {
                records: 0,
                head_hash: None,
                problems,
            });
        }

        for (name, expected) in [
            ("records_no_update", SQL_NO_UPDATE),
            ("records_no_delete", SQL_NO_DELETE),
            ("records_chain", SQL_CHAIN),
        ] {
            let sql: Option<String> = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                    [name],
                    |r| r.get(0),
                )
                .optional()?;
            match sql {
                None => problems.push(Problem {
                    seq: None,
                    message: format!("protection trigger {name} has been removed"),
                }),
                Some(s) if s != expected => problems.push(Problem {
                    seq: None,
                    message: format!("protection trigger {name} has been altered"),
                }),
                Some(_) => {}
            }
        }

        let rows = self.raw_rows()?;
        let mut expected_seq: i64 = 1;
        let mut prev = GENESIS_HASH.to_string();
        for row in &rows {
            if row.seq != expected_seq {
                problems.push(Problem {
                    seq: Some(row.seq.max(0) as u64),
                    message: if row.seq > expected_seq {
                        format!(
                            "records {expected_seq} to {} are missing (deleted or reordered)",
                            row.seq - 1
                        )
                    } else {
                        format!("sequence out of order: expected record {expected_seq}")
                    },
                });
            }
            expected_seq = row.seq + 1;
            if row.prev_hash != prev {
                problems.push(Problem {
                    seq: Some(row.seq as u64),
                    message: "hash chain broken: the previous-record hash does not match".into(),
                });
            }
            let recomputed = record_hash(
                row.seq as u64,
                &row.ts,
                &row.writer,
                row.session_id.as_deref(),
                &row.kind,
                &row.prev_hash,
                &row.body,
            );
            if recomputed != row.hash {
                problems.push(Problem {
                    seq: Some(row.seq as u64),
                    message: "content altered: the stored hash does not match the record".into(),
                });
            }
            match serde_json::from_str::<Record>(&row.body) {
                Err(e) => problems.push(Problem {
                    seq: Some(row.seq as u64),
                    message: format!("record body unreadable: {e}"),
                }),
                Ok(rec) if rec.kind().as_str() != row.kind => problems.push(Problem {
                    seq: Some(row.seq as u64),
                    message: format!(
                        "kind column says {} but the body is a {}",
                        row.kind,
                        rec.kind()
                    ),
                }),
                Ok(rec) => {
                    if let Err(message) = row_obeys_rules(row, &rec, &rows) {
                        problems.push(Problem {
                            seq: Some(row.seq as u64),
                            message,
                        });
                    }
                }
            }
            prev = row.hash.clone();
        }

        let last_seq = rows.last().map_or(0, |r| r.seq);
        // The database's own high-water mark for the sequence counter.
        let counter: Option<i64> = conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name='records'",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        if let Some(counter) = counter
            && counter > last_seq
        {
            problems.push(Problem {
                seq: Some((last_seq + 1) as u64),
                message: format!(
                    "ledger truncated: it ended at record {counter} but now stops at {last_seq}"
                ),
            });
        }
        // No head file (or no file path) means there is just no extra check.
        if let Some(Ok(text)) = head_text {
            match parse_head(&text) {
                None => problems.push(Problem {
                    seq: None,
                    message: "ledger.head is unreadable".into(),
                }),
                Some((seq, _)) if seq > last_seq => problems.push(Problem {
                    seq: Some((last_seq + 1) as u64),
                    message: format!(
                        "ledger truncated: ledger.head says record {seq} existed but the ledger stops at {last_seq}"
                    ),
                }),
                Some((seq, hash)) => {
                    if rows.iter().find(|r| r.seq == seq).is_some_and(|r| r.hash != hash) {
                        problems.push(Problem {
                            seq: Some(seq as u64),
                            message: "record differs from the hash saved in ledger.head".into(),
                        });
                    }
                }
            }
        }

        // Report the earliest break first.
        problems.sort_by_key(|p| p.seq.unwrap_or(u64::MAX));
        Ok(VerifyReport {
            records: rows.len() as u64,
            head_hash: rows.last().map(|r| r.hash.clone()),
            problems,
        })
    }
}

/// The who-may-write-what rules and the hypothesis rule, checked on a stored row. The ledger API
/// enforces these on append; this catches a row inserted around the API with raw SQL.
fn row_obeys_rules(row: &RawRow, rec: &Record, rows: &[RawRow]) -> Result<(), String> {
    let Some(writer) = Writer::parse(&row.writer) else {
        return Err(format!("unknown writer {:?}", row.writer));
    };
    let kind = rec.kind();
    if !writer.may_write(kind) {
        return Err(format!(
            "written by {writer}, who may not write a {kind} record (not made through the ledger)"
        ));
    }
    match rec {
        Record::Finding(f) if f.evidence.is_empty() => {
            return Err("a finding with no evidence pointer (must be a hypothesis)".into());
        }
        Record::Decision { who, .. } if *who != Writer::Human => {
            return Err("a decision whose `who` is not the human".into());
        }
        Record::Correction {
            target_seq,
            replacement,
            ..
        } => {
            let target = rows
                .iter()
                .find(|r| r.seq == *target_seq as i64 && r.seq < row.seq);
            let Some(target) = target else {
                return Err(format!(
                    "correction targets record {target_seq}, which is not an earlier record"
                ));
            };
            if !matches!(writer, Writer::Core | Writer::Human)
                && Writer::parse(&target.writer) != Some(writer)
            {
                return Err(format!(
                    "{writer} corrected record {target_seq}, written by {}",
                    target.writer
                ));
            }
            if let Some(rep) = replacement {
                let rep_kind = rep.kind();
                if rep_kind == RecordKind::Correction || !writer.may_write(rep_kind) {
                    return Err(format!("{writer} may not correct into a {rep_kind} record"));
                }
                match &**rep {
                    Record::Finding(f) if f.evidence.is_empty() => {
                        return Err("a correction into a finding with no evidence pointer".into());
                    }
                    Record::Decision { who, .. } if *who != Writer::Human => {
                        return Err(
                            "a correction into a decision whose `who` is not the human".into()
                        );
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}
