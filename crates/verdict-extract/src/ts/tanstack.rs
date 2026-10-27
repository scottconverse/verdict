//! TanStack Start server functions and server routes, and TanStack Router pages.

use super::{HTTP_VERBS, TS_LANGS, args, callee, is_fn, key_name, kids, prop, str_lit};
use crate::http::normalize_path;
use crate::model::{
    Call, Endpoint, EndpointKey, EndpointKind, Facts, FileFacts, Framework, Lang, Page,
};
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

/// `createServerFn` endpoints, the calls to them, and server route handlers.
pub struct TanStackStart;

/// `createFileRoute("/path")` and file-route pages.
pub struct TanStackRouter;

/// Route constructors whose first argument is the route path.
const ROUTE_CALLS: &[&str] = &[
    "createFileRoute",
    "createLazyFileRoute",
    "createServerFileRoute",
    "createAPIFileRoute",
    "createServerRoute",
];

/// Follow a call chain inward (`a(...)(...)`, `a(...).b(...)`) to its innermost calls, yielding
/// each call's callee name and node.
fn chain_calls<'t>(start: Node<'t>, src: &str) -> Vec<(String, Node<'t>)> {
    let mut out = Vec::new();
    let mut cur = Some(start);
    while let Some(c) = cur {
        match c.kind() {
            "call_expression" => {
                if let Some((_, name)) = callee(c, src) {
                    out.push((name.to_string(), c));
                }
                cur = c.child_by_field_name("function");
            }
            "member_expression" => cur = c.child_by_field_name("object"),
            "await_expression" | "parenthesized_expression" => {
                cur = kids(c).into_iter().next();
            }
            _ => break,
        }
    }
    out
}

/// The route constructor in a call chain and its literal path, if any (`None` path when the
/// constructor has no string argument).
fn route_ctor<'t>(start: Node<'t>, src: &str) -> Option<(String, Node<'t>, Option<String>)> {
    chain_calls(start, src)
        .into_iter()
        .find(|(name, _)| ROUTE_CALLS.contains(&name.as_str()))
        .map(|(name, call)| {
            let path = args(call)
                .first()
                .and_then(|a| str_lit(*a, src))
                .map(|(p, _)| normalize_path(&p));
            (name, call, path)
        })
}

/// The path of the TanStack Start server route a node sits in, normalized.
pub(crate) fn route_path_above(n: Node, src: &str) -> Option<String> {
    let mut cur = n.parent();
    while let Some(c) = cur {
        if c.kind() == "call_expression"
            && let Some((_, _, Some(path))) = route_ctor(c, src)
        {
            return Some(path);
        }
        cur = c.parent();
    }
    None
}

/// The route path a file-based route file serves, from its path under a `routes/` folder.
/// `None` for the root layout and files outside a `routes/` folder.
pub(crate) fn file_route(path: &str) -> Option<String> {
    let idx = path.rfind("routes/")?;
    if idx > 0 && !path[..idx].ends_with('/') {
        return None;
    }
    let rel = &path[idx + "routes/".len()..];
    let stem = rel.rsplit_once('.').map(|(s, _)| s).unwrap_or(rel);
    let stem = stem.strip_suffix(".lazy").unwrap_or(stem);
    let mut segs = Vec::new();
    for part in stem.split(['/', '.']) {
        if part == "__root" {
            return None;
        }
        if part.is_empty()
            || part == "index"
            || part == "route"
            || part.starts_with('_')
            || (part.starts_with('(') && part.ends_with(')'))
        {
            continue;
        }
        segs.push(part.trim_end_matches('_'));
    }
    Some(normalize_path(&segs.join("/")))
}

fn endpoint(out: &mut FileFacts, key: EndpointKey, handler: Option<String>, n: Node) {
    let loc = out.loc(line(n));
    out.endpoints.push(Endpoint {
        key,
        handler,
        router: None,
        loc,
        framework: Framework::TanStackStart,
    });
    out.frameworks.insert(Framework::TanStackStart);
}

/// The verb handlers in a `{ GET: fn, POST: name }` object.
fn verb_handlers(obj: Node, path: &str, src: &str, out: &mut FileFacts) {
    if obj.kind() != "object" {
        return;
    }
    for p in kids(obj) {
        if p.kind() != "pair" {
            continue;
        }
        let (Some(k), Some(v)) = (p.child_by_field_name("key"), p.child_by_field_name("value"))
        else {
            continue;
        };
        let verb = key_name(k, src);
        if !HTTP_VERBS.contains(&verb.as_str()) {
            continue;
        }
        let handler = if v.kind() == "identifier" {
            Some(text(v, src).to_string())
        } else if is_fn(v) {
            Some(format!("{verb} {path}"))
        } else {
            None
        };
        let key = EndpointKey {
            kind: EndpointKind::Http,
            method: Some(verb),
            name: path.to_string(),
        };
        endpoint(out, key, handler, p);
    }
}

