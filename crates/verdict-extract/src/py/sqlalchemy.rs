//! SQLAlchemy writes: `session.add/add_all/delete/merge/bulk_*`, the `insert(T)`, `update(T)` and
//! `delete(T)` statements (core and ORM, including the PostgreSQL upsert form), `Query.update()`
//! and `Query.delete()`, `table.insert()` on a `Table`, and raw SQL given to `text(...)`,
//! `exec_driver_sql` or a session's `execute`.
//!
//! A call such as `x.add(...)` is a write only when `x` is a session: a name that is bound to a
//! session in this file (assigned or opened from a `Session`, `SessionLocal`, `sessionmaker`,
//! `get_session`, a parameter annotated `Session`) or a conventional session name in a file that
//! shows SQLAlchemy at work. A set's `add` and an HTTP client's `delete` are not writes.

use std::collections::{BTreeMap, BTreeSet};

use super::common::{
    Bindings, assigned_calls, call_args, call_chain, chain, enclosing, enclosing_fn, eval_str,
    is_camel, last_assignment, module_consts, param_annotation, scope_of, unwrap_expr,
};
use super::sql::{has_dollar_param, write_of};
use crate::model::*;
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// SQLAlchemy writes.
pub struct SqlAlchemy;

const EVIDENCE: &[&str] = &[
    "sqlalchemy",
    "sqlmodel",
    "SessionLocal",
    "AsyncSession",
    "sessionmaker",
    "get_db",
    "get_session",
    "db_session",
    "flask_sqlalchemy",
];

const SESSION_NAMES: &[&str] = &[
    "session",
    "sess",
    "db",
    "dbsession",
    "asyncsession",
    "sqlsession",
    "ormsession",
    "sessionlocal",
];

/// Which names in a file are database sessions.
pub(super) struct Sessions {
    bound: BTreeSet<String>,
    evidence: bool,
}

fn norm(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '_')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn is_session_factory(c: &[String]) -> bool {
    c.last()
        .is_some_and(|l| l.to_ascii_lowercase().contains("session"))
}

impl Sessions {
    pub(super) fn new(root: Node, src: &str, file_text: &str) -> Sessions {
        let mut bound = BTreeSet::new();
        for (name, ctor, _) in assigned_calls(root, src) {
            if is_session_factory(&ctor) {
                bound.insert(name);
            }
        }
        for_each_node(root, |n| match n.kind() {
            "as_pattern" => {
                let (Some(v), Some(alias)) = (n.named_child(0), n.child_by_field_name("alias"))
                else {
                    return;
                };
                let v = unwrap_expr(v);
                if v.kind() == "call"
                    && let Some(c) = call_chain(v, src)
                    && is_session_factory(&c)
                {
                    bound.insert(text(alias, src).trim().to_string());
                }
            }
            "typed_parameter" | "typed_default_parameter" => {
                let (name, ty) = if n.kind() == "typed_parameter" {
                    (n.named_child(0), n.child_by_field_name("type"))
                } else {
                    (n.child_by_field_name("name"), n.child_by_field_name("type"))
                };
                if let (Some(name), Some(ty)) = (name, ty)
                    && name.kind() == "identifier"
                    && text(ty, src).contains("Session")
                {
                    bound.insert(text(name, src).to_string());
                }
            }
            _ => {}
        });
        Sessions {
            bound,
            evidence: EVIDENCE.iter().any(|e| file_text.contains(e)),
        }
    }

    /// True when `recv` (the object of `recv.add(...)`) is a session.
    pub(super) fn is_session(&self, recv: Node, src: &str) -> bool {
        let r = unwrap_expr(recv);
        if r.kind() == "call" {
            return call_chain(r, src).is_some_and(|c| is_session_factory(&c));
        }
        let Some(c) = chain(r, src) else { return false };
        let last = c.last().map(String::as_str).unwrap_or("");
        if c.len() == 1 && self.bound.contains(last) {
            return true;
        }
        let n = norm(last);
        self.evidence && (SESSION_NAMES.contains(&n.as_str()) || n.ends_with("session"))
    }
}

