//! SQLAlchemy and asyncpg write facts.

mod py_common;

use py_common::*;
use verdict_extract::{Facts, Framework, WriteOp};

/// `(op, table, enclosing, line)` of every write.
fn writes(f: &Facts) -> Vec<(WriteOp, Option<String>, Option<String>, u32)> {
    f.writes()
        .map(|w| (w.op, w.table.clone(), w.enclosing.clone(), w.loc.line))
        .collect()
}

fn t(s: &str) -> Option<String> {
    Some(s.to_string())
}

#[test]
fn session_writes_name_the_model() {
    let f = one(r#"from sqlalchemy.orm import Session
from app.models import User, Note

def create(db: Session, name: str):
    user = User(name=name)
    db.add(user)
    db.add(Note(text="x"))
    db.commit()

def remove(session: Session, uid: int):
    u = session.get(User, uid)
    session.delete(u)

def many(session: Session, rows):
    session.add_all([User(name="a"), Note(text="b"), User(name="c")])
    session.add_all([Note(**r) for r in rows])
    session.merge(User(id=1))

def typed(session: Session, user: User):
    session.add(user)
"#);
    assert_eq!(
        writes(&f),
        [
            (WriteOp::Insert, t("User"), t("create"), 6),
            (WriteOp::Insert, t("Note"), t("create"), 7),
            (WriteOp::Delete, t("User"), t("remove"), 12),
            (WriteOp::Insert, t("User"), t("many"), 15),
            (WriteOp::Insert, t("Note"), t("many"), 15),
            (WriteOp::Insert, t("Note"), t("many"), 16),
            (WriteOp::Upsert, t("User"), t("many"), 17),
            (WriteOp::Insert, t("User"), t("typed"), 20),
        ]
    );
    assert!(f.frameworks().contains(&Framework::SqlAlchemy));
}

#[test]
fn async_session_from_context_manager_and_methods() {
    let f = one(r#"from sqlalchemy.ext.asyncio import async_sessionmaker
from app.models import Item

class Repo:
    def __init__(self, session):
        self.session = session

    async def save(self, item):
        async with self.maker() as s:
            s.add(Item(**item))
            await s.commit()
        self.session.add(Item())
"#);
    // `s` was opened from something named `maker`, which is not a session factory; but the file
    // shows SQLAlchemy and `self.session` is a session by name.
    assert_eq!(
        writes(&f),
        [(WriteOp::Insert, t("Item"), t("Repo.save"), 12)]
    );
}

#[test]
fn statements_core_and_orm() {
    let f = one(r#"import sqlalchemy as sa
from sqlalchemy import insert, update, delete, text
from sqlalchemy.dialects.postgresql import insert as pg_insert
from app.models import User, Order, orders_table

def go(session):
    session.execute(insert(User).values(name="a"))
    session.execute(update(Order).where(Order.id == 1).values(x=1))
    session.execute(delete(orders_table))
    session.execute(pg_insert(User).values(id=1).on_conflict_do_update(index_elements=["id"], set_={}))
    session.execute(sa.insert(User.__table__), [{"a": 1}])
    session.execute(sa.delete(User))
"#);
    assert_eq!(
        writes(&f),
        [
            (WriteOp::Insert, t("User"), t("go"), 7),
            (WriteOp::Update, t("Order"), t("go"), 8),
            (WriteOp::Delete, t("orders_table"), t("go"), 9),
            (WriteOp::Upsert, t("User"), t("go"), 10),
            (WriteOp::Insert, t("User"), t("go"), 11),
            (WriteOp::Delete, t("User"), t("go"), 12),
        ]
    );
}

#[test]
fn raw_sql_through_text_and_driver() {
    let f = one(r#"from sqlalchemy import text

def a(session):
    session.execute(text("INSERT INTO audit (who) VALUES (:w)"), {"w": 1})
    session.execute(text("select * from audit"))

def b(conn, table):
    conn.exec_driver_sql("DELETE FROM audit")
    stmt = text(f"UPDATE {table} SET x = 1")
    conn.execute(stmt)

SQL = "TRUNCATE TABLE audit"

def c(session):
    session.execute(text(SQL))
    session.execute("UPDATE audit SET n = 1")
"#);
    assert_eq!(
        writes(&f),
        [
            (WriteOp::Insert, t("audit"), t("a"), 4),
            (WriteOp::Delete, t("audit"), t("b"), 8),
            (WriteOp::Update, None, t("b"), 9),
            (WriteOp::Delete, t("audit"), t("c"), 15),
            (WriteOp::Update, t("audit"), t("c"), 16),
        ]
    );
}

#[test]
fn query_update_delete_and_table_objects() {
    let f = one(r#"import sqlalchemy as sa
from app.models import User

users = sa.Table("users", meta)

def go(session):
    session.query(User).filter(User.id == 1).delete()
    User.query.filter_by(x=1).update({"x": 2})
    users.insert().values(a=1)
    User.__table__.delete()
    return cache.delete()
"#);
    assert_eq!(
        writes(&f),
        [
            (WriteOp::Delete, t("User"), t("go"), 7),
            (WriteOp::Update, t("User"), t("go"), 8),
            (WriteOp::Insert, t("users"), t("go"), 9),
            (WriteOp::Delete, t("User"), t("go"), 10),
        ]
    );
}

#[test]
fn things_that_only_look_like_writes() {
    let f = one(r#"import requests
import sqlalchemy

def a():
    seen = set()
    seen.add(1)
    cache.delete("k")
    session = requests.Session()
    session.delete("https://x/items/1")
    d = {}
    d.update(a=1)
    insert(5)
    update("x")
"#);
    assert!(writes(&f).is_empty(), "{:?}", writes(&f));
}

#[test]
fn sqlmodel_style_and_no_evidence() {
    let f = one(r#"from sqlmodel import Session, delete, select
from app.models import Hero

def a(session: Session):
    session.add(Hero(name="x"))
    session.exec(delete(Hero))
"#);
    assert_eq!(
        writes(&f),
        [
            (WriteOp::Insert, t("Hero"), t("a"), 5),
            (WriteOp::Delete, t("Hero"), t("a"), 6),
        ]
    );
    // A conventional session name in a file with no sign of an ORM is not read.
    let g = one("def a(db):\n    db.add(thing)\n");
    assert!(writes(&g).is_empty());
}

#[test]
fn asyncpg_writes() {
    let f = one(r#"import asyncpg

SQL = """
INSERT INTO events (kind) VALUES ($1)
"""

async def a(pool: asyncpg.Pool, rows):
    await pool.execute(SQL, "x")
    await pool.executemany("UPDATE events SET n = n + 1 WHERE id = $1", rows)

async def b(conn):
    row = await conn.fetchrow("INSERT INTO events (kind) VALUES ($1) RETURNING id", "k")
    await conn.fetch("SELECT * FROM events")
    await conn.copy_records_to_table("events", records=[(1,)])
    await conn.execute("DELETE FROM events WHERE id = $1", 1)
    q = "INSERT INTO ledger (a) VALUES ($1) ON CONFLICT (a) DO NOTHING"
    await conn.execute(q, 1)
    await conn.execute(build_sql(), 1)
"#);
    let w: Vec<_> = f
        .writes()
        .map(|w| {
            (
                w.op,
                w.table.clone(),
                w.enclosing.clone(),
                w.loc.line,
                w.framework,
            )
        })
        .collect();
    let a = Framework::Asyncpg;
    assert_eq!(
        w,
        [
            (WriteOp::Insert, t("events"), t("a"), 8, a),
            (WriteOp::Update, t("events"), t("a"), 9, a),
            (WriteOp::Insert, t("events"), t("b"), 12, a),
            (WriteOp::Insert, t("events"), t("b"), 14, a),
            (WriteOp::Delete, t("events"), t("b"), 15, a),
            (WriteOp::Upsert, t("ledger"), t("b"), 17, a),
        ]
    );
    assert!(f.frameworks().contains(&Framework::Asyncpg));
    assert!(!f.frameworks().contains(&Framework::SqlAlchemy));
}

#[test]
fn other_drivers_are_not_taken_for_asyncpg() {
    let f = one(r#"import sqlite3

def a(cur):
    cur.execute("INSERT INTO t VALUES (?)", (1,))
    conn = sqlite3.connect("x")
    conn.execute("DELETE FROM t")
"#);
    assert!(writes(&f).is_empty());
}

#[test]
fn session_execute_of_plain_string_is_sqlalchemy_not_asyncpg() {
    let f = one(r#"from sqlalchemy.orm import Session

def a(session: Session):
    session.execute("UPDATE t SET x = 1")
"#);
    let w: Vec<_> = f.writes().collect();
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].framework, Framework::SqlAlchemy);
}