impl Extractor for TanStackStart {
    fn name(&self) -> &'static str {
        "tanstack_start"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        for_each_node(s.tree.root_node(), |n| match n.kind() {
            "variable_declarator" => {
                // const saveNote = createServerFn({ method: "POST" }).handler(...)
                let (Some(name), Some(v)) =
                    (n.child_by_field_name("name"), n.child_by_field_name("value"))
                else {
                    return;
                };
                if name.kind() != "identifier" {
                    return;
                }
                if chain_calls(v, src)
                    .iter()
                    .any(|(c, _)| c == "createServerFn")
                {
                    let name = text(name, src).to_string();
                    let key = EndpointKey {
                        kind: EndpointKind::ServerFn,
                        method: None,
                        name: name.clone(),
                    };
                    endpoint(out, key, Some(name), n);
                }
            }
            "call_expression" => {
                let Some((ctor, ctor_call, path)) = route_ctor(n, src) else {
                    return;
                };
                let Some(path) = path else { return };
                if ctor_call == n {
                    return;
                }
                let is_outer_of_ctor = n.child_by_field_name("function") == Some(ctor_call);
                let Some(obj) = args(n).first().copied() else {
                    return;
                };
                match callee(n, src) {
                    // createServerFileRoute("/api/x").methods({ GET: ... })
                    Some((Some(_), "methods")) => verb_handlers(obj, &path, src, out),
                    _ if is_outer_of_ctor => {
                        if ctor == "createFileRoute" || ctor == "createLazyFileRoute" {
                            // createFileRoute("/api/x")({ server: { handlers: { GET } } })
                            if let Some(h) = prop(obj, "server", src)
                                .and_then(|server| prop(server, "handlers", src))
                            {
                                verb_handlers(h, &path, src, out);
                            }
                        } else {
                            // createAPIFileRoute("/api/x")({ GET: ... })
                            verb_handlers(obj, &path, src, out);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        });
    }

    /// A server function is called wherever its name is called or handed on
    /// (`saveNote({ data })`, `useServerFn(saveNote)`).
    fn link(&self, facts: &mut Facts) {
        let mut fns: BTreeMap<String, EndpointKey> = BTreeMap::new();
        for e in facts.endpoints() {
            if e.key.kind == EndpointKind::ServerFn {
                fns.insert(e.key.name.clone(), e.key.clone());
            }
        }
        if fns.is_empty() {
            return;
        }
        for f in &mut facts.files {
            if !TS_LANGS.contains(&f.lang) {
                continue;
            }
            let mut seen = BTreeSet::new();
            let mut new_calls = Vec::new();
            for r in &f.refs {
                if let Some(key) = fns.get(&r.callee)
                    && r.caller.as_deref() != Some(r.callee.as_str())
                    && seen.insert((r.loc.line, r.callee.clone()))
                {
                    new_calls.push(Call {
                        target: key.clone(),
                        enclosing: r.caller.clone(),
                        dynamic: false,
                        loc: r.loc.clone(),
                        framework: Framework::TanStackStart,
                    });
                }
            }
            if !new_calls.is_empty() {
                f.frameworks.insert(Framework::TanStackStart);
                f.calls.extend(new_calls);
            }
        }
    }
}

impl Extractor for TanStackRouter {
    fn name(&self) -> &'static str {
        "tanstack_router"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        let mut pages = Vec::new();
        for_each_node(s.tree.root_node(), |n| {
            if n.kind() != "call_expression" {
                return;
            }
            let Some((_, name)) = callee(n, src) else {
                return;
            };
            match name {
                "createFileRoute" | "createLazyFileRoute" => {
                    let path = args(n)
                        .first()
                        .and_then(|a| str_lit(*a, src))
                        .map(|(p, _)| normalize_path(&p))
                        .or_else(|| file_route(s.path));
                    let Some(path) = path else { return };
                    // The options object is the argument of the call that wraps this one.
                    let opts = n
                        .parent()
                        .filter(|p| {
                            p.kind() == "call_expression"
                                && p.child_by_field_name("function") == Some(n)
                        })
                        .and_then(|p| args(p).first().copied());
                    let component = opts.and_then(|o| prop(o, "component", src));
                    let server_only = opts.is_some_and(|o| prop(o, "server", src).is_some())
                        && component.is_none();
                    if server_only {
                        return;
                    }
                    let component = component
                        .filter(|c| c.kind() == "identifier")
                        .map(|c| text(c, src).to_string());
                    pages.push((path, component, line(n)));
                }
                "createRoute" => {
                    // createRoute({ getParentRoute, path: "/about", component: About })
                    let Some(o) = args(n).first().copied() else {
                        return;
                    };
                    let Some((path, _)) = prop(o, "path", src).and_then(|p| str_lit(p, src)) else {
                        return;
                    };
                    let component = prop(o, "component", src)
                        .filter(|c| c.kind() == "identifier")
                        .map(|c| text(c, src).to_string());
                    pages.push((normalize_path(&path), component, line(n)));
                }
                _ => {}
            }
        });
        for (route, component, ln) in pages {
            let loc = out.loc(ln);
            out.pages.push(Page {
                route,
                component,
                loc,
                framework: Framework::TanStackRouter,
            });
            out.frameworks.insert(Framework::TanStackRouter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::file_route;

    #[test]
    fn file_routes() {
        assert_eq!(file_route("src/routes/index.tsx").as_deref(), Some("/"));
        assert_eq!(
            file_route("src/routes/posts/$postId.tsx").as_deref(),
            Some("/posts/{}")
        );
        assert_eq!(
            file_route("src/routes/posts.$postId.edit.tsx").as_deref(),
            Some("/posts/{}/edit")
        );
        assert_eq!(
            file_route("src/routes/_auth/(app)/settings.lazy.tsx").as_deref(),
            Some("/settings")
        );
        assert_eq!(file_route("src/routes/__root.tsx"), None);
        assert_eq!(file_route("src/components/x.tsx"), None);
        assert_eq!(file_route("src/myroutes/x.tsx"), None);
    }
}
