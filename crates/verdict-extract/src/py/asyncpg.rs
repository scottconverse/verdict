//! asyncpg writes: `execute`, `executemany` and `fetch*` calls whose SQL is an INSERT, UPDATE,
//! DELETE or upsert-style statement, and `copy_records_to_table` / `copy_to_table`.
//!
//! A bare `cursor.execute("INSERT ...")` is also a driver call, but it could be sqlite3 or
//! psycopg; this reader takes it only with a sign of asyncpg: the file mentions asyncpg, the SQL
//! uses `$1` placeholders, the method is one only asyncpg has, or the receiver is a pool. SQL
//! passed through `text(...)` or to a SQLAlchemy session belongs to the SQLAlchemy reader.

use super::common::{call_args, chain, enclosing, eval_str, module_consts, unwrap_expr};
use super::sql::{has_dollar_param, write_of};
use super::sqlalchemy::{Sessions, sql_of};
use crate::model::*;
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};

/// asyncpg writes.
pub struct Asyncpg;

impl Extractor for Asyncpg {
    fn name(&self) -> &'static str {
        "asyncpg"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        let s = src.text;
        if !["execute", "fetch", "copy_records_to_table", "copy_to_table"]
            .iter()
            .any(|m| s.contains(m))
        {
            return;
        }
        let root = src.tree.root_node();
        let consts = module_consts(root, s);
        let sessions = Sessions::new(root, s, s);
        let mentions = s.contains("asyncpg");

        let mut writes: Vec<DataWrite> = Vec::new();
        for_each_node(root, |n| {
            if n.kind() != "call" {
                return;
            }
            let Some(f) = n
                .child_by_field_name("function")
                .filter(|f| f.kind() == "attribute")
            else {
                return;
            };
            let (Some(obj), Some(meth)) = (
                f.child_by_field_name("object"),
                f.child_by_field_name("attribute"),
            ) else {
                return;
            };
            let meth = text(meth, s);
            let args = call_args(n, s);
            let recv_last = chain(unwrap_expr(obj), s)
                .and_then(|c| c.last().cloned())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let pool = recv_last.ends_with("pool");
            let mut push = |op: WriteOp, table: Option<String>| {
                writes.push(DataWrite {
                    op,
                    table,
                    enclosing: enclosing(n, s),
                    loc: out.loc(line(n)),
                    framework: Framework::Asyncpg,
                });
            };
            match meth {
                "copy_records_to_table" | "copy_to_table" => {
                    let t = args
                        .get(0, "table_name")
                        .and_then(|t| eval_str(t, s, &consts, None));
                    push(WriteOp::Insert, t);
                }
                "execute" | "executemany" | "fetch" | "fetchrow" | "fetchval" | "fetchmany" => {
                    let Some(a) = args.get(0, "query") else {
                        return;
                    };
                    if unwrap_expr(a).kind() == "call" {
                        return; // text(...) and other builders
                    }
                    let Some(q) = sql_of(a, root, s, &consts, 0) else {
                        return;
                    };
                    let dollar = has_dollar_param(&q);
                    if !dollar && sessions.is_session(obj, s) {
                        return; // SQLAlchemy's `session.execute("...")`
                    }
                    let specific = matches!(meth, "fetchrow" | "fetchval" | "fetchmany");
                    if !(mentions || dollar || specific || pool) {
                        return;
                    }
                    if let Some(w) = write_of(&q) {
                        push(w.op, w.table);
                    }
                }
                _ => {}
            }
        });

        if !writes.is_empty() {
            out.frameworks.insert(Framework::Asyncpg);
        }
        out.writes.extend(writes);
    }
}
