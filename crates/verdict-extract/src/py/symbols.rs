//! The Python definitions pass: functions, methods and classes (with the stub test), every call as
//! a reference from its enclosing function, and every import.

use super::common::{class_name, enclosing, func_name, has_decorator, is_method};
use crate::model::{DefKind, Definition, FileFacts, Import, Lang, Ref};
use crate::syntax::{end_line, for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// Definitions, references and imports for Python files.
pub struct PySymbols;

impl Extractor for PySymbols {
    fn name(&self) -> &'static str {
        "python_symbols"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        let text_src = src.text;
        let root = src.tree.root_node();
        let mut defs = Vec::new();
        let mut refs = Vec::new();
        let mut imports = Vec::new();
        for_each_node(root, |n| match n.kind() {
            "function_definition" => {
                let kind = if is_method(n) {
                    DefKind::Method
                } else {
                    DefKind::Function
                };
                defs.push(Definition {
                    name: func_name(n, text_src),
                    kind,
                    loc: out.loc(line(n)),
                    end_line: end_line(n),
                    is_stub: is_stub(n, text_src),
                });
            }
            "class_definition" => defs.push(Definition {
                name: class_name(n, text_src),
                kind: DefKind::Class,
                loc: out.loc(line(n)),
                end_line: end_line(n),
                // A class with an empty body (an exception, a marker) is complete as written.
                is_stub: false,
            }),
            "call" => {
                if let Some(callee) = callee_name(n, text_src) {
                    refs.push(Ref {
                        caller: enclosing(n, text_src),
                        callee,
                        loc: out.loc(line(n)),
                    });
                }
            }
            "import_statement" => {
                let mut cur = n.walk();
                for item in n.children_by_field_name("name", &mut cur) {
                    let name = match item.kind() {
                        "aliased_import" => item.child_by_field_name("name"),
                        _ => Some(item),
                    };
                    if let Some(name) = name {
                        imports.push(Import {
                            module: compact(text(name, text_src)),
                            names: Vec::new(),
                            loc: out.loc(line(n)),
                        });
                    }
                }
            }
            "import_from_statement" => {
                let module = n
                    .child_by_field_name("module_name")
                    .map(|m| compact(text(m, text_src)))
                    .unwrap_or_default();
                let mut names = Vec::new();
                let mut cur = n.walk();
                for item in n.children_by_field_name("name", &mut cur) {
                    let name = match item.kind() {
                        "aliased_import" => item.child_by_field_name("name"),
                        _ => Some(item),
                    };
                    if let Some(name) = name {
                        names.push(compact(text(name, text_src)));
                    }
                }
                let mut cur2 = n.walk();
                if n.children(&mut cur2).any(|c| c.kind() == "wildcard_import") {
                    names.push("*".to_string());
                }
                imports.push(Import {
                    module,
                    names,
                    loc: out.loc(line(n)),
                });
            }
            "future_import_statement" => {
                let mut names = Vec::new();
                let mut cur = n.walk();
                for item in n.children_by_field_name("name", &mut cur) {
                    let name = match item.kind() {
                        "aliased_import" => item.child_by_field_name("name"),
                        _ => Some(item),
                    };
                    if let Some(name) = name {
                        names.push(compact(text(name, text_src)));
                    }
                }
                imports.push(Import {
                    module: "__future__".into(),
                    names,
                    loc: out.loc(line(n)),
                });
            }
            _ => {}
        });
        out.defs.extend(defs);
        out.refs.extend(refs);
        out.imports.extend(imports);
    }
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The last name of what a call calls: `c` for `a.b.c()`, `f` for `f()`. `None` for `f()()`,
/// `x[0]()` and the like.
fn callee_name(call: Node, src: &str) -> Option<String> {
    let f = call.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => Some(text(f, src).to_string()),
        "attribute" => Some(text(f.child_by_field_name("attribute")?, src).to_string()),
        _ => None,
    }
}

/// True when a function body is only `pass`, `...`, a docstring or `raise NotImplementedError`.
/// Abstract methods and overloads are declarations by design, so they are not stubs.
fn is_stub(func: Node, src: &str) -> bool {
    if has_decorator(
        func,
        &["abstractmethod", "abstractproperty", "overload"],
        src,
    ) {
        return false;
    }
    let Some(body) = func.child_by_field_name("body") else {
        return false;
    };
    let mut cur = body.walk();
    let mut any = false;
    for stmt in body.named_children(&mut cur) {
        match stmt.kind() {
            "comment" | "pass_statement" => {}
            "expression_statement" => {
                let ok = stmt.named_child(0).is_some_and(|e| {
                    matches!(e.kind(), "string" | "concatenated_string" | "ellipsis")
                });
                if !ok {
                    return false;
                }
            }
            "raise_statement" => {
                if !raises_not_implemented(stmt, src) {
                    return false;
                }
            }
            _ => return false,
        }
        any = true;
    }
    any
}

fn raises_not_implemented(stmt: Node, src: &str) -> bool {
    let Some(e) = stmt.named_child(0) else {
        return false;
    };
    let target = if e.kind() == "call" {
        match e.child_by_field_name("function") {
            Some(f) => f,
            None => return false,
        }
    } else {
        e
    };
    let t = text(target, src);
    t == "NotImplementedError" || t == "builtins.NotImplementedError"
}
