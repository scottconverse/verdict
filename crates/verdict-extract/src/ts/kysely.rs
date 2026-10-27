//! Kysely writes: `insertInto`, `updateTable`, `deleteFrom`, `replaceInto` (and `mergeInto`).

use super::{TS_LANGS, args, callee, enclosing, str_lit};
use crate::model::{DataWrite, FileFacts, Framework, Lang, WriteOp};
use crate::syntax::{for_each_node, line};
use crate::{Extractor, Source};

/// Kysely query-builder writes.
pub struct Kysely;

impl Extractor for Kysely {
    fn name(&self) -> &'static str {
        "kysely"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        for_each_node(s.tree.root_node(), |n| {
            if n.kind() != "call_expression" {
                return;
            }
            let Some((Some(_), method)) = callee(n, src) else {
                return;
            };
            let op = match method {
                "insertInto" => WriteOp::Insert,
                "updateTable" => WriteOp::Update,
                "deleteFrom" => WriteOp::Delete,
                "replaceInto" | "mergeInto" => WriteOp::Upsert,
                _ => return,
            };
            // db.insertInto("notes"), db.updateTable("person as p"); a computed table is still
            // a write, with no table name.
            let table = args(n)
                .first()
                .and_then(|a| str_lit(*a, src))
                .filter(|(_, dynamic)| !dynamic)
                .map(|(t, _)| t.split_whitespace().next().unwrap_or_default().to_string());
            let loc = out.loc(line(n));
            out.writes.push(DataWrite {
                op,
                table,
                enclosing: enclosing(n, src),
                loc,
                framework: Framework::Kysely,
            });
            out.frameworks.insert(Framework::Kysely);
        });
    }
}