/// The SQL text an argument holds, when it can be read: a literal, an f-string whose unreadable
/// parts are written `${}`, a module constant, or a local variable assigned one of those.
pub(super) fn sql_of(
    arg: Node,
    root: Node,
    src: &str,
    consts: &BTreeMap<String, String>,
    depth: usize,
) -> Option<String> {
    let a = unwrap_expr(arg);
    match a.kind() {
        "string" | "concatenated_string" | "binary_operator" => {
            eval_str(a, src, consts, Some("${}"))
        }
        "identifier" => {
            if depth > 3 {
                return None;
            }
            let name = text(a, src);
            let scope = scope_of(a, root);
            if let Some(rhs) = last_assignment(scope, name, a.start_byte(), src) {
                return sql_of(rhs, root, src, consts, depth + 1);
            }
            consts.get(name).cloned()
        }
        _ => None,
    }
}

fn table_of_expr(n: Node, src: &str, consts: &BTreeMap<String, String>) -> Option<String> {
    let n = unwrap_expr(n);
    match n.kind() {
        "identifier" => Some(text(n, src).to_string()),
        "attribute" => {
            let c = chain(n, src)?;
            if c.last().map(String::as_str) == Some("__table__") && c.len() >= 2 {
                Some(c[c.len() - 2].clone())
            } else {
                c.last().cloned()
            }
        }
        "call" => {
            let c = call_chain(n, src)?;
            if matches!(c.last().map(String::as_str), Some("table" | "Table")) {
                let a = call_args(n, src);
                return eval_str(*a.pos.first()?, src, consts, None);
            }
            None
        }
        _ => None,
    }
}

/// The model a `session.add(x)` argument is an instance of, when it can be read.
struct ModelCtx<'a, 't> {
    root: Node<'t>,
    src: &'a str,
}

impl<'t> ModelCtx<'_, 't> {
    fn model_of(&self, expr: Node<'t>, depth: usize) -> Option<String> {
        let e = unwrap_expr(expr);
        match e.kind() {
            "call" => {
                if let Some(c) = call_chain(e, self.src)
                    && let Some(last) = c.last()
                    && is_camel(last)
                {
                    return Some(last.clone());
                }
                self.query_model(e)
            }
            "identifier" => {
                if depth > 3 {
                    return None;
                }
                let name = text(e, self.src);
                if let Some(func) = enclosing_fn(e)
                    && let Some(ann) = param_annotation(func, name, self.src)
                    && ann
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
                    && let Some(last) = ann.rsplit('.').next()
                    && is_camel(last)
                {
                    return Some(last.to_string());
                }
                let scope = scope_of(e, self.root);
                let rhs = last_assignment(scope, name, e.start_byte(), self.src)?;
                self.model_of(rhs, depth + 1)
            }
            _ => None,
        }
    }

    /// `session.get(User, 1)`, `session.query(User)...`, `session.scalars(select(User))...`.
    fn query_model(&self, expr: Node<'t>) -> Option<String> {
        let mut cur = unwrap_expr(expr);
        for _ in 0..16 {
            match cur.kind() {
                "call" => {
                    let f = cur.child_by_field_name("function")?;
                    let last = match f.kind() {
                        "identifier" => text(f, self.src),
                        "attribute" => text(f.child_by_field_name("attribute")?, self.src),
                        _ => return None,
                    };
                    let args = call_args(cur, self.src);
                    match last {
                        "get" | "query" | "select" | "get_one" => {
                            if let Some(a) = args.pos.first()
                                && let Some(c) = chain(*a, self.src)
                                && let Some(l) = c.last()
                                && is_camel(l)
                            {
                                return Some(l.clone());
                            }
                        }
                        "scalars" | "scalar" | "execute" | "exec" => {
                            if let Some(a) = args.pos.first()
                                && let Some(m) = self.query_model(*a)
                            {
                                return Some(m);
                            }
                        }
                        _ => {}
                    }
                    if f.kind() == "attribute" {
                        cur = unwrap_expr(f.child_by_field_name("object")?);
                    } else {
                        return None;
                    }
                }
                "attribute" => cur = unwrap_expr(cur.child_by_field_name("object")?),
                _ => return None,
            }
        }
        None
    }
}

