//! The wiring fixtures for the Python frameworks: in each red tree the facts show the break the
//! claim runs into; in each green tree they do not.
//!
//! These tests read only extractor facts. The wiring checker itself is built elsewhere; whether
//! it answers `broken`, `not_built` or `half_built` for each red tree is checked there.

mod py_common;

use py_common::*;
use std::path::Path;
use verdict_extract::http::paths_match;
use verdict_extract::{EndpointKind, Facts, Framework, WriteOp};

fn session_json(fixture: &str, kind: &str) -> serde_json::Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../verdict-checkers/fixtures/wiring")
        .join(fixture)
        .join(kind)
        .join("session.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// The first backticked name in the claims that starts with `/`.
fn claimed_path(fixture: &str, kind: &str) -> String {
    let v = session_json(fixture, kind);
    for c in v["claims"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        for span in text.split('`').skip(1).step_by(2) {
            if span.starts_with('/') {
                return span.to_string();
            }
        }
    }
    panic!("{fixture}/{kind}: no claimed path");
}

fn serves(f: &Facts, path: &str) -> bool {
    f.endpoints()
        .any(|e| e.key.kind == EndpointKind::Http && paths_match(&e.key.name, path))
}

fn handler_of<'a>(f: &'a Facts, name: &str) -> &'a verdict_extract::Definition {
    f.defs()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no definition {name}"))
}

/// The TypeScript the fixture's page calls: the URL in its `fetch("...")`.
fn fetched(fixture: &str, kind: &str, file: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../verdict-checkers/fixtures/wiring")
        .join(fixture)
        .join(kind)
        .join("change")
        .join(file);
    let text = std::fs::read_to_string(p).unwrap();
    let start = text.find("fetch(\"").unwrap() + 7;
    text[start..start + text[start..].find('"').unwrap()].to_string()
}

#[test]
fn every_fixture_has_both_sides_and_the_files_the_harness_reads() {
    for fx in [
        "py_fastapi",
        "py_sqlalchemy",
        "py_asyncpg",
        "py_alembic",
        "py_typer",
        "py_mcp",
    ] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../verdict-checkers/fixtures/wiring")
            .join(fx);
        for kind in ["red", "green"] {
            assert!(root.join(kind).join("base").is_dir(), "{fx}/{kind}/base");
            assert!(
                root.join(kind).join("change").is_dir(),
                "{fx}/{kind}/change"
            );
            assert!(
                root.join(kind).join("session.json").is_file(),
                "{fx}/{kind}/session.json"
            );
        }
        let expect = std::fs::read_to_string(root.join("red/expect.txt")).unwrap();
        let rules: Vec<&str> = expect
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert!(!rules.is_empty());
        assert!(
            rules
                .iter()
                .all(|r| ["broken", "not_built", "half_built"].contains(r)),
            "{rules:?}"
        );
        assert!(!root.join("green/expect.txt").exists());
    }
}

#[test]
fn fastapi_red_calls_a_path_no_route_serves() {
    let red = fixture_facts("py_fastapi", "red");
    let green = fixture_facts("py_fastapi", "green");
    // The mount prefix is /api and the router's own prefix /notes, so the route is /api/notes.
    for f in [&red, &green] {
        let e = endpoint(f, "/api/notes");
        assert_eq!(e.key.method.as_deref(), Some("POST"));
        assert_eq!(e.handler.as_deref(), Some("create_note"));
        assert_eq!(e.loc.file, "app/routers/notes.py");
        assert_eq!(e.router, None);
    }
    let want_red = claimed_path("py_fastapi", "red");
    let want_green = claimed_path("py_fastapi", "green");
    assert_eq!(
        fetched("py_fastapi", "red", "web/src/NotesPage.tsx"),
        want_red
    );
    assert_eq!(
        fetched("py_fastapi", "green", "web/src/NotesPage.tsx"),
        want_green
    );
    assert!(
        !serves(&red, &want_red),
        "red: {want_red} must have no route"
    );
    assert!(
        serves(&green, &want_green),
        "green: {want_green} must have a route"
    );
    assert!(red.frameworks().contains(&Framework::FastApi));
    assert!(red.is_complete() && green.is_complete());
}

