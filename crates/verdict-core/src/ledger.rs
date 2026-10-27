//! The append-only, hash-chained evidence ledger.
//!
//! Storage is SQLite (WAL) at `<project>/.verdict/ledger.db`. Each row carries a monotonic sequence
//! number and a blake3 hash over its own content and the previous row's hash. Database triggers
//! reject `UPDATE` and `DELETE` and any `INSERT` that does not extend the chain, so even raw SQL
//! against the file cannot rewrite history without first dropping the triggers; and doing that is
//! itself caught by [`Ledger::verify`], as is any change to row content, order or length.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::{DecisionKind, Record, RecordKind, Writer};
use crate::project::ledger_path;
use crate::timefmt::now_rfc3339;

/// The `prev_hash` of the first record.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub(crate) const SQL_TABLE: &str = "CREATE TABLE records (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL,
    writer TEXT NOT NULL,
    session_id TEXT,
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    prev_hash TEXT NOT NULL,
    hash TEXT NOT NULL
)";
pub(crate) const SQL_NO_UPDATE: &str = "CREATE TRIGGER records_no_update BEFORE UPDATE ON records
BEGIN SELECT RAISE(ABORT, 'verdict ledger is append-only: rows cannot be updated'); END";
pub(crate) const SQL_NO_DELETE: &str = "CREATE TRIGGER records_no_delete BEFORE DELETE ON records
BEGIN SELECT RAISE(ABORT, 'verdict ledger is append-only: rows cannot be deleted'); END";
pub(crate) const SQL_CHAIN: &str = "CREATE TRIGGER records_chain BEFORE INSERT ON records
WHEN NEW.seq IS NOT (COALESCE((SELECT MAX(seq) FROM records), 0) + 1)
  OR NEW.prev_hash IS NOT COALESCE((SELECT hash FROM records ORDER BY seq DESC LIMIT 1),
     '0000000000000000000000000000000000000000000000000000000000000000')
BEGIN SELECT RAISE(ABORT, 'verdict ledger: record does not extend the chain'); END";

/// Errors from the ledger.
#[derive(Debug)]
pub enum LedgerError {
    /// SQLite failed.
    Sqlite(rusqlite::Error),
    /// A file operation failed.
    Io(std::io::Error),
    /// JSON (de)serialization failed.
    Json(serde_json::Error),
    /// The write was refused by the who-may-write-what rules. The refusal itself is in the
    /// ledger as record `refused_seq`.
    Refused {
        /// Sequence number of the `refused_write` record.
        refused_seq: u64,
        /// Why.
        reason: String,
    },
    /// The request was malformed (for example a correction of a record that does not exist).
    Invalid(String),
    /// A stored row could not be read back; run verify.
    Corrupt(String),
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LedgerError::Sqlite(e) => write!(f, "ledger database error: {e}"),
            LedgerError::Io(e) => write!(f, "ledger file error: {e}"),
            LedgerError::Json(e) => write!(f, "ledger JSON error: {e}"),
            LedgerError::Refused {
                refused_seq,
                reason,
            } => write!(
                f,
                "write refused (recorded as record {refused_seq}): {reason}"
            ),
            LedgerError::Invalid(m) => write!(f, "invalid ledger request: {m}"),
            LedgerError::Corrupt(m) => write!(f, "ledger row unreadable: {m}"),
        }
    }
}

impl std::error::Error for LedgerError {}
impl From<rusqlite::Error> for LedgerError {
    fn from(e: rusqlite::Error) -> Self {
        LedgerError::Sqlite(e)
    }
}
impl From<std::io::Error> for LedgerError {
    fn from(e: std::io::Error) -> Self {
        LedgerError::Io(e)
    }
}
impl From<serde_json::Error> for LedgerError {
    fn from(e: serde_json::Error) -> Self {
        LedgerError::Json(e)
    }
}

/// What [`Ledger::append`] stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Appended {
    /// Sequence number of the new record.
    pub seq: u64,
    /// Its hash.
    pub hash: String,
    /// The kind actually stored (a finding without evidence is stored as `hypothesis`).
    pub kind: RecordKind,
    /// True when a finding was stored as a hypothesis because it had no evidence pointer.
    pub stored_as_hypothesis: bool,
}

