//! HTTP calls from TS/JS: `fetch`, axios-style clients and openapi-typescript clients, plus the
//! routes an openapi-typescript `paths` schema declares.

use super::{HTTP_VERBS, TS_LANGS, args, callee, enclosing, is_fn, key_name, kids, prop, str_lit};
use crate::http::normalize_path;
use crate::model::{Call, Endpoint, EndpointKey, EndpointKind, FileFacts, Framework, Lang};
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// `fetch(...)` and axios-style `client.get("/x")` calls.
pub struct Fetch;

/// openapi-typescript `client.GET("/x")` calls and the `paths` schema's routes.
pub struct OpenapiTs;

/// True for a URL this project could serve: a path, a template whose first part is a base URL
/// expression, or an absolute URL on localhost. A call to another host is not this project's
/// route (and the fact model has no host field to keep it apart), so it is left out.
pub(crate) fn local_url(url: &str) -> bool {
    if let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
    {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        return matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]");
    }
    url.starts_with('/') || url.starts_with("${")
}

fn push_call(
    out: &mut FileFacts,
    n: Node,
    src: &str,
    method: Option<String>,
    url: &str,
    dynamic: bool,
    framework: Framework,
) {
    let loc = out.loc(line(n));
    out.calls.push(Call {
        target: EndpointKey {
            kind: EndpointKind::Http,
            method,
            name: normalize_path(url),
        },
        enclosing: enclosing(n, src),
        dynamic,
        loc,
        framework,
    });
    out.frameworks.insert(framework);
}

/// The receiver names an axios-style client is usually bound to.
fn looks_like_client(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l == "axios"
        || l == "ky"
        || l == "$fetch"
        || l.ends_with("api")
        || l.ends_with("client")
        || l.ends_with("http")
        || l.ends_with("fetcher")
        || l == "request"
        || l == "instance"
}

impl Extractor for Fetch {
    fn name(&self) -> &'static str {
        "fetch"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        for_each_node(s.tree.root_node(), |n| {
            if n.kind() != "call_expression" {
                return;
            }
            let Some((recv, name)) = callee(n, src) else {
                return;
            };
            let a = args(n);
            let Some(first) = a.first() else { return };
            let Some((url, dynamic)) = str_lit(*first, src) else {
                return;
            };
            if !local_url(&url) {
                return;
            }
            let is_fetch = name == "fetch"
                && recv.is_none_or(|r| matches!(text(r, src), "window" | "globalThis" | "self"));
            if is_fetch {
                // fetch(url, { method: "POST" }); the default is GET.
                let method = a
                    .get(1)
                    .and_then(|o| prop(*o, "method", src))
                    .and_then(|m| str_lit(m, src))
                    .map(|(m, _)| m.to_ascii_uppercase())
                    .unwrap_or_else(|| "GET".into());
                push_call(out, n, src, Some(method), &url, dynamic, Framework::Fetch);
                return;
            }
            // axios.get("/x"), api.post(`/x/${id}`, body). A string path followed by a function
            // is a route definition (Express-style), not a call.
            let verb = name.to_ascii_lowercase();
            let Some(r) = recv else { return };
            let recv_name = match r.kind() {
                "identifier" => text(r, src),
                "member_expression" => r
                    .child_by_field_name("property")
                    .map(|p| text(p, src))
                    .unwrap_or(""),
                _ => "",
            };
            if name != verb
                || !matches!(
                    verb.as_str(),
                    "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
                )
                || !looks_like_client(recv_name)
                || a.iter().skip(1).any(|x| is_fn(*x))
            {
                return;
            }
            push_call(
                out,
                n,
                src,
                Some(verb.to_ascii_uppercase()),
                &url,
                dynamic,
                Framework::Fetch,
            );
        });
    }
}

impl Extractor for OpenapiTs {
    fn name(&self) -> &'static str {
        "openapi_ts"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        for_each_node(s.tree.root_node(), |n| match n.kind() {
            "call_expression" => {
                // client.GET("/users/{id}", { params }) — the path is the schema key, so it is
                // never dynamic.
                let Some((Some(_), verb)) = callee(n, src) else {
                    return;
                };
                if !HTTP_VERBS.contains(&verb) {
                    return;
                }
                let Some((path, dynamic)) = args(n).first().and_then(|f| str_lit(*f, src)) else {
                    return;
                };
                if !path.starts_with('/') {
                    return;
                }
                push_call(
                    out,
                    n,
                    src,
                    Some(verb.to_string()),
                    &path,
                    dynamic,
                    Framework::OpenapiTs,
                );
            }
            "interface_declaration" | "type_alias_declaration" => {
                let named_paths = n
                    .child_by_field_name("name")
                    .is_some_and(|k| text(k, src) == "paths");
                if !named_paths {
                    return;
                }
                let body = n
                    .child_by_field_name("body")
                    .or_else(|| n.child_by_field_name("value"));
                if let Some(body) = body {
                    schema_routes(body, src, out);
                }
            }
            _ => {}
        });
    }
}

/// The routes in an openapi-typescript `paths` type: each `"/path": { get: ...; put?: never }`
/// member whose verb type is not `never`.
fn schema_routes(body: Node, src: &str, out: &mut FileFacts) {
    for member in kids(body) {
        if member.kind() != "property_signature" {
            continue;
        }
        let Some(path) = member.child_by_field_name("name").map(|k| key_name(k, src)) else {
            continue;
        };
        if !path.starts_with('/') {
            continue;
        }
        let Some(ops) = member
            .child_by_field_name("type")
            .and_then(|t| kids(t).into_iter().next())
        else {
            continue;
        };
        if ops.kind() != "object_type" {
            continue;
        }
        for op in kids(ops) {
            if op.kind() != "property_signature" {
                continue;
            }
            let Some(verb) = op.child_by_field_name("name").map(|k| key_name(k, src)) else {
                continue;
            };
            let verb = verb.to_ascii_uppercase();
            if !HTTP_VERBS.contains(&verb.as_str()) {
                continue;
            }
            let never = op
                .child_by_field_name("type")
                .map(|t| text(t, src).trim_start_matches(':').trim() == "never")
                .unwrap_or(true);
            if never {
                continue;
            }
            let loc = out.loc(line(op));
            out.endpoints.push(Endpoint {
                key: EndpointKey {
                    kind: EndpointKind::Http,
                    method: Some(verb),
                    name: normalize_path(&path),
                },
                handler: None,
                router: None,
                loc,
                framework: Framework::OpenapiTs,
            });
            out.frameworks.insert(Framework::OpenapiTs);
        }
    }
}
