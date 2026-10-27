//! Alembic migration facts.

mod py_common;

use py_common::*;
use verdict_extract::Framework;

const MIGRATION: &str = r#""""add notes

Revision ID: 0002
"""
import sqlalchemy as sa
from alembic import op

revision = "0002"
down_revision = "0001"


def upgrade():
    op.create_table(
        "notes",
        sa.Column("id", sa.Integer, primary_key=True),
    )
    op.add_column("users", sa.Column("note_count", sa.Integer))
    op.create_index("ix_notes_id", "notes", ["id"])
    op.create_foreign_key("fk_n", "notes", "users", ["uid"], ["id"])
    with op.batch_alter_table("accounts") as batch_op:
        batch_op.drop_column("old")
    op.execute("UPDATE settings SET v = 1")
    op.rename_table("a_old", "a_new")
    op.alter_column("users", "name", nullable=False)


def downgrade():
    op.drop_column("users", "note_count")
    op.drop_table("notes")
"#;

#[test]
fn tables_revision_and_line() {
    let f = project(&[("alembic/versions/0002_add_notes.py", MIGRATION)]);
    let file = f.file("alembic/versions/0002_add_notes.py").unwrap();
    assert_eq!(file.migrations.len(), 1);
    let m = &file.migrations[0];
    assert_eq!(m.revision.as_deref(), Some("0002"));
    assert_eq!(m.loc.line, 8);
    assert_eq!(
        m.tables,
        ["notes", "users", "accounts", "settings", "a_old", "a_new"]
    );
    assert_eq!(m.framework, Framework::Alembic);
    assert!(f.frameworks().contains(&Framework::Alembic));
}

#[test]
fn only_versions_files_with_a_revision() {
    let f = project(&[
        ("alembic/env.py", MIGRATION),
        ("alembic/versions/__init__.py", ""),
        (
            "alembic/versions/helper.py",
            "def f():\n    op.drop_table('x')\n",
        ),
        (
            "db/versions/0001.py",
            "from alembic import op\nrevision: str = 'abc'\ndef upgrade():\n    op.create_table('t')\n",
        ),
    ]);
    let all: Vec<_> = f
        .files
        .iter()
        .flat_map(|x| {
            x.migrations
                .iter()
                .map(|m| (x.path.clone(), m.revision.clone(), m.tables.clone()))
        })
        .collect();
    assert_eq!(
        all,
        [(
            "db/versions/0001.py".to_string(),
            Some("abc".to_string()),
            vec!["t".to_string()]
        )]
    );
}

#[test]
fn data_migration_with_bulk_insert_and_a_computed_table() {
    let src = r#"from alembic import op
import sqlalchemy as sa

revision = "r1"

def upgrade():
    t = sa.table("roles", sa.column("name"))
    op.bulk_insert(t, [{"name": "a"}])
    op.drop_table(some_name)
"#;
    let f = project(&[("m/versions/r1.py", src)]);
    let m = &f.file("m/versions/r1.py").unwrap().migrations[0];
    assert_eq!(m.tables, ["roles"]);
}