/// A record as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// Monotonic sequence number, starting at 1.
    pub seq: u64,
    /// Time written, RFC 3339 UTC.
    pub ts: String,
    /// Who wrote it.
    pub writer: Writer,
    /// Session it belongs to, if any.
    pub session_id: Option<String>,
    /// Its kind.
    pub kind: RecordKind,
    /// The typed record.
    pub record: Record,
    /// Hash of the previous record (all zeros for the first).
    pub prev_hash: String,
    /// This record's hash.
    pub hash: String,
}

/// A raw row, as it sits in the database (used by verify and export).
#[derive(Debug, Clone)]
pub(crate) struct RawRow {
    pub seq: i64,
    pub ts: String,
    pub writer: String,
    pub session_id: Option<String>,
    pub kind: String,
    pub body: String,
    pub prev_hash: String,
    pub hash: String,
}

/// The hash of a record: blake3 over its sequence number, time, writer, session, kind, the
/// previous hash and the stored body text. Fields are length-prefixed so none can bleed into another.
pub fn record_hash(
    seq: u64,
    ts: &str,
    writer: &str,
    session_id: Option<&str>,
    kind: &str,
    prev_hash: &str,
    body: &str,
) -> String {
    let mut h = blake3::Hasher::new();
    h.update(b"verdict-ledger-v1");
    h.update(&seq.to_le_bytes());
    for field in [ts, writer, session_id.unwrap_or(""), kind, prev_hash, body] {
        h.update(&(field.len() as u64).to_le_bytes());
        h.update(field.as_bytes());
    }
    h.update(&[session_id.is_some() as u8]);
    h.finalize().to_hex().to_string()
}

/// The evidence ledger. See the [module docs](self).
pub struct Ledger {
    conn: Connection,
    path: Option<PathBuf>,
    clock: Box<dyn Fn() -> String + Send>,
}

impl fmt::Debug for Ledger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger").field("path", &self.path).finish()
    }
}

impl Ledger {
    /// Open (creating if needed) the ledger of the project at `root`:
    /// `<root>/.verdict/ledger.db`.
    pub fn open(root: &Path) -> Result<Ledger, LedgerError> {
        let path = ledger_path(root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            // The design keeps `.verdict/` out of git. A `.gitignore` of `*` inside the folder
            // does that without touching the project's own `.gitignore`. A file already there
            // is the human's and is kept as it is. A failed write does not stop the ledger.
            let ignore = dir.join(".gitignore");
            if !ignore.exists() {
                let _ = std::fs::write(&ignore, "*\n");
            }
        }
        Self::open_path(&path)
    }

    /// Open the ledger of the project at `root` only if it already exists. Never creates files.
    pub fn open_existing(root: &Path) -> Result<Option<Ledger>, LedgerError> {
        let path = ledger_path(root);
        if !path.is_file() {
            return Ok(None);
        }
        Self::open_path(&path).map(Some)
    }

