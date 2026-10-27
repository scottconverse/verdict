//! Python MCP tools: `@mcp.tool()` on a `FastMCP(...)` or `Server(...)` variable (the official SDK
//! and FastMCP), `add_tool(fn)`, and `Tool(name="...")` listings. Each is an `McpTool` endpoint
//! named by the tool's name: the argument when given, else the function name.

use std::collections::BTreeSet;

use super::common::{
    Bindings, assigned_calls, call_args, call_chain, chain, eval_str, func_name, module_consts,
    receiver_ref,
};
use super::graph::{Linker, Style};
use crate::model::*;
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};

/// MCP tool registrations in Python.
pub struct McpPython;

const SERVER_CTORS: &[&str] = &[
    "FastMCP",
    "Server",
    "MCPServer",
    "McpServer",
    "FastMCPServer",
];

fn looks_like_server(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.contains("mcp") || l.contains("server")
}

impl Extractor for McpPython {
    fn name(&self) -> &'static str {
        "mcp_python"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        let s = src.text;
        if !s.contains("tool") && !s.contains("Tool") {
            return;
        }
        let root = src.tree.root_node();
        let b = Bindings::parse(root, s);
        let consts = module_consts(root, s);
        let file = src.path;

        let mut defined: BTreeSet<String> = BTreeSet::new();
        let mut foreign: BTreeSet<String> = BTreeSet::new();
        let mut mounts: Vec<Mount> = Vec::new();
        for (name, ctor, call) in assigned_calls(root, s) {
            if defined.contains(&name) || foreign.contains(&name) {
                continue;
            }
            let last = ctor.last().map(String::as_str).unwrap_or("");
            if SERVER_CTORS.contains(&last) {
                let id = format!("{file}:{name}");
                defined.insert(name);
                mounts.push(Mount {
                    parent: Some(id.clone()),
                    child: id,
                    prefix: String::new(),
                    loc: out.loc(line(call)),
                    framework: Framework::McpPython,
                });
            } else if !looks_like_server(&name) {
                // `agent = Agent(...)` has a `.tool` decorator too; it is not an MCP server.
                foreign.insert(name);
            }
        }

        let tool_from_sdk = |call: tree_sitter::Node| -> bool {
            let Some(c) = call_chain(call, s) else {
                return false;
            };
            if c.last().map(String::as_str) != Some("Tool") {
                return false;
            }
            if c.len() == 1 {
                return ["mcp", "fastmcp"]
                    .iter()
                    .any(|p| b.imported_name(&c[0], p).as_deref() == Some("Tool"));
            }
            b.head_from(&c, "mcp") || b.head_from(&c, "fastmcp")
        };

        let mut endpoints: Vec<Endpoint> = Vec::new();
        let recv = |obj: tree_sitter::Node| -> Option<(Option<String>, String)> {
            let c = chain(obj, s)?;
            let hint = c.len() == 1 && looks_like_server(&c[0]);
            let r = receiver_ref(&c, file, &b, &defined, &foreign, hint)?;
            let local = c.len() == 1 && defined.contains(&c[0]);
            Some((if local { None } else { Some(r.clone()) }, r))
        };
        for_each_node(root, |n| match n.kind() {
            "decorated_definition" => {
                let Some(def) = n.child_by_field_name("definition") else {
                    return;
                };
                if def.kind() != "function_definition" {
                    return;
                }
                let handler = func_name(def, s);
                let plain = def
                    .child_by_field_name("name")
                    .map(|x| text(x, s).to_string())
                    .unwrap_or_default();
                let mut cur = n.walk();
                for d in n.children(&mut cur) {
                    if d.kind() != "decorator" {
                        continue;
                    }
                    let Some(e) = d.named_child(0) else { continue };
                    let (callee, call) = if e.kind() == "call" {
                        (e.child_by_field_name("function"), Some(e))
                    } else {
                        (Some(e), None)
                    };
                    let Some(f) = callee.filter(|f| f.kind() == "attribute") else {
                        continue;
                    };
                    let (Some(obj), Some(meth)) = (
                        f.child_by_field_name("object"),
                        f.child_by_field_name("attribute"),
                    ) else {
                        continue;
                    };
                    if text(meth, s) != "tool" {
                        continue;
                    }
                    let Some((router, _)) = recv(obj) else {
                        continue;
                    };
                    let name = match call.map(|c| call_args(c, s)).and_then(|a| a.get(0, "name")) {
                        Some(nm) => {
                            eval_str(nm, s, &consts, Some("{}")).unwrap_or_else(|| "{}".into())
                        }
                        None => plain.clone(),
                    };
                    endpoints.push(Endpoint {
                        key: EndpointKey {
                            kind: EndpointKind::McpTool,
                            method: None,
                            name,
                        },
                        handler: Some(handler.clone()),
                        router,
                        loc: out.loc(line(d)),
                        framework: Framework::McpPython,
                    });
                }
            }
            "call" => {
                if tool_from_sdk(n) {
                    let args = call_args(n, s);
                    if let Some(nm) = args.keyword("name") {
                        let name =
                            eval_str(nm, s, &consts, Some("{}")).unwrap_or_else(|| "{}".into());
                        endpoints.push(Endpoint {
                            key: EndpointKey {
                                kind: EndpointKind::McpTool,
                                method: None,
                                name,
                            },
                            handler: None,
                            router: None,
                            loc: out.loc(line(n)),
                            framework: Framework::McpPython,
                        });
                    }
                    return;
                }
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
                if text(meth, s) != "add_tool" {
                    return;
                }
                let Some((router, _)) = recv(obj) else { return };
                let args = call_args(n, s);
                let handler = args
                    .get(0, "fn")
                    .and_then(|h| chain(h, s))
                    .and_then(|c| c.last().cloned());
                let name = match args.keyword("name") {
                    Some(nm) => eval_str(nm, s, &consts, Some("{}")).unwrap_or_else(|| "{}".into()),
                    None => match &handler {
                        Some(h) => h.clone(),
                        None => return,
                    },
                };
                endpoints.push(Endpoint {
                    key: EndpointKey {
                        kind: EndpointKind::McpTool,
                        method: None,
                        name,
                    },
                    handler,
                    router,
                    loc: out.loc(line(n)),
                    framework: Framework::McpPython,
                });
            }
            _ => {}
        });

        if !endpoints.is_empty() || !mounts.is_empty() {
            out.frameworks.insert(Framework::McpPython);
        }
        out.endpoints.extend(endpoints);
        out.mounts.extend(mounts);
    }

    fn link(&self, facts: &mut Facts) {
        let linker = Linker::new(facts, Framework::McpPython, Style::Path, false);
        for f in facts.files.iter_mut() {
            let path = f.path.clone();
            f.endpoints.retain_mut(|ep| {
                if ep.framework != Framework::McpPython {
                    return true;
                }
                let Some(r) = ep.router.clone() else {
                    return true;
                };
                let id = linker.resolve(&path, &r);
                if linker.kind(&id) == super::graph::Root::App {
                    ep.router = None;
                    return true;
                }
                // Imported from a file we read that builds no MCP server under a server-like
                // name (`agent.tool` from pydantic-ai, say): not an MCP tool.
                let imported = !r.rsplit_once(':').is_some_and(|(l, _)| l.ends_with(".py"));
                let var = id.rsplit_once(':').map(|(_, v)| v).unwrap_or("");
                let in_tree = id.rsplit_once(':').is_some_and(|(l, _)| l.ends_with(".py"));
                if imported && in_tree && !looks_like_server(var) {
                    return false;
                }
                ep.router = Some(id);
                true
            });
        }
    }
}
