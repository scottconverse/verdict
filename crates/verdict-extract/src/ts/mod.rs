//! TypeScript and JavaScript extractors: fetch, openapi-typescript clients, TanStack Start server
//! functions and routes, Kysely, TS MCP tools, Tauri `invoke`, React buttons/forms/links, and the
//! TS/JS definitions/calls/imports pass.
//!
//! JS and JSX files share every extractor here: the JavaScript grammar parses JSX, and the node
//! kinds these extractors read (`call_expression`, `member_expression`, `jsx_element`, ...) are the
//! same in both grammars.

mod http;
mod kysely;
mod mcp;
mod react;
mod symbols;
mod tanstack;
mod tauri;

use crate::Extractor;
use crate::model::Lang;
use crate::syntax::{string_value, text};
use tree_sitter::Node;

/// The languages every extractor in this module reads.
pub(crate) const TS_LANGS: &[Lang] = &[Lang::TypeScript, Lang::Tsx, Lang::JavaScript, Lang::Jsx];

/// HTTP verbs as written in route handler objects and openapi-typescript clients.
pub(crate) const HTTP_VERBS: &[&str] =
    &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Every TypeScript/JavaScript extractor.
pub fn extractors() -> Vec<Box<dyn Extractor>> {
    vec![
        Box::new(symbols::TsSymbols),
        Box::new(http::Fetch),
        Box::new(http::OpenapiTs),
        Box::new(tanstack::TanStackStart),
        Box::new(tanstack::TanStackRouter),
        Box::new(kysely::Kysely),
        Box::new(mcp::McpTs),
        Box::new(tauri::TauriInvoke),
        Box::new(react::React),
    ]
}

/// The named children of a node, comments left out.
pub(crate) fn kids(n: Node) -> Vec<Node> {
    let mut c = n.walk();
    n.named_children(&mut c)
        .filter(|k| k.kind() != "comment")
        .collect()
}

/// The arguments of a call expression (comments left out).
pub(crate) fn args(call: Node) -> Vec<Node> {
    call.child_by_field_name("arguments")
        .map(kids)
        .unwrap_or_default()
}

/// What a call calls: the receiver (for `a.b(...)`) and the last name (`b`, or `f` for `f(...)`).
pub(crate) fn callee<'t, 's>(call: Node<'t>, src: &'s str) -> Option<(Option<Node<'t>>, &'s str)> {
    let f = call.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => Some((None, text(f, src))),
        "member_expression" => {
            let prop = f.child_by_field_name("property")?;
            Some((f.child_by_field_name("object"), text(prop, src)))
        }
        _ => None,
    }
}

/// The string value of a literal or template node, without quotes.
pub(crate) fn str_lit(n: Node, src: &str) -> Option<(String, bool)> {
    string_value(n, src)
}

/// The value node of `key` in an object literal (`{ key: value }`), if present.
pub(crate) fn prop<'t>(obj: Node<'t>, key: &str, src: &str) -> Option<Node<'t>> {
    if obj.kind() != "object" {
        return None;
    }
    for p in kids(obj) {
        if p.kind() == "pair"
            && let (Some(k), Some(v)) =
                (p.child_by_field_name("key"), p.child_by_field_name("value"))
            && key_name(k, src) == key
        {
            return Some(v);
        }
    }
    None
}

/// A property key as a plain name (`a`, `"a"`, `'a'`).
pub(crate) fn key_name(k: Node, src: &str) -> String {
    match str_lit(k, src) {
        Some((s, _)) => s,
        None => text(k, src).to_string(),
    }
}

/// True for an arrow function or function expression.
pub(crate) fn is_fn(n: Node) -> bool {
    matches!(
        n.kind(),
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
}

/// True when a function's body is a stub: an empty block, or a block whose only statement throws
/// an error that says the code is not implemented (or is a TODO).
pub(crate) fn is_stub_fn(f: Node, src: &str) -> bool {
    let Some(body) = f.child_by_field_name("body") else {
        return false;
    };
    if body.kind() != "statement_block" {
        return false;
    }
    let stmts = kids(body);
    match stmts.as_slice() {
        [] => true,
        [one] if one.kind() == "throw_statement" => {
            let t = text(*one, src).to_ascii_lowercase();
            t.contains("not implemented")
                || t.contains("not yet implemented")
                || t.contains("unimplemented")
                || t.contains("todo")
        }
        _ => false,
    }
}

/// The class name of a method or field's enclosing class body.
fn class_name(n: Node, src: &str) -> Option<String> {
    let body = n.parent()?;
    if body.kind() != "class_body" {
        return None;
    }
    let class = body.parent()?;
    class
        .child_by_field_name("name")
        .map(|k| text(k, src).to_string())
}

/// True when a variable declarator sits at module level (`const x = ...` or `export const x`).
fn top_level_declarator(d: Node) -> bool {
    let Some(decl) = d.parent() else { return false };
    if !matches!(decl.kind(), "lexical_declaration" | "variable_declaration") {
        return false;
    }
    match decl.parent() {
        Some(p) if p.kind() == "program" => true,
        Some(p) if p.kind() == "export_statement" => true,
        _ => false,
    }
}

/// The name of the route an inline verb handler serves, `GET /api/x`, when `pair` is
/// `GET: fn` inside a TanStack Start server route.
pub(crate) fn verb_handler_name(pair: Node, src: &str) -> Option<String> {
    let k = pair.child_by_field_name("key")?;
    let verb = key_name(k, src);
    if !HTTP_VERBS.contains(&verb.as_str()) {
        return None;
    }
    let path = tanstack::route_path_above(pair, src)?;
    Some(format!("{verb} {path}"))
}

/// The function, method or component a node sits in, by the name the definitions pass gives it:
/// `name` for functions and module-level `const name = () => ...` (including
/// `const name = createServerFn().handler(fn)`), `Class.method` for methods and arrow fields, the
/// key for an object-literal function, `tool:<name>` for an inline MCP tool handler and
/// `GET /path` for an inline TanStack Start route handler.
pub(crate) fn enclosing(n: Node, src: &str) -> Option<String> {
    let mut cur = n.parent();
    while let Some(c) = cur {
        match c.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = c.child_by_field_name("name") {
                    return Some(text(name, src).to_string());
                }
            }
            "method_definition" => {
                let name = c.child_by_field_name("name").map(|k| key_name(k, src))?;
                return Some(match class_name(c, src) {
                    Some(cls) => format!("{cls}.{name}"),
                    None => name,
                });
            }
            k if is_fn(c) || k == "arrow_function" => {
                if let Some(name) = fn_binding_name(c, src) {
                    return Some(name);
                }
            }
            _ => {}
        }
        cur = c.parent();
    }
    None
}

