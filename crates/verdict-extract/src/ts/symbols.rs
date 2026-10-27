//! The TS/JS definitions pass: functions, components, classes, methods, module constants, the
//! calls between them (refs) and imports.

use super::{
    TS_LANGS, args, callee, enclosing, fn_binding_name, inside_fn, is_fn, is_stub_fn, key_name,
    kids, str_lit,
};
use crate::model::{DefKind, Definition, FileFacts, Import, Lang, Ref};
use crate::syntax::{end_line, for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// Definitions, refs and imports in TypeScript and JavaScript.
pub struct TsSymbols;

fn capitalized(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn fn_kind(name: &str, lang: Lang) -> DefKind {
    // A capitalized function in a file that can hold JSX is a React component.
    if capitalized(name) && lang != Lang::TypeScript {
        DefKind::Component
    } else {
        DefKind::Function
    }
}

fn def(out: &mut FileFacts, name: String, kind: DefKind, n: Node, is_stub: bool) {
    let loc = out.loc(line(n));
    out.defs.push(Definition {
        name,
        kind,
        loc,
        end_line: end_line(n),
        is_stub,
    });
}

fn add_ref(out: &mut FileFacts, n: Node, callee: &str, src: &str) {
    if callee.is_empty() {
        return;
    }
    let loc = out.loc(line(n));
    out.refs.push(Ref {
        caller: enclosing(n, src),
        callee: callee.to_string(),
        loc,
    });
}

impl Extractor for TsSymbols {
    fn name(&self) -> &'static str {
        "ts_symbols"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        let lang = s.lang;
        for_each_node(s.tree.root_node(), |n| match n.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = n.child_by_field_name("name") {
                    let name = text(name, src).to_string();
                    let kind = fn_kind(&name, lang);
                    def(out, name, kind, n, is_stub_fn(n, src));
                }
            }
            "class_declaration" | "abstract_class_declaration" => {
                if let Some(name) = n.child_by_field_name("name") {
                    def(out, text(name, src).to_string(), DefKind::Class, n, false);
                }
            }
            "method_definition" => {
                // Methods of classes, and of object literals at module level.
                let Some(name) = n.child_by_field_name("name").map(|k| key_name(k, src)) else {
                    return;
                };
                let owner = n.parent().and_then(|b| {
                    if b.kind() == "class_body" {
                        b.parent()
                            .and_then(|c| c.child_by_field_name("name"))
                            .map(|c| text(c, src).to_string())
                    } else {
                        None
                    }
                });
                match owner {
                    Some(cls) => def(
                        out,
                        format!("{cls}.{name}"),
                        DefKind::Method,
                        n,
                        is_stub_fn(n, src),
                    ),
                    None if !inside_fn(n) => {
                        def(out, name, DefKind::Method, n, is_stub_fn(n, src));
                    }
                    None => {}
                }
            }
            k if is_fn(n) || k == "arrow_function" => {
                if let Some(name) = fn_binding_name(n, src) {
                    let field = n
                        .parent()
                        .is_some_and(|p| matches!(p.kind(), "public_field_definition" | "field_definition"));
                    let kind = if field {
                        DefKind::Method
                    } else if name.starts_with("tool:") || name.contains(' ') {
                        DefKind::Function
                    } else {
                        fn_kind(&name, lang)
                    };
                    def(out, name, kind, n, is_stub_fn(n, src));
                }
            }
            "variable_declarator" => {
                // Module-level constants (functions bound to a name are listed above).
                let top = n
                    .parent()
                    .and_then(|d| d.parent())
                    .is_some_and(|p| matches!(p.kind(), "program" | "export_statement"));
                if !top {
                    return;
                }
                if let (Some(name), Some(v)) =
                    (n.child_by_field_name("name"), n.child_by_field_name("value"))
                    && name.kind() == "identifier"
                    && !is_fn(v)
                {
                    // `const s = createServerFn().handler(fn)` is the function `s`, not a constant.
                    let name = text(name, src).to_string();
                    let mut bound_fn = false;
                    for_each_node(v, |d| {
                        if !bound_fn && is_fn(d) && fn_binding_name(d, src).as_ref() == Some(&name) {
                            bound_fn = true;
                        }
                    });
                    if !bound_fn {
                        def(out, name, DefKind::Constant, n, false);
                    }
                }
            }
            "call_expression" => {
                if let Some((_, name)) = callee(n, src) {
                    add_ref(out, n, name, src);
                }
                // A function passed by name (`useServerFn(save)`, `.then(handle)`) is used here.
                for a in args(n) {
                    if a.kind() == "identifier" {
                        add_ref(out, a, text(a, src), src);
                    }
                }
            }
            "new_expression" => {
                if let Some(c) = n.child_by_field_name("constructor")
                    && c.kind() == "identifier"
                {
                    add_ref(out, n, text(c, src), src);
                }
            }
            "jsx_opening_element" | "jsx_self_closing_element" => {
                // `<Notes />` renders the Notes component.
                if let Some(name) = n.child_by_field_name("name") {
                    let t = match name.kind() {
                        "identifier" => text(name, src),
                        "member_expression" => name
                            .child_by_field_name("property")
                            .map(|p| text(p, src))
                            .unwrap_or(""),
                        _ => "",
                    };
                    if capitalized(t) {
                        add_ref(out, n, t, src);
                    }
                }
            }
            "jsx_expression" => {
                // `onClick={save}` hands `save` to the element.
                if let [only] = kids(n).as_slice()
                    && only.kind() == "identifier"
                {
                    add_ref(out, n, text(*only, src), src);
                }
            }
            "pair" => {
                // `{ component: Notes, handler: save }` names a function to call later.
                if let Some(v) = n.child_by_field_name("value")
                    && v.kind() == "identifier"
                {
                    add_ref(out, v, text(v, src), src);
                }
            }
            "import_statement" => {
                let Some(module) = n
                    .child_by_field_name("source")
                    .and_then(|m| str_lit(m, src))
                    .map(|(m, _)| m)
                else {
                    return;
                };
                let mut names = Vec::new();
                for clause in kids(n) {
                    if clause.kind() != "import_clause" {
                        continue;
                    }
                    for part in kids(clause) {
                        match part.kind() {
                            "identifier" => names.push(text(part, src).to_string()),
                            "namespace_import" => {
                                for id in kids(part) {
                                    names.push(text(id, src).to_string());
                                }
                            }
                            "named_imports" => {
                                for spec in kids(part) {
                                    if let Some(nm) = spec.child_by_field_name("name") {
                                        names.push(text(nm, src).to_string());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let loc = out.loc(line(n));
                out.imports.push(Import { module, names, loc });
            }
            _ => {}
        });
    }
}
