//! Alembic: a file under a `versions/` folder that declares `revision = "..."` is a migration.
//! Its tables are the ones `op.create_table`, `op.add_column`, `op.drop_table`,
//! `op.alter_column`, `op.batch_alter_table` and the other `op.*` calls name, plus the tables in
//! SQL given to `op.execute`.

use super::common::{Args, call_args, call_chain, eval_str, module_consts};
use super::sql::tables_of;
use super::sqlalchemy::sql_of;
use crate::model::*;
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// Alembic migrations.
pub struct Alembic;

/// For each `op` function: the positions and keyword names of the arguments that name a table.
fn table_args(func: &str) -> &'static [(usize, &'static str)] {
    match func {
        "create_table"
        | "drop_table"
        | "batch_alter_table"
        | "create_table_comment"
        | "drop_table_comment" => &[(0, "table_name")],
        "add_column" | "drop_column" | "alter_column" => &[(0, "table_name")],
        "rename_table" => &[(0, "old_table_name"), (1, "new_table_name")],
        "create_index"
        | "create_unique_constraint"
        | "create_primary_key"
        | "create_check_constraint"
        | "drop_constraint"
        | "drop_index" => &[(1, "table_name")],
        "create_foreign_key" => &[(1, "source_table"), (2, "referent_table")],
        _ => &[],
    }
}

fn in_versions_folder(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    parts.contains(&"versions")
}

fn revision_of(root: Node, src: &str) -> Option<(Option<String>, u32)> {
    let mut cur = root.walk();
    for stmt in root.named_children(&mut cur) {
        if stmt.kind() != "expression_statement" {
            continue;
        }
        let Some(a) = stmt.named_child(0).filter(|a| a.kind() == "assignment") else {
            continue;
        };
        let (Some(l), Some(r)) = (
            a.child_by_field_name("left"),
            a.child_by_field_name("right"),
        ) else {
            continue;
        };
        if l.kind() == "identifier" && text(l, src) == "revision" {
            let v = eval_str(r, src, &Default::default(), None);
            return Some((v, line(a)));
        }
    }
    None
}

impl Extractor for Alembic {
    fn name(&self) -> &'static str {
        "alembic"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        if !in_versions_folder(src.path) {
            return;
        }
        let s = src.text;
        let root = src.tree.root_node();
        let Some((revision, rev_line)) = revision_of(root, s) else {
            return;
        };
        let consts = module_consts(root, s);
        let mut tables: Vec<String> = Vec::new();
        let mut add = |t: String| {
            if !t.is_empty() && !tables.contains(&t) {
                tables.push(t);
            }
        };
        for_each_node(root, |n| {
            if n.kind() != "call" {
                return;
            }
            let Some(c) = call_chain(n, s) else { return };
            if c.len() != 2 || c[0] != "op" {
                return;
            }
            let args: Args = call_args(n, s);
            let func = c[1].as_str();
            match func {
                "execute" => {
                    if let Some(a) = args.get(0, "sqltext")
                        && let Some(q) = sql_of_any(a, root, s, &consts)
                    {
                        for t in tables_of(&q) {
                            add(t);
                        }
                    }
                }
                "bulk_insert" => {
                    // `op.bulk_insert(table, rows)`: the table is usually `sa.table("name", ...)`
                    // assigned to a variable.
                    if let Some(a) = args.get(0, "table")
                        && let Some(t) = table_name_of(a, root, s, &consts)
                    {
                        add(t);
                    }
                }
                _ => {
                    for (idx, kw) in table_args(func) {
                        if let Some(a) = args.get(*idx, kw)
                            && let Some(t) = eval_str(a, s, &consts, None)
                        {
                            add(t);
                        }
                    }
                }
            }
        });
        out.frameworks.insert(Framework::Alembic);
        out.migrations.push(Migration {
            revision,
            tables,
            loc: out.loc(rev_line),
            framework: Framework::Alembic,
        });
    }
}

fn sql_of_any(
    a: Node,
    root: Node,
    src: &str,
    consts: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    // `op.execute(sa.text("..."))` as well as `op.execute("...")`.
    if a.kind() == "call"
        && call_chain(a, src).is_some_and(|c| c.last().map(String::as_str) == Some("text"))
    {
        let inner = call_args(a, src);
        return sql_of(*inner.pos.first()?, root, src, consts, 0);
    }
    sql_of(a, root, src, consts, 0)
}

fn table_name_of(
    a: Node,
    root: Node,
    src: &str,
    consts: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let table_call = |call: Node| -> Option<String> {
        let c = call_chain(call, src)?;
        if matches!(c.last().map(String::as_str), Some("table" | "Table")) {
            let args = call_args(call, src);
            return eval_str(*args.pos.first()?, src, consts, None);
        }
        None
    };
    if a.kind() == "call" {
        return table_call(a);
    }
    if a.kind() == "identifier" {
        let rhs = super::common::last_assignment(
            super::common::scope_of(a, root),
            text(a, src),
            a.start_byte(),
            src,
        )?;
        if rhs.kind() == "call" {
            return table_call(rhs);
        }
    }
    None
}