#[test]
fn sqlalchemy_red_handler_never_reaches_a_write() {
    let red = fixture_facts("py_sqlalchemy", "red");
    let green = fixture_facts("py_sqlalchemy", "green");
    for f in [&red, &green] {
        assert!(serves(f, &claimed_path("py_sqlalchemy", "green")));
        assert_eq!(
            endpoint_m(f, "POST", "/api/notes").handler.as_deref(),
            Some("create_note")
        );
        assert!(!handler_of(f, "create_note").is_stub);
    }
    assert!(!reaches_write(&red, "create_note"));
    assert!(red.writes().next().is_none(), "red has no write anywhere");
    assert!(reaches_write(&green, "create_note"));
    let w: Vec<_> = green.writes().collect();
    assert_eq!(w.len(), 1);
    assert_eq!(
        (w[0].op, w[0].table.as_deref()),
        (WriteOp::Insert, Some("Note"))
    );
    assert_eq!(w[0].enclosing.as_deref(), Some("save_note"));
    assert_eq!(w[0].loc.file, "app/services.py");
    assert_eq!(w[0].framework, Framework::SqlAlchemy);
}

#[test]
fn asyncpg_red_handler_is_a_stub_and_the_insert_is_not_reached() {
    let red = fixture_facts("py_asyncpg", "red");
    let green = fixture_facts("py_asyncpg", "green");
    for f in [&red, &green] {
        assert!(serves(f, &claimed_path("py_asyncpg", "green")));
        assert_eq!(
            endpoint(f, "/api/events").handler.as_deref(),
            Some("create_event")
        );
        let w: Vec<_> = f.writes().collect();
        assert_eq!(w.len(), 1, "the INSERT is in the tree on both sides");
        assert_eq!(
            (w[0].op, w[0].table.as_deref()),
            (WriteOp::Insert, Some("events"))
        );
        assert_eq!(w[0].framework, Framework::Asyncpg);
        assert_eq!(w[0].enclosing.as_deref(), Some("insert_event"));
    }
    assert!(handler_of(&red, "create_event").is_stub);
    assert!(!reaches_write(&red, "create_event"));
    assert!(!handler_of(&green, "create_event").is_stub);
    assert!(reaches_write(&green, "create_event"));
}

#[test]
fn alembic_red_has_no_migration_for_the_claimed_table() {
    let red = fixture_facts("py_alembic", "red");
    let green = fixture_facts("py_alembic", "green");
    let tables = |f: &Facts| -> Vec<String> {
        f.files
            .iter()
            .flat_map(|x| x.migrations.iter())
            .flat_map(|m| m.tables.clone())
            .collect()
    };
    // Code writes to the model on both sides.
    for f in [&red, &green] {
        let w: Vec<_> = f.writes().collect();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].table.as_deref(), Some("Note"));
        assert!(
            tables(f).contains(&"users".to_string()),
            "the first migration is read"
        );
    }
    assert!(red.file("alembic/versions/0002_add_notes.py").is_none());
    assert!(!tables(&red).contains(&"notes".to_string()));
    let m = &green
        .file("alembic/versions/0002_add_notes.py")
        .unwrap()
        .migrations[0];
    assert_eq!(m.revision.as_deref(), Some("0002"));
    assert_eq!(m.tables, ["notes"]);
    assert_eq!(m.loc.line, 8);
}

#[test]
fn typer_red_command_is_not_mounted() {
    let red = fixture_facts("py_typer", "red");
    let green = fixture_facts("py_typer", "green");
    let cmds = |f: &Facts| eps(f, EndpointKind::CliCommand);
    assert_eq!(
        cmds(&green),
        [
            (
                "*".to_string(),
                "export report".to_string(),
                "report".to_string(),
                None
            ),
            (
                "*".to_string(),
                "version".to_string(),
                "version".to_string(),
                None
            ),
        ]
    );
    let r = cmds(&red);
    assert!(!r.iter().any(|c| c.1 == "export report"));
    let report = r
        .iter()
        .find(|c| c.2 == "report")
        .expect("the command is in the source");
    assert_eq!(
        report.3.as_deref(),
        Some("cli/export.py:app"),
        "unmounted, so the router stays set"
    );
    assert!(r.iter().any(|c| c.1 == "version" && c.3.is_none()));
}

#[test]
fn mcp_red_tool_is_a_stub() {
    let red = fixture_facts("py_mcp", "red");
    let green = fixture_facts("py_mcp", "green");
    for f in [&red, &green] {
        let names: Vec<_> = eps(f, EndpointKind::McpTool)
            .into_iter()
            .map(|e| e.1)
            .collect();
        assert_eq!(names, ["list_notes", "delete_note"]);
        assert!(eps(f, EndpointKind::McpTool).iter().all(|e| e.3.is_none()));
        assert!(!handler_of(f, "list_notes").is_stub);
    }
    assert!(handler_of(&red, "delete_note").is_stub);
    assert!(!handler_of(&green, "delete_note").is_stub);
}
