//! Rust extractors on small sources: Tauri commands, stubs, refs.

use verdict_extract::*;

fn facts(src: &str) -> FileFacts {
    extract_source("src-tauri/src/main.rs", src, Lang::Rust, &registry())
}

#[test]
fn rs_tauri_commands_honor_rename() {
    let f = facts(
        r#"
use tauri::Manager;

#[tauri::command]
async fn save_note(n: String) -> Result<(), String> { store(n) }

#[tauri::command(rename = "list")]
fn list_notes() -> Vec<String> { vec![] }

#[tauri::command(rename_all = "snake_case")]
fn delete_note(id: u32) { todo!() }

fn helper() {}
"#,
    );
    let cmds: Vec<(&str, Option<&str>)> = f
        .endpoints
        .iter()
        .map(|e| {
            assert_eq!(e.key.kind, EndpointKind::TauriCommand);
            (e.key.name.as_str(), e.handler.as_deref())
        })
        .collect();
    assert_eq!(
        cmds,
        [
            ("save_note", Some("save_note")),
            ("list", Some("list_notes")),
            ("delete_note", Some("delete_note")),
        ]
    );
    assert!(f.frameworks.contains(&Framework::Tauri));
}

#[test]
fn rs_stubs_and_methods() {
    let f = facts(
        r#"
fn a() { todo!() }
fn b(x: u32) -> u32 { let _ = x; unimplemented!("later") }
fn c() -> u32 { if true { todo!() } 1 }
struct S;
impl S { pub fn m(&self) { unimplemented!() } fn n(&self) -> u8 { 1 } }
const MAX: u8 = 3;
"#,
    );
    let defs: Vec<(&str, DefKind, bool)> = f
        .defs
        .iter()
        .map(|d| (d.name.as_str(), d.kind, d.is_stub))
        .collect();
    assert_eq!(
        defs,
        [
            ("a", DefKind::Function, true),
            ("b", DefKind::Function, true),
            ("c", DefKind::Function, false),
            ("S", DefKind::Class, false),
            ("S.m", DefKind::Method, true),
            ("S.n", DefKind::Method, false),
            ("MAX", DefKind::Constant, false),
        ]
    );
}

#[test]
fn rs_refs_and_registered_commands() {
    let f = facts(
        r#"
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![save_note, notes::list_notes])
        .run(ctx());
    helper(1);
}
impl S { fn m(&self) { self.store.save(); Other::make(); } }
"#,
    );
    let refs: Vec<(Option<&str>, &str)> = f
        .refs
        .iter()
        .map(|r| (r.caller.as_deref(), r.callee.as_str()))
        .collect();
    for want in [
        (Some("main"), "save_note"),
        (Some("main"), "list_notes"),
        (Some("main"), "helper"),
        (Some("main"), "invoke_handler"),
        (Some("S.m"), "save"),
        (Some("S.m"), "make"),
    ] {
        assert!(refs.contains(&want), "missing {want:?} in {refs:?}");
    }
    assert!(!refs.contains(&(Some("main"), "notes")));
}