/// `Some(table)` when the receiver chain contains `session.query(T)` or `T.query`.
fn query_chain_table(
    obj: Node,
    src: &str,
    consts: &BTreeMap<String, String>,
) -> Option<Option<String>> {
    let mut cur = unwrap_expr(obj);
    for _ in 0..24 {
        match cur.kind() {
            "call" => {
                let f = cur.child_by_field_name("function")?;
                if f.kind() != "attribute" {
                    return None;
                }
                if text(f.child_by_field_name("attribute")?, src) == "query" {
                    let a = call_args(cur, src);
                    return Some(a.pos.first().and_then(|n| table_of_expr(*n, src, consts)));
                }
                cur = unwrap_expr(f.child_by_field_name("object")?);
            }
            "attribute" => {
                let attr = text(cur.child_by_field_name("attribute")?, src);
                let o = unwrap_expr(cur.child_by_field_name("object")?);
                if attr == "query" && o.kind() == "identifier" {
                    return Some(Some(text(o, src).to_string()));
                }
                cur = o;
            }
            _ => return None,
        }
    }
    None
}

impl Extractor for SqlAlchemy {
    fn name(&self) -> &'static str {
        "sqlalchemy"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        let s = src.text;
        let root = src.tree.root_node();
        let b = Bindings::parse(root, s);
        let consts = module_consts(root, s);
        let sessions = Sessions::new(root, s, s);
        let models = ModelCtx { root, src: s };

        let mut table_vars: BTreeMap<String, String> = BTreeMap::new();
        for (name, ctor, call) in assigned_calls(root, s) {
            if matches!(ctor.last().map(String::as_str), Some("Table" | "table")) {
                let a = call_args(call, s);
                let t = a
                    .pos
                    .first()
                    .and_then(|n| eval_str(*n, s, &consts, None))
                    .unwrap_or_else(|| name.clone());
                table_vars.entry(name).or_insert(t);
            }
        }

