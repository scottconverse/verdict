//! Tauri `invoke("command", ...)` calls from the frontend.

use super::{TS_LANGS, args, callee, enclosing, imports_from, str_lit};
use crate::model::{Call, EndpointKey, EndpointKind, FileFacts, Framework, Lang};
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};

/// Tauri `invoke` calls.
pub struct TauriInvoke;

impl Extractor for TauriInvoke {
    fn name(&self) -> &'static str {
        "tauri_invoke"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        let root = s.tree.root_node();
        let imported = imports_from(root, src, "@tauri-apps/");
        for_each_node(root, |n| {
            if n.kind() != "call_expression" {
                return;
            }
            let Some((recv, name)) = callee(n, src) else {
                return;
            };
            if name != "invoke" {
                return;
            }
            // `invoke` from @tauri-apps/api, or window.__TAURI__.core.invoke.
            let tauri = match recv {
                None => imported,
                Some(r) => imported || text(r, src).contains("__TAURI"),
            };
            if !tauri {
                return;
            }
            let Some((cmd, dynamic)) = args(n).first().and_then(|a| str_lit(*a, src)) else {
                return;
            };
            // Plugin commands (`plugin:fs|read`) are served by the plugin, not this project.
            if cmd.starts_with("plugin:") || cmd.is_empty() {
                return;
            }
            let loc = out.loc(line(n));
            out.calls.push(Call {
                target: EndpointKey {
                    kind: EndpointKind::TauriCommand,
                    method: None,
                    name: cmd,
                },
                enclosing: enclosing(n, src),
                dynamic,
                loc,
                framework: Framework::Tauri,
            });
            out.frameworks.insert(Framework::Tauri);
        });
    }
}
