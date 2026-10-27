//! MCP tools in TypeScript: `server.tool("name", ...)`, `server.registerTool("name", ...)`, and
//! `{ name: "...", inputSchema }` entries in a `tools: [...]` listing.

use super::{TS_LANGS, args, callee, imports_from, is_fn, kids, prop, str_lit};
use crate::model::{Endpoint, EndpointKey, EndpointKind, FileFacts, Framework, Lang};
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// TS MCP tool registrations.
pub struct McpTs;

fn tool(out: &mut FileFacts, name: String, handler: Option<String>, n: Node) {
    let loc = out.loc(line(n));
    out.endpoints.push(Endpoint {
        key: EndpointKey {
            kind: EndpointKind::McpTool,
            method: None,
            name,
        },
        handler,
        router: None,
        loc,
        framework: Framework::McpTs,
    });
    out.frameworks.insert(Framework::McpTs);
}

impl Extractor for McpTs {
    fn name(&self) -> &'static str {
        "mcp_ts"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        let root = s.tree.root_node();
        let sdk = imports_from(root, src, "@modelcontextprotocol/")
            || src.contains("@modelcontextprotocol/");
        for_each_node(root, |n| match n.kind() {
            "call_expression" => {
                let Some((Some(recv), method)) = callee(n, src) else {
                    return;
                };
                if method != "tool" && method != "registerTool" {
                    return;
                }
                let recv_name = text(recv, src).to_ascii_lowercase();
                if !sdk && !recv_name.contains("server") && !recv_name.contains("mcp") {
                    return;
                }
                let a = args(n);
                let Some((name, false)) = a.first().and_then(|f| str_lit(*f, src)) else {
                    return;
                };
                let Some(last) = a.last().filter(|_| a.len() > 1) else {
                    return;
                };
                let handler = if last.kind() == "identifier" {
                    Some(text(*last, src).to_string())
                } else if is_fn(*last) {
                    // The definitions pass names an inline handler `tool:<name>`.
                    Some(format!("tool:{name}"))
                } else {
                    None
                };
                tool(out, name, handler, n);
            }
            "pair" if sdk => {
                // tools: [{ name: "add", description, inputSchema }, ...]
                let is_tools = n
                    .child_by_field_name("key")
                    .is_some_and(|k| text(k, src).trim_matches(['"', '\'']) == "tools");
                let Some(list) = n.child_by_field_name("value").filter(|_| is_tools) else {
                    return;
                };
                if list.kind() != "array" {
                    return;
                }
                for item in kids(list) {
                    let listed = prop(item, "inputSchema", src).is_some()
                        || prop(item, "description", src).is_some();
                    if let Some((name, false)) =
                        prop(item, "name", src).and_then(|v| str_lit(v, src))
                        && listed
                    {
                        tool(out, name, None, item);
                    }
                }
            }
            _ => {}
        });
    }
}