        let sa_fn = |call: Node| -> Option<&'static str> {
            let c = call_chain(call, s)?;
            let original: String = if c.len() == 1 {
                b.imported_name(&c[0], "sqlalchemy")
                    .or_else(|| b.imported_name(&c[0], "sqlmodel"))?
            } else if b.head_from(&c, "sqlalchemy") || b.head_from(&c, "sqlmodel") {
                c.last()?.clone()
            } else {
                return None;
            };
            ["insert", "update", "delete", "text"]
                .into_iter()
                .find(|k| *k == original)
        };

        let mut writes: Vec<DataWrite> = Vec::new();
        let mut push = |op: WriteOp, table: Option<String>, n: Node| {
            writes.push(DataWrite {
                op,
                table,
                enclosing: enclosing(n, s),
                loc: out.loc(line(n)),
                framework: Framework::SqlAlchemy,
            });
        };

        for_each_node(root, |n| {
            if n.kind() != "call" {
                return;
            }
            let args = call_args(n, s);
            // Statement constructs and raw text.
            if let Some(kind) = sa_fn(n) {
                match kind {
                    "text" => {
                        if let Some(a) = args.pos.first()
                            && let Some(w) =
                                sql_of(*a, root, s, &consts, 0).and_then(|q| write_of(&q))
                        {
                            push(w.op, w.table, n);
                        }
                    }
                    k => {
                        let table = args.pos.first().and_then(|a| table_of_expr(*a, s, &consts));
                        let mut op = match k {
                            "insert" => WriteOp::Insert,
                            "update" => WriteOp::Update,
                            _ => WriteOp::Delete,
                        };
                        if op == WriteOp::Insert && has_upsert_method(n, s) {
                            op = WriteOp::Upsert;
                        }
                        push(op, table, n);
                    }
                }
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
            let is_sess = sessions.is_session(obj, s);
            match meth {
                "add" | "add_all" | "delete" | "merge" | "bulk_save_objects" if is_sess => {
                    let Some(arg) = args.pos.first().copied() else {
                        return;
                    };
                    // A string argument is an HTTP client's `session.delete(url)`, not a write.
                    if matches!(unwrap_expr(arg).kind(), "string" | "concatenated_string") {
                        return;
                    }
                    let op = match meth {
                        "delete" => WriteOp::Delete,
                        "merge" => WriteOp::Upsert,
                        _ => WriteOp::Insert,
                    };
                    let list = matches!(meth, "add_all" | "bulk_save_objects");
                    let mut found: Vec<Option<String>> = Vec::new();
                    if list {
                        let a = unwrap_expr(arg);
                        match a.kind() {
                            "list" | "tuple" | "set" => {
                                let mut cur = a.walk();
                                for e in a.named_children(&mut cur) {
                                    let m = models.model_of(e, 0);
                                    if !found.contains(&m) {
                                        found.push(m);
                                    }
                                }
                            }
                            "list_comprehension" | "generator_expression" => {
                                if let Some(body) = a.child_by_field_name("body") {
                                    found.push(models.model_of(body, 0));
                                }
                            }
                            _ => found.push(models.model_of(a, 0)),
                        }
                    } else {
                        found.push(models.model_of(arg, 0));
                    }
                    if found.is_empty() {
                        found.push(None);
                    }
                    for m in found {
                        push(op, m, n);
                    }
                }
                "bulk_insert_mappings" | "bulk_update_mappings" if is_sess => {
                    let t = args.pos.first().and_then(|a| table_of_expr(*a, s, &consts));
                    let op = if meth == "bulk_insert_mappings" {
                        WriteOp::Insert
                    } else {
                        WriteOp::Update
                    };
                    push(op, t, n);
                }
                "exec_driver_sql" | "execute" => {
                    let Some(a) = args.pos.first().copied() else {
                        return;
                    };
                    if meth == "execute" && (!is_sess || unwrap_expr(a).kind() == "call") {
                        return;
                    }
                    let Some(q) = sql_of(a, root, s, &consts, 0) else {
                        return;
                    };
                    if has_dollar_param(&q) && meth == "execute" {
                        return; // asyncpg's placeholders; that extractor reads it
                    }
                    if let Some(w) = write_of(&q) {
                        push(w.op, w.table, n);
                    }
                }
                "update" | "delete" => {
                    if let Some(t) = query_chain_table(obj, s, &consts) {
                        let op = if meth == "update" {
                            WriteOp::Update
                        } else {
                            WriteOp::Delete
                        };
                        push(op, t, n);
                    } else if args.pos.is_empty()
                        && args.kw.is_empty()
                        && let Some(t) = core_table(obj, s, &table_vars)
                    {
                        let op = if meth == "update" {
                            WriteOp::Update
                        } else {
                            WriteOp::Delete
                        };
                        push(op, Some(t), n);
                    }
                }
                "insert" if args.pos.is_empty() && args.kw.is_empty() => {
                    if let Some(t) = core_table(obj, s, &table_vars) {
                        push(WriteOp::Insert, Some(t), n);
                    }
                }
                _ => {}
            }
        });

        if !writes.is_empty() {
            out.frameworks.insert(Framework::SqlAlchemy);
        }
        out.writes.extend(writes);
    }
}

/// The table behind `X.__table__` or a variable assigned from `Table(...)`.
fn core_table(obj: Node, src: &str, table_vars: &BTreeMap<String, String>) -> Option<String> {
    let c = chain(unwrap_expr(obj), src)?;
    if c.last().map(String::as_str) == Some("__table__") && c.len() >= 2 {
        return Some(c[c.len() - 2].clone());
    }
    if c.len() == 1 {
        return table_vars.get(&c[0]).cloned();
    }
    None
}

/// True when the statement is continued with `.on_conflict_do_update(...)` and the like.
fn has_upsert_method(call: Node, src: &str) -> bool {
    let mut cur = call;
    loop {
        let Some(p) = cur.parent() else { return false };
        if p.kind() != "attribute"
            || p.child_by_field_name("object").map(|o| o.id()) != Some(cur.id())
        {
            return false;
        }
        let Some(pp) = p.parent().filter(|pp| {
            pp.kind() == "call"
                && pp.child_by_field_name("function").map(|f| f.id()) == Some(p.id())
        }) else {
            return false;
        };
        let m = p
            .child_by_field_name("attribute")
            .map(|a| text(a, src))
            .unwrap_or("");
        if m.starts_with("on_conflict") || m == "on_duplicate_key_update" {
            return true;
        }
        cur = pp;
    }
}