    /// Open a ledger at an explicit database path.
    pub fn open_path(path: &Path) -> Result<Ledger, LedgerError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        Self::init(conn, Some(path.to_path_buf()))
    }

    /// An in-memory ledger, for tests. Same schema, same rules, no file.
    pub fn open_in_memory() -> Result<Ledger, LedgerError> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<Ledger, LedgerError> {
        // Only a brand-new database gets a schema. An existing one is never "repaired" here,
        // so a dropped trigger stays visible to verify.
        let has_table: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='records')",
            [],
            |r| r.get(0),
        )?;
        if !has_table {
            let tx = conn.unchecked_transaction()?;
            for sql in [SQL_TABLE, SQL_NO_UPDATE, SQL_NO_DELETE, SQL_CHAIN] {
                tx.execute(sql, [])?;
            }
            tx.execute_batch("PRAGMA user_version = 1")?;
            tx.commit()?;
        }
        Ok(Ledger {
            conn,
            path,
            clock: Box::new(now_rfc3339),
        })
    }

    /// Replace the clock (tests and golden files use a fixed one).
    pub fn set_clock(&mut self, clock: impl Fn() -> String + Send + 'static) {
        self.clock = Box::new(clock);
    }

    /// Path of the database file, if it has one.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// The `ledger.head` sidecar next to the database: the last `seq hash` written. It catches a
    /// truncated tail even if the database's own bookkeeping was edited too.
    pub(crate) fn head_file(&self) -> Option<PathBuf> {
        self.path.as_ref().map(|p| p.with_file_name("ledger.head"))
    }

    /// Append a record on behalf of `writer`.
    ///
    /// * A write the writer is not allowed to make is **refused and recorded** as a
    ///   `refused_write`; the error is [`LedgerError::Refused`].
    /// * A `Finding` with no evidence pointer is stored as a `Hypothesis`
    ///   ([`Appended::stored_as_hypothesis`]).
    pub fn append(
        &mut self,
        writer: Writer,
        session_id: Option<&str>,
        record: Record,
    ) -> Result<Appended, LedgerError> {
        let head_file = self.head_file();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Only move the head file forward if it still agrees with the database. If it names a
        // record that is gone or different, the ledger was cut or rewritten; leaving the file as
        // it is keeps that evidence for verify instead of overwriting it.
        let head_trusted = match &head_file {
            Some(file) => head_file_agrees(&tx, file)?,
            None => false,
        };
        let decision = permission(&tx, writer, &record)?;
        let (outcome, head) = match decision {
            Err(reason) => {
                let attempted = serde_json::to_value(&record)?;
                let refusal = Record::RefusedWrite {
                    attempted_by: writer,
                    attempted_kind: record.kind(),
                    attempted_decision: match &record {
                        Record::Decision { decision, .. } => Some(*decision),
                        _ => None,
                    },
                    reason: reason.clone(),
                    attempted,
                };
                let a = insert(&tx, &self.clock, Writer::Core, session_id, &refusal)?;
                (
                    Err(LedgerError::Refused {
                        refused_seq: a.seq,
                        reason,
                    }),
                    (a.seq, a.hash),
                )
            }
            Ok(()) => {
                let converted = matches!(&record, Record::Finding(f) if f.evidence.is_empty());
                let record = normalize(record);
                let mut a = insert(&tx, &self.clock, writer, session_id, &record)?;
                a.stored_as_hypothesis = converted;
                let head = (a.seq, a.hash.clone());
                (Ok(a), head)
            }
        };
        tx.commit()?;
        if head_trusted && let Some(file) = head_file {
            // Best effort: a head file that lags is harmless, a missing one is just no extra check.
            write_head_file(&file, head.0, &head.1);
        }
        outcome
    }

    /// Number of records.
    pub fn count(&self) -> Result<u64, LedgerError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get::<_, i64>(0))?
            as u64)
    }

    /// The last record's sequence number and hash.
    pub fn head(&self) -> Result<Option<(u64, String)>, LedgerError> {
        Ok(self
            .conn
            .query_row(
                "SELECT seq, hash FROM records ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?)),
            )
            .optional()?)
    }

    /// One record by sequence number.
    pub fn get(&self, seq: u64) -> Result<Option<Stored>, LedgerError> {
        let row = self
            .conn
            .query_row(
                "SELECT seq, ts, writer, session_id, kind, body, prev_hash, hash
                 FROM records WHERE seq = ?1",
                params![seq as i64],
                raw_row,
            )
            .optional()?;
        row.map(stored_from_raw).transpose()
    }

    /// All records in order.
    pub fn records(&self) -> Result<Vec<Stored>, LedgerError> {
        self.raw_rows()?.into_iter().map(stored_from_raw).collect()
    }

    /// All records of one session, in order.
    pub fn records_for_session(&self, session_id: &str) -> Result<Vec<Stored>, LedgerError> {
        Ok(self
            .records()?
            .into_iter()
            .filter(|s| s.session_id.as_deref() == Some(session_id))
            .collect())
    }

    /// Corrections that reference record `seq`, in order.
    pub fn corrections_for(&self, seq: u64) -> Result<Vec<Stored>, LedgerError> {
        Ok(self
            .records()?
            .into_iter()
            .filter(|s| matches!(&s.record, Record::Correction { target_seq, .. } if *target_seq == seq))
            .collect())
    }

    pub(crate) fn raw_rows(&self) -> Result<Vec<RawRow>, LedgerError> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, ts, writer, session_id, kind, body, prev_hash, hash
             FROM records ORDER BY seq",
        )?;
        let rows = stmt.query_map([], raw_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Write every record as one JSON object per line (JSONL), in order. Each line holds
    /// `seq, ts, writer, session_id, kind, body, prev_hash, hash`; `body` is the stored text
    /// verbatim, so the hash chain can be re-checked from the export alone.
    pub fn export_jsonl(&self, out: &mut dyn Write) -> Result<(), LedgerError> {
        #[derive(serde::Serialize)]
        struct Line<'a> {
            seq: i64,
            ts: &'a str,
            writer: &'a str,
            session_id: &'a Option<String>,
            kind: &'a str,
            body: Box<serde_json::value::RawValue>,
            prev_hash: &'a str,
            hash: &'a str,
        }
        for r in self.raw_rows()? {
            let body = serde_json::value::RawValue::from_string(r.body.clone()).or_else(|_| {
                serde_json::value::RawValue::from_string(serde_json::to_string(&r.body)?)
            })?;
            let line = Line {
                seq: r.seq,
                ts: &r.ts,
                writer: &r.writer,
                session_id: &r.session_id,
                kind: &r.kind,
                body,
                prev_hash: &r.prev_hash,
                hash: &r.hash,
            };
            serde_json::to_writer(&mut *out, &line)?;
            out.write_all(b"\n")?;
        }
        Ok(())
    }
}