/// The name a function expression is bound to, if the definitions pass names it.
pub(crate) fn fn_binding_name(f: Node, src: &str) -> Option<String> {
    let p = f.parent()?;
    match p.kind() {
        "variable_declarator" => p
            .child_by_field_name("name")
            .map(|k| text(k, src).to_string()),
        "public_field_definition" | "field_definition" => {
            let name = p
                .child_by_field_name("name")
                .or_else(|| p.child_by_field_name("property"))
                .map(|k| key_name(k, src))?;
            Some(match class_name(p, src) {
                Some(cls) => format!("{cls}.{name}"),
                None => name,
            })
        }
        // A verb handler in a server route, or a function in a module-level object literal. A
        // function in an object built inside another function belongs to that function.
        "pair" => verb_handler_name(p, src).or_else(|| {
            if inside_fn(p) {
                return None;
            }
            p.child_by_field_name("key")
                .map(|k| key_name(k, src))
                .filter(|k| !k.is_empty())
        }),
        "arguments" => {
            let call = p.parent()?;
            if let Some((Some(_), m)) = callee(call, src)
                && (m == "tool" || m == "registerTool")
                && let Some(first) = args(call).first()
                && let Some((name, _)) = str_lit(*first, src)
            {
                return Some(format!("tool:{name}"));
            }
            // `const x = make(...).handler(fn)` or `const x = useCallback(fn)`: the function is
            // the body of `x`. Climb the call chain to a module-level declarator.
            let mut up = Some(call);
            while let Some(u) = up {
                match u.kind() {
                    "call_expression" | "member_expression" | "arguments" | "await_expression" => {
                        up = u.parent()
                    }
                    "variable_declarator" if top_level_declarator(u) => {
                        return u
                            .child_by_field_name("name")
                            .map(|k| text(k, src).to_string());
                    }
                    _ => break,
                }
            }
            None
        }
        _ => None,
    }
}

/// True when a node sits inside a function, method or arrow.
pub(crate) fn inside_fn(n: Node) -> bool {
    let mut cur = n.parent();
    while let Some(c) = cur {
        if is_fn(c)
            || matches!(
                c.kind(),
                "function_declaration" | "generator_function_declaration" | "method_definition"
            )
        {
            return true;
        }
        cur = c.parent();
    }
    false
}

/// True when the file imports from a module whose name starts with `prefix`.
pub(crate) fn imports_from(root: Node, src: &str, prefix: &str) -> bool {
    kids(root).into_iter().any(|s| {
        s.kind() == "import_statement"
            && s.child_by_field_name("source")
                .and_then(|m| str_lit(m, src))
                .is_some_and(|(m, _)| m.starts_with(prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{for_each_node, parse};

    fn enclosing_of_call(src: &str, callee_name: &str) -> Option<String> {
        let tree = parse(Lang::TypeScript, src).unwrap();
        let mut found = None;
        for_each_node(tree.root_node(), |n| {
            if n.kind() == "call_expression"
                && found.is_none()
                && callee(n, src).is_some_and(|(_, c)| c == callee_name)
            {
                found = Some(enclosing(n, src));
            }
        });
        found.flatten()
    }

    #[test]
    fn enclosing_names_match_the_definitions_pass() {
        assert_eq!(
            enclosing_of_call("function f() { g(); }", "g").as_deref(),
            Some("f")
        );
        assert_eq!(
            enclosing_of_call("class K { m() { g(); } }", "g").as_deref(),
            Some("K.m")
        );
        assert_eq!(
            enclosing_of_call(
                "const s = createServerFn().handler(async () => { g(); });",
                "g"
            )
            .as_deref(),
            Some("s")
        );
        assert_eq!(
            enclosing_of_call("server.tool('add', {}, async () => { g(); });", "g").as_deref(),
            Some("tool:add")
        );
        assert_eq!(
            enclosing_of_call("function f() { items.map((x) => g(x)); }", "g").as_deref(),
            Some("f")
        );
        assert_eq!(enclosing_of_call("g();", "g"), None);
    }

    #[test]
    fn stubs() {
        let src = "function a() {}\nfunction b() { throw new Error('Not implemented'); }\nfunction c() { return 1; }\n";
        let tree = parse(Lang::TypeScript, src).unwrap();
        let mut got = Vec::new();
        for_each_node(tree.root_node(), |n| {
            if n.kind() == "function_declaration" {
                got.push(is_stub_fn(n, src));
            }
        });
        assert_eq!(got, [true, true, false]);
    }
}
