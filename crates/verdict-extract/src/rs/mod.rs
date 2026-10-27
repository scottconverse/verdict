//! Rust extractors: Tauri `#[tauri::command]` handlers and the Rust definitions/calls pass.

use crate::model::{
    DefKind, Definition, Endpoint, EndpointKey, EndpointKind, FileFacts, Framework, Import, Lang,
    Ref,
};
use crate::syntax::{end_line, for_each_node, line, string_value, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

const RS_LANGS: &[Lang] = &[Lang::Rust];

/// Every Rust extractor.
pub fn extractors() -> Vec<Box<dyn Extractor>> {
    vec![Box::new(RustSymbols), Box::new(TauriCommands)]
}

/// Definitions, refs and `use` imports in Rust.
pub struct RustSymbols;

/// `#[tauri::command]` functions as Tauri command endpoints.
pub struct TauriCommands;

fn kids(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c)
        .filter(|k| !matches!(k.kind(), "line_comment" | "block_comment"))
        .collect()
}

/// The type an `impl` block is for, when `n` is a function inside one.
fn impl_type(n: Node, src: &str) -> Option<String> {
    let list = n.parent()?;
    if list.kind() != "declaration_list" {
        return None;
    }
    let owner = list.parent()?;
    let ty = match owner.kind() {
        "impl_item" => owner.child_by_field_name("type")?,
        "trait_item" => owner.child_by_field_name("name")?,
        _ => return None,
    };
    // `impl<T> Foo<T>` → `Foo`.
    let t = text(ty, src);
    Some(t.split('<').next().unwrap_or(t).trim().to_string())
}

/// The name the definitions pass gives a function item: `name` or `Type.method`.
fn fn_name(f: Node, src: &str) -> Option<String> {
    let name = text(f.child_by_field_name("name")?, src).to_string();
    Some(match impl_type(f, src) {
        Some(t) => format!("{t}.{name}"),
        None => name,
    })
}

/// The function a node sits in, by its definitions-pass name.
fn enclosing(n: Node, src: &str) -> Option<String> {
    let mut cur = n.parent();
    while let Some(c) = cur {
        if c.kind() == "function_item" {
            return fn_name(c, src);
        }
        cur = c.parent();
    }
    None
}

/// True when a macro invocation is `todo!(..)` or `unimplemented!(..)`.
fn is_todo_macro(n: Node, src: &str) -> bool {
    n.kind() == "macro_invocation"
        && n.child_by_field_name("macro")
            .is_some_and(|m| matches!(text(m, src), "todo" | "unimplemented"))
}

/// True when a function's body does nothing but `todo!()` / `unimplemented!()` (after ignoring
/// `let _ = arg;` lines that silence unused arguments).
fn is_stub(f: Node, src: &str) -> bool {
    let Some(body) = f.child_by_field_name("body") else {
        return false;
    };
    let stmts: Vec<Node> = kids(body)
        .into_iter()
        .filter(|s| !(s.kind() == "let_declaration" && text(*s, src).starts_with("let _")))
        .collect();
    match stmts.as_slice() {
        [one] => {
            is_todo_macro(*one, src)
                || (one.kind() == "expression_statement"
                    && kids(*one).first().is_some_and(|e| is_todo_macro(*e, src)))
        }
        _ => false,
    }
}

/// The last path segment a call expression calls.
fn callee_name<'s>(f: Node, src: &'s str) -> Option<&'s str> {
    match f.kind() {
        "identifier" => Some(text(f, src)),
        "field_expression" => f.child_by_field_name("field").map(|x| text(x, src)),
        "scoped_identifier" => f.child_by_field_name("name").map(|x| text(x, src)),
        "generic_function" => f
            .child_by_field_name("function")
            .and_then(|g| callee_name(g, src)),
        _ => None,
    }
}

fn add_ref(out: &mut FileFacts, n: Node, callee: &str, src: &str) {
    let loc = out.loc(line(n));
    out.refs.push(Ref {
        caller: enclosing(n, src),
        callee: callee.to_string(),
        loc,
    });
}