/// Parse the `ledger.head` text: `seq hash`.
pub(crate) fn parse_head(text: &str) -> Option<(i64, String)> {
    let mut parts = text.split_whitespace();
    let seq = parts.next()?.parse::<i64>().ok()?;
    let hash = parts.next()?.to_string();
    Some((seq, hash))
}

/// True when the head file is absent (nothing to protect yet) or names a record that exists with
/// the same hash. False when it is unreadable or names a missing or different record.
fn head_file_agrees(tx: &rusqlite::Transaction<'_>, file: &Path) -> Result<bool, LedgerError> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(_) => return Ok(false),
    };
    let Some((seq, hash)) = parse_head(&text) else {
        return Ok(false);
    };
    let stored: Option<String> = tx
        .query_row(
            "SELECT hash FROM records WHERE seq = ?1",
            params![seq],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored.as_deref() == Some(hash.as_str()))
}

/// Write the head file atomically (temp file, then rename), and never move it backward: a
/// concurrent writer may already have recorded a later head.
fn write_head_file(file: &Path, seq: u64, hash: &str) {
    if let Ok(text) = std::fs::read_to_string(file)
        && let Some((current, _)) = parse_head(&text)
        && current >= seq as i64
    {
        return;
    }
    let tmp = file.with_file_name(format!("ledger.head.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, format!("{seq} {hash}\n")).is_ok()
        && std::fs::rename(&tmp, file).is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn raw_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        seq: r.get(0)?,
        ts: r.get(1)?,
        writer: r.get(2)?,
        session_id: r.get(3)?,
        kind: r.get(4)?,
        body: r.get(5)?,
        prev_hash: r.get(6)?,
        hash: r.get(7)?,
    })
}

fn stored_from_raw(r: RawRow) -> Result<Stored, LedgerError> {
    let record: Record = serde_json::from_str(&r.body)
        .map_err(|e| LedgerError::Corrupt(format!("record {}: {e}", r.seq)))?;
    let writer = Writer::parse(&r.writer)
        .ok_or_else(|| LedgerError::Corrupt(format!("record {}: writer {:?}", r.seq, r.writer)))?;
    Ok(Stored {
        seq: r.seq as u64,
        ts: r.ts,
        writer,
        session_id: r.session_id,
        kind: record.kind(),
        record,
        prev_hash: r.prev_hash,
        hash: r.hash,
    })
}

