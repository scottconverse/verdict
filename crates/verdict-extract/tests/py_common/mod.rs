//! Shared helpers for the Python extractor tests: build a small project in a temp folder and
//! read its facts.
#![allow(dead_code)]

use std::path::Path;
use verdict_extract::{
    Endpoint, EndpointKind, Facts, ScanOptions, extract_files, extract_tree, registry,
};

/// Write `files` into a temp folder and extract them (no git involved).
pub fn project(files: &[(&str, &str)]) -> Facts {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for (p, text) in files {
        let full = dir.path().join(p);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, text).unwrap();
        paths.push(p.to_string());
    }
    extract_files(dir.path(), &paths, &ScanOptions::default(), &registry())
}

/// One Python file.
pub fn one(src: &str) -> Facts {
    project(&[("app/main.py", src)])
}

/// `(method, name, handler, router)` of every endpoint of a kind, in file then source order.
pub fn eps(facts: &Facts, kind: EndpointKind) -> Vec<(String, String, String, Option<String>)> {
    facts
        .endpoints()
        .filter(|e| e.key.kind == kind)
        .map(|e| {
            (
                e.key.method.clone().unwrap_or_else(|| "*".into()),
                e.key.name.clone(),
                e.handler.clone().unwrap_or_default(),
                e.router.clone(),
            )
        })
        .collect()
}

/// `METHOD /path` strings, sorted.
pub fn routes(facts: &Facts) -> Vec<String> {
    let mut v: Vec<String> = facts
        .endpoints()
        .filter(|e| e.key.kind == EndpointKind::Http)
        .map(|e| format!("{} {}", e.key.method.as_deref().unwrap_or("*"), e.key.name))
        .collect();
    v.sort();
    v
}

pub fn endpoint<'a>(facts: &'a Facts, name: &str) -> &'a Endpoint {
    facts
        .endpoints()
        .find(|e| e.key.name == name)
        .unwrap_or_else(|| panic!("no endpoint {name}"))
}

/// Copy a fixture side (`base` then `change`, minus `deleted.txt`) into a temp folder and
/// extract the tree. `fixture` is `py_fastapi`, `kind` is `red` or `green`.
pub fn fixture_facts(fixture: &str, kind: &str) -> Facts {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../verdict-checkers/fixtures/wiring")
        .join(fixture)
        .join(kind);
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&root.join("base"), dir.path());
    copy_dir(&root.join("change"), dir.path());
    if let Ok(list) = std::fs::read_to_string(root.join("deleted.txt")) {
        for p in list.lines().map(str::trim).filter(|p| !p.is_empty()) {
            std::fs::remove_file(dir.path().join(p)).unwrap();
        }
    }
    extract_tree(dir.path(), &ScanOptions::default())
}

fn copy_dir(src: &Path, dst: &Path) {
    if !src.is_dir() {
        return;
    }
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), &to).unwrap();
        }
    }
}

/// True when a function named `handler` reaches a data write by following plain calls to
/// functions defined in the tree (a small version of what the wiring checker does).
pub fn reaches_write(facts: &Facts, handler: &str) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let mut todo = vec![handler.to_string()];
    while let Some(f) = todo.pop() {
        if !seen.insert(f.clone()) {
            continue;
        }
        if facts
            .writes()
            .any(|w| w.enclosing.as_deref() == Some(f.as_str()))
        {
            return true;
        }
        for r in facts
            .refs()
            .filter(|r| r.caller.as_deref() == Some(f.as_str()))
        {
            for d in facts.defs() {
                if d.name == r.callee || d.name.ends_with(&format!(".{}", r.callee)) {
                    todo.push(d.name.clone());
                }
            }
        }
    }
    false
}

/// The endpoint with this method and name.
pub fn endpoint_m<'a>(facts: &'a Facts, method: &str, name: &str) -> &'a Endpoint {
    facts
        .endpoints()
        .find(|e| e.key.name == name && e.key.method.as_deref() == Some(method))
        .unwrap_or_else(|| panic!("no endpoint {method} {name}"))
}
