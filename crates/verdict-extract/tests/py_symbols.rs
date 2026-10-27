//! The Python definitions, references and imports pass.

mod py_common;

use py_common::*;
use verdict_extract::DefKind;

#[test]
fn definitions_and_the_stub_test() {
    let f = one(r#"
def real():
    return 1

def stub_pass():
    pass

def stub_dots(): ...

def stub_doc():
    """Does a thing."""

def stub_raise():
    """todo"""
    raise NotImplementedError("later")

def stub_raise_bare():
    raise NotImplementedError

def not_stub_raise_other():
    raise ValueError("no")

def not_stub_mixed():
    x = 1
    raise NotImplementedError

def commented():
    # nothing yet
    pass

async def not_stub_async():
    await go()

class Empty(Exception):
    pass

class Thing:
    def method(self):
        pass

    async def amethod(self):
        return 2

    class Inner:
        def deep(self):
            ...

    @abstractmethod
    def declared(self):
        ...

    @staticmethod
    def helper():
        return 3

def outer():
    def nested():
        pass
    return nested
"#);
    let d: Vec<_> = f
        .defs()
        .map(|d| (d.name.as_str(), d.kind, d.is_stub))
        .collect();
    use DefKind::*;
    assert_eq!(
        d,
        [
            ("real", Function, false),
            ("stub_pass", Function, true),
            ("stub_dots", Function, true),
            ("stub_doc", Function, true),
            ("stub_raise", Function, true),
            ("stub_raise_bare", Function, true),
            ("not_stub_raise_other", Function, false),
            ("not_stub_mixed", Function, false),
            ("commented", Function, true),
            ("not_stub_async", Function, false),
            ("Empty", Class, false),
            ("Thing", Class, false),
            ("Thing.method", Method, true),
            ("Thing.amethod", Method, false),
            ("Thing.Inner", Class, false),
            ("Thing.Inner.deep", Method, true),
            ("Thing.declared", Method, false),
            ("Thing.helper", Method, false),
            ("outer", Function, false),
            ("nested", Function, true),
        ]
    );
    let real = f.defs().find(|d| d.name == "real").unwrap();
    assert_eq!((real.loc.line, real.end_line), (2, 3));
    let thing = f.defs().find(|d| d.name == "Thing").unwrap();
    assert_eq!((thing.loc.line, thing.end_line), (37, 54));
}

#[test]
fn decorated_definitions_start_at_the_def_line() {
    let f = one("import x\n\n@x.deco\n@x.other()\ndef f():\n    return 1\n");
    let d = f.defs().next().unwrap();
    assert_eq!((d.loc.line, d.end_line), (5, 6));
}

#[test]
fn every_call_is_a_ref_from_its_enclosing_function() {
    let f = one(r#"import logging
setup()

def handler(db):
    row = load(db).first()
    save(row)
    logging.getLogger("x").info("done")
    return [fmt(r) for r in rows]

class S:
    def run(self):
        self.step()
        super().run()
        return (lambda: inner())()

    @helper(1)
    def decorated(self):
        pass

def outer():
    def inner():
        deep()
    inner()
"#);
    let r: Vec<_> = f
        .refs()
        .map(|r| {
            (
                r.caller.as_deref().unwrap_or("-"),
                r.callee.as_str(),
                r.loc.line,
            )
        })
        .collect();
    assert_eq!(
        r,
        [
            ("-", "setup", 2),
            ("handler", "first", 5),
            ("handler", "load", 5),
            ("handler", "save", 6),
            ("handler", "info", 7),
            ("handler", "getLogger", 7),
            ("handler", "fmt", 8),
            ("S.run", "step", 12),
            ("S.run", "run", 13),
            ("S.run", "super", 13),
            ("S.run", "inner", 14),
            ("-", "helper", 16),
            ("inner", "deep", 22),
            ("outer", "inner", 23),
        ]
    );
}

#[test]
fn imports_of_every_shape() {
    let f = one(r#"from __future__ import annotations
import os, sys as system
import a.b.c
import a.b as ab
from m import x, y as z
from . import sibling
from ..pkg.mod import thing
from star import *

def late():
    import json
"#);
    let file = f.file("app/main.py").unwrap();
    let i: Vec<_> = file
        .imports
        .iter()
        .map(|i| (i.module.as_str(), i.names.join(","), i.loc.line))
        .collect();
    assert_eq!(
        i,
        [
            ("__future__", "annotations".to_string(), 1),
            ("os", String::new(), 2),
            ("sys", String::new(), 2),
            ("a.b.c", String::new(), 3),
            ("a.b", String::new(), 4),
            ("m", "x,y".to_string(), 5),
            (".", "sibling".to_string(), 6),
            ("..pkg.mod", "thing".to_string(), 7),
            ("star", "*".to_string(), 8),
            ("json", String::new(), 11),
        ]
    );
}

#[test]
fn syntax_errors_still_give_facts() {
    let f = one("def ok():\n    pass\n\ndef broken(:\n    pass\n");
    let file = f.file("app/main.py").unwrap();
    assert!(file.parse_errors);
    assert!(file.defs.iter().any(|d| d.name == "ok"));
}