impl Extractor for RustSymbols {
    fn name(&self) -> &'static str {
        "rust_symbols"
    }

    fn langs(&self) -> &'static [Lang] {
        RS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        for_each_node(s.tree.root_node(), |n| match n.kind() {
            "function_item" => {
                if let Some(name) = fn_name(n, src) {
                    let kind = if name.contains('.') {
                        DefKind::Method
                    } else {
                        DefKind::Function
                    };
                    let loc = out.loc(line(n));
                    out.defs.push(Definition {
                        name,
                        kind,
                        loc,
                        end_line: end_line(n),
                        is_stub: is_stub(n, src),
                    });
                }
            }
            "struct_item" | "enum_item" | "trait_item" | "union_item" | "const_item"
            | "static_item" => {
                if let Some(name) = n.child_by_field_name("name") {
                    let kind = if matches!(n.kind(), "const_item" | "static_item") {
                        DefKind::Constant
                    } else {
                        DefKind::Class
                    };
                    let loc = out.loc(line(n));
                    out.defs.push(Definition {
                        name: text(name, src).to_string(),
                        kind,
                        loc,
                        end_line: end_line(n),
                        is_stub: false,
                    });
                }
            }
            "call_expression" => {
                if let Some(c) = n
                    .child_by_field_name("function")
                    .and_then(|f| callee_name(f, src))
                {
                    add_ref(out, n, c, src);
                }
            }
            "macro_invocation" => {
                // generate_handler![save_note, notes::list] registers commands by name.
                let is_handler_list = n
                    .child_by_field_name("macro")
                    .is_some_and(|m| text(m, src).ends_with("generate_handler"));
                if !is_handler_list {
                    return;
                }
                let mut names = Vec::new();
                for_each_node(n, |t| {
                    if t.kind() == "identifier"
                        && t.parent().is_some_and(|p| p.kind() == "token_tree")
                    {
                        names.push((t, text(t, src)));
                    }
                });
                // In `a::b`, only the last segment is the command name.
                let mut iter = names.iter().peekable();
                while let Some((node, name)) = iter.next() {
                    let next_is_path = iter.peek().is_some_and(|(nx, _)| {
                        node.end_byte() + 2 == nx.start_byte()
                            && src.get(node.end_byte()..nx.start_byte()) == Some("::")
                    });
                    if !next_is_path {
                        add_ref(out, *node, name, src);
                    }
                }
            }
            "use_declaration" => {
                if let Some(arg) = n.child_by_field_name("argument") {
                    let module = text(arg, src).to_string();
                    let last = module
                        .rsplit("::")
                        .next()
                        .unwrap_or("")
                        .trim_matches(['{', '}', ' '])
                        .to_string();
                    let loc = out.loc(line(n));
                    out.imports.push(Import {
                        module,
                        names: if last.is_empty() || last.contains(',') {
                            Vec::new()
                        } else {
                            vec![last]
                        },
                        loc,
                    });
                }
            }
            _ => {}
        });
    }
}

/// The `rename = "x"` value in a `#[tauri::command(...)]` attribute, if any (`rename_all` is a
/// different option and is ignored).
fn rename_of(attr: Node, src: &str) -> Option<String> {
    let mut found = None;
    let mut prev_ident: Option<&str> = None;
    let mut saw_eq = false;
    for_each_node(attr, |t| {
        if found.is_some() {
            return;
        }
        match t.kind() {
            "identifier" => {
                prev_ident = Some(text(t, src));
                saw_eq = false;
            }
            "=" => saw_eq = true,
            "string_literal" if saw_eq && prev_ident == Some("rename") => {
                found = string_value(t, src).map(|(v, _)| v);
            }
            _ => {}
        }
    });
    found
}

/// The `#[tauri::command]` attribute right above a function, if any.
fn command_attr<'t>(f: Node<'t>, src: &str) -> Option<Node<'t>> {
    let mut prev = f.prev_named_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let a = text(p, src);
                let compact: String = a.chars().filter(|c| !c.is_whitespace()).collect();
                if compact.starts_with("#[tauri::command") || compact.starts_with("#[command") {
                    return Some(p);
                }
            }
            "line_comment" | "block_comment" => {}
            _ => return None,
        }
        prev = p.prev_named_sibling();
    }
    None
}

impl Extractor for TauriCommands {
    fn name(&self) -> &'static str {
        "tauri_commands"
    }

    fn langs(&self) -> &'static [Lang] {
        RS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        // A bare `#[command]` counts only in a file that uses tauri.
        let uses_tauri = src.contains("tauri");
        for_each_node(s.tree.root_node(), |n| {
            if n.kind() != "function_item" {
                return;
            }
            let Some(attr) = command_attr(n, src) else {
                return;
            };
            if !uses_tauri {
                return;
            }
            let Some(handler) = fn_name(n, src) else {
                return;
            };
            let plain = n
                .child_by_field_name("name")
                .map(|x| text(x, src).to_string())
                .unwrap_or_default();
            let name = rename_of(attr, src).unwrap_or(plain);
            let loc = out.loc(line(n));
            out.endpoints.push(Endpoint {
                key: EndpointKey {
                    kind: EndpointKind::TauriCommand,
                    method: None,
                    name,
                },
                handler: Some(handler),
                router: None,
                loc,
                framework: Framework::Tauri,
            });
            out.frameworks.insert(Framework::Tauri);
        });
    }
}
