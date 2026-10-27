//! FastAPI: routes on `FastAPI()` apps and `APIRouter()` routers, router prefixes, and
//! `include_router` / `mount` chains across files.
//!
//! `extract` records, per file, each route with the router it hangs on and each mount with the
//! prefix as written. `link` joins the prefixes along the mount chain and rewrites every route's
//! `key.name` with its full path (see [`super::graph`] for the reference formats).

use std::collections::{BTreeMap, BTreeSet};

use super::common::{
    Bindings, assigned_calls, call_args, chain, eval_str, module_consts, receiver_ref, var_ref,
};
use super::graph::{Linker, Style};
use crate::http::normalize_path;
use crate::model::*;
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// Constructors of other web frameworks: a variable assigned from one is not a FastAPI router.
const OTHER_FRAMEWORKS: &[&str] = &[
    "Flask",
    "Blueprint",
    "Sanic",
    "Quart",
    "Starlette",
    "Bottle",
    "Typer",
    "FastMCP",
    "Server",
    "Application",
    "Api",
    "Namespace",
];

/// FastAPI routes, router prefixes and mounts.
pub struct FastApi;

fn compact(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Methods named by a `methods=[...]` argument. `None` inside the list means the method could
/// not be read. A missing argument gives `default`.
pub(super) fn methods_arg(
    args: &super::common::Args,
    src: &str,
    consts: &BTreeMap<String, String>,
    default: &[&str],
) -> Vec<Option<String>> {
    let Some(m) = args.keyword("methods") else {
        return default.iter().map(|d| Some(d.to_string())).collect();
    };
    if !matches!(m.kind(), "list" | "tuple" | "set") {
        return vec![None];
    }
    let mut out = Vec::new();
    let mut cur = m.walk();
    for e in m.named_children(&mut cur) {
        match eval_str(e, src, consts, None) {
            Some(s) => out.push(Some(s.to_ascii_uppercase())),
            None => return vec![None],
        }
    }
    if out.is_empty() { vec![None] } else { out }
}

impl Extractor for FastApi {
    fn name(&self) -> &'static str {
        "fastapi"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        let s = src.text;
        let root = src.tree.root_node();
        let b = Bindings::parse(root, s);
        let consts = module_consts(root, s);
        let file = src.path;
        let hint = s.contains("fastapi") || s.contains("APIRouter") || s.contains("starlette");

        // Pass 1: which variables are apps and routers, and each router's own prefix.
        let mut defined: BTreeSet<String> = BTreeSet::new();
        let mut foreign: BTreeSet<String> = BTreeSet::new();
        let mut mounts: Vec<Mount> = Vec::new();
        for (name, ctor, call) in assigned_calls(root, s) {
            if defined.contains(&name) || foreign.contains(&name) {
                continue;
            }
            let last = ctor.last().map(String::as_str).unwrap_or("");
            let id = format!("{file}:{name}");
            match last {
                "APIRouter" => {
                    let args = call_args(call, s);
                    let prefix = match args.keyword("prefix") {
                        Some(p) => eval_str(p, s, &consts, None)
                            .unwrap_or_else(|| format!("?{}", compact(text(p, s)))),
                        None => String::new(),
                    };
                    defined.insert(name);
                    mounts.push(Mount {
                        parent: None,
                        child: id,
                        prefix,
                        loc: out.loc(line(call)),
                        framework: Framework::FastApi,
                    });
                }
                "FastAPI" => {
                    defined.insert(name);
                    mounts.push(Mount {
                        parent: Some(id.clone()),
                        child: id,
                        prefix: String::new(),
                        loc: out.loc(line(call)),
                        framework: Framework::FastApi,
                    });
                }
                l if OTHER_FRAMEWORKS.contains(&l) => {
                    foreign.insert(name);
                }
                _ => {}
            }
        }

        // Pass 2: routes, include_router, mount.
        let mut endpoints: Vec<Endpoint> = Vec::new();
        let recv = |obj: Node| -> Option<String> {
            let c = chain(obj, s)?;
            receiver_ref(&c, file, &b, &defined, &foreign, hint)
        };
        for_each_node(root, |n| match n.kind() {
            "decorated_definition" => {
                let Some(def) = n.child_by_field_name("definition") else {
                    return;
                };
                if def.kind() != "function_definition" {
                    return;
                }
                let handler = super::common::func_name(def, s);
                let mut cur = n.walk();
                for d in n.children(&mut cur) {
                    if d.kind() != "decorator" {
                        continue;
                    }
                    let Some(e) = d.named_child(0).filter(|e| e.kind() == "call") else {
                        continue;
                    };
                    let Some(f) = e
                        .child_by_field_name("function")
                        .filter(|f| f.kind() == "attribute")
                    else {
                        continue;
                    };
                    let (Some(obj), Some(meth)) = (
                        f.child_by_field_name("object"),
                        f.child_by_field_name("attribute"),
                    ) else {
                        continue;
                    };
                    let meth = text(meth, s);
                    let methods: Vec<Option<String>> = if HTTP_METHODS.contains(&meth) {
                        vec![Some(meth.to_ascii_uppercase())]
                    } else if meth == "websocket" {
                        vec![None]
                    } else if meth == "api_route" || meth == "route" {
                        methods_arg(&call_args(e, s), s, &consts, &["GET"])
                    } else {
                        continue;
                    };
                    let args = call_args(e, s);
                    let Some(path) = args.get(0, "path") else {
                        continue;
                    };
                    let Some(router) = recv(obj) else { continue };
                    let path = eval_str(path, s, &consts, Some("{}")).unwrap_or_default();
                    for m in methods {
                        endpoints.push(Endpoint {
                            key: EndpointKey {
                                kind: EndpointKind::Http,
                                method: m,
                                name: normalize_path(&path),
                            },
                            handler: Some(handler.clone()),
                            router: Some(router.clone()),
                            loc: out.loc(line(d)),
                            framework: Framework::FastApi,
                        });
                    }
                }
            }
            "call" => {
                let Some(f) = n
                    .child_by_field_name("function")
                    .filter(|f| f.kind() == "attribute")
                else {
                    return;
                };
                let (Some(obj), Some(meth)) = (
                    f.child_by_field_name("object"),
                    f.child_by_field_name("attribute"),
                ) else {
                    return;
                };
                let args = call_args(n, s);
                match text(meth, s) {
                    "include_router" => {
                        let Some(parent) = recv(obj) else { return };
                        let Some(child) = args.get(0, "router") else {
                            return;
                        };
                        let child_ref = chain(child, s)
                            .and_then(|c| var_ref(&c, file, &b))
                            .unwrap_or_else(|| compact(text(child, s)));
                        let prefix = match args.keyword("prefix") {
                            Some(p) => eval_str(p, s, &consts, None)
                                .unwrap_or_else(|| format!("?{}", compact(text(p, s)))),
                            None => String::new(),
                        };
                        mounts.push(Mount {
                            parent: Some(parent),
                            child: child_ref,
                            prefix,
                            loc: out.loc(line(n)),
                            framework: Framework::FastApi,
                        });
                    }
                    "mount" => {
                        let (Some(path), Some(app)) = (args.get(0, "path"), args.get(1, "app"))
                        else {
                            return;
                        };
                        let Some(child_ref) = chain(app, s).and_then(|c| var_ref(&c, file, &b))
                        else {
                            return;
                        };
                        let Some(parent) = recv(obj) else { return };
                        let prefix = eval_str(path, s, &consts, None)
                            .unwrap_or_else(|| format!("?{}", compact(text(path, s))));
                        mounts.push(Mount {
                            parent: Some(parent),
                            child: child_ref,
                            prefix,
                            loc: out.loc(line(n)),
                            framework: Framework::FastApi,
                        });
                    }
                    m @ ("add_api_route"
                    | "add_route"
                    | "add_api_websocket_route"
                    | "add_websocket_route") => {
                        let Some(path) = args.get(0, "path") else {
                            return;
                        };
                        let Some(router) = recv(obj) else { return };
                        let handler = args
                            .get(1, "endpoint")
                            .or_else(|| args.keyword("route"))
                            .and_then(|h| chain(h, s))
                            .and_then(|c| c.last().cloned());
                        let methods = if m.contains("websocket") {
                            vec![None]
                        } else {
                            methods_arg(&args, s, &consts, &["GET"])
                        };
                        let path = eval_str(path, s, &consts, Some("{}")).unwrap_or_default();
                        for me in methods {
                            endpoints.push(Endpoint {
                                key: EndpointKey {
                                    kind: EndpointKind::Http,
                                    method: me,
                                    name: normalize_path(&path),
                                },
                                handler: handler.clone(),
                                router: Some(router.clone()),
                                loc: out.loc(line(n)),
                                framework: Framework::FastApi,
                            });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        });

        if !endpoints.is_empty() || !mounts.is_empty() {
            out.frameworks.insert(Framework::FastApi);
        }
        out.endpoints.extend(endpoints);
        out.mounts.extend(mounts);
    }

    fn link(&self, facts: &mut Facts) {
        let linker = Linker::new(facts, Framework::FastApi, Style::Path, false);
        linker.rewrite(facts, Framework::FastApi, EndpointKind::Http, false);
    }
}