/// A finding with no evidence pointer is a hypothesis.
fn normalize(record: Record) -> Record {
    match record {
        Record::Finding(f) if f.evidence.is_empty() => Record::Hypothesis(f),
        Record::Correction {
            target_seq,
            reason,
            replacement,
        } => Record::Correction {
            target_seq,
            reason,
            replacement: replacement.map(|r| Box::new(normalize(*r))),
        },
        other => other,
    }
}

/// Decide whether `writer` may write `record`. `Ok(Err(reason))` is a refusal;
/// `Err(_)` is a real error (or an invalid request).
fn permission(
    tx: &rusqlite::Transaction<'_>,
    writer: Writer,
    record: &Record,
) -> Result<Result<(), String>, LedgerError> {
    let kind = record.kind();
    if !writer.may_write(kind) {
        let reason = match record {
            Record::Decision {
                decision: DecisionKind::CommitAnyway,
                ..
            } => "commit anyway is human-only (GUI or vdt), never an agent, MCP call or hook"
                .to_string(),
            Record::Decision {
                decision: DecisionKind::ModeSwitch,
                ..
            } => "only the human can switch between shadow and enforce".to_string(),
            _ => format!("{writer} may not write a {kind} record"),
        };
        return Ok(Err(reason));
    }
    match record {
        Record::Decision {
            who,
            decision,
            reason,
            ..
        } => {
            if *who != Writer::Human {
                return Err(LedgerError::Invalid(
                    "a decision's `who` is always the human".into(),
                ));
            }
            if *decision == DecisionKind::CommitAnyway
                && reason.as_deref().is_none_or(|r| r.trim().is_empty())
            {
                return Err(LedgerError::Invalid(
                    "commit anyway needs a reason the human typed".into(),
                ));
            }
        }
        Record::Correction {
            target_seq,
            replacement,
            ..
        } => {
            let target_writer: Option<String> = tx
                .query_row(
                    "SELECT writer FROM records WHERE seq = ?1",
                    params![*target_seq as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(target_writer) = target_writer else {
                return Err(LedgerError::Invalid(format!(
                    "correction targets record {target_seq}, which does not exist"
                )));
            };
            if !matches!(writer, Writer::Core | Writer::Human)
                && Writer::parse(&target_writer) != Some(writer)
            {
                return Ok(Err(format!(
                    "{writer} may only correct its own records (record {target_seq} was written by {target_writer})"
                )));
            }
            if let Some(rep) = replacement {
                if matches!(**rep, Record::Correction { .. }) {
                    return Err(LedgerError::Invalid(
                        "a correction's replacement cannot itself be a correction".into(),
                    ));
                }
                if !writer.may_write(rep.kind()) {
                    return Ok(Err(format!(
                        "{writer} may not write a {} record, so cannot correct into one",
                        rep.kind()
                    )));
                }
                if let Record::Decision { who, .. } = &**rep
                    && *who != Writer::Human
                {
                    return Err(LedgerError::Invalid(
                        "a decision's `who` is always the human".into(),
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(Ok(()))
}

fn insert(
    tx: &rusqlite::Transaction<'_>,
    clock: &dyn Fn() -> String,
    writer: Writer,
    session_id: Option<&str>,
    record: &Record,
) -> Result<Appended, LedgerError> {
    let last: Option<(i64, String)> = tx
        .query_row(
            "SELECT seq, hash FROM records ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (seq, prev) = match last {
        Some((s, h)) => (s as u64 + 1, h),
        None => (1, GENESIS_HASH.to_string()),
    };
    let ts = clock();
    let kind = record.kind();
    let body = serde_json::to_string(record)?;
    let hash = record_hash(
        seq,
        &ts,
        writer.as_str(),
        session_id,
        kind.as_str(),
        &prev,
        &body,
    );
    tx.execute(
        "INSERT INTO records (seq, ts, writer, session_id, kind, body, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            seq as i64,
            ts,
            writer.as_str(),
            session_id,
            kind.as_str(),
            body,
            prev,
            hash
        ],
    )?;
    Ok(Appended {
        seq,
        hash,
        kind,
        stored_as_hypothesis: kind == RecordKind::Hypothesis,
    })
}
