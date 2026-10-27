//! React controls: buttons (`onClick`), forms (`onSubmit` / `action`) and links (`<Link to>`,
//! `<a href="/...">`), with their literal labels and the handler each one calls.
//!
//! Handler names as recorded in [`UiAction::handler`]:
//! - a function name (`save`), or the single function an inline arrow calls;
//! - `(inline)` for an inline arrow that does more than call one function;
//! - `(form submit)` for a button with no `onClick` that submits its form;
//! - `(props)` when the element spreads props (`{...props}`), so a handler may come from there;
//! - for a link, the target path as written (or `(dynamic)` when it is computed).
//!
//! A control with none of these has no handler (`None`).

use super::{TS_LANGS, args, callee, enclosing, is_fn, kids, str_lit};
use crate::http::normalize_path;
use crate::model::{Call, EndpointKey, EndpointKind, FileFacts, Framework, Lang, UiAction, UiKind};
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};
use tree_sitter::Node;

/// React buttons, forms and links.
pub struct React;

struct Attrs<'t> {
    pairs: Vec<(String, Option<Node<'t>>)>,
    spread: bool,
}

impl<'t> Attrs<'t> {
    fn read(open: Node<'t>, src: &str) -> Self {
        let mut pairs = Vec::new();
        let mut spread = false;
        for a in kids(open) {
            match a.kind() {
                "jsx_attribute" => {
                    let parts = kids(a);
                    if let Some(name) = parts.first() {
                        pairs.push((text(*name, src).to_string(), parts.get(1).copied()));
                    }
                }
                "jsx_expression" => spread = true,
                _ => {}
            }
        }
        Attrs { pairs, spread }
    }

    fn get(&self, name: &str) -> Option<Option<Node<'t>>> {
        self.pairs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
    }
}

/// The function an event attribute's value calls.
fn handler_of(v: Node, src: &str) -> String {
    if let Some((s, _)) = str_lit(v, src) {
        return s;
    }
    let inner = if v.kind() == "jsx_expression" {
        match kids(v).first() {
            Some(i) => *i,
            None => return "(inline)".into(),
        }
    } else {
        v
    };
    match inner.kind() {
        "identifier" => text(inner, src).to_string(),
        "member_expression" => inner
            .child_by_field_name("property")
            .map(|p| text(p, src).to_string())
            .unwrap_or_else(|| "(inline)".into()),
        "call_expression" => call_handler(inner, src),
        _ if is_fn(inner) => {
            let Some(body) = inner.child_by_field_name("body") else {
                return "(inline)".into();
            };
            let call = match body.kind() {
                "call_expression" => Some(body),
                "await_expression" => kids(body).first().copied(),
                "statement_block" => match kids(body).as_slice() {
                    [one] if one.kind() == "expression_statement" => {
                        kids(*one).first().copied().map(|e| {
                            if e.kind() == "await_expression" {
                                kids(e).first().copied().unwrap_or(e)
                            } else {
                                e
                            }
                        })
                    }
                    _ => None,
                },
                _ => None,
            };
            match call.filter(|c| c.kind() == "call_expression") {
                Some(c) => call_handler(c, src),
                None => "(inline)".into(),
            }
        }
        _ => "(inline)".into(),
    }
}

/// `save(id)` → `save`; `form.handleSubmit(save)` → `save`.
fn call_handler(c: Node, src: &str) -> String {
    match callee(c, src) {
        Some((_, "handleSubmit")) => args(c)
            .first()
            .filter(|a| a.kind() == "identifier")
            .map(|a| text(*a, src).to_string())
            .unwrap_or_else(|| "handleSubmit".into()),
        Some((_, name)) => name.to_string(),
        None => "(inline)".into(),
    }
}

/// The literal text inside an element, whitespace collapsed.
fn label_of(el: Node, src: &str) -> Option<String> {
    let mut parts = Vec::new();
    for k in kids(el) {
        match k.kind() {
            "jsx_text" => parts.push(text(k, src).to_string()),
            "jsx_expression" => {
                if let Some((s, false)) = kids(k).first().and_then(|i| str_lit(*i, src)) {
                    parts.push(s);
                }
            }
            "jsx_element" => {
                if let Some(l) = label_of(k, src) {
                    parts.push(l);
                }
            }
            _ => {}
        }
    }
    let joined = parts.join(" ");
    let label = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    (!label.is_empty()).then_some(label)
}

fn inside_form(n: Node, src: &str) -> bool {
    let mut cur = n.parent();
    while let Some(c) = cur {
        if c.kind() == "jsx_element"
            && let Some(open) = c.child_by_field_name("open_tag")
            && let Some(name) = open.child_by_field_name("name")
            && matches!(text(name, src), "form" | "Form")
        {
            return true;
        }
        cur = c.parent();
    }
    false
}

impl Extractor for React {
    fn name(&self) -> &'static str {
        "react"
    }

    fn langs(&self) -> &'static [Lang] {
        TS_LANGS
    }

    fn extract(&self, s: &Source, out: &mut FileFacts) {
        let src = s.text;
        let mut found: Vec<(UiAction, Option<(String, String, bool)>)> = Vec::new();
        for_each_node(s.tree.root_node(), |n| {
            let (open, el) = match n.kind() {
                "jsx_element" => match n.child_by_field_name("open_tag") {
                    Some(o) => (o, Some(n)),
                    None => return,
                },
                "jsx_self_closing_element" => (n, None),
                _ => return,
            };
            let Some(name) = open.child_by_field_name("name").map(|k| text(k, src)) else {
                return;
            };
            let attrs = Attrs::read(open, src);
            let attr_text = |key: &str| {
                attrs
                    .get(key)
                    .flatten()
                    .and_then(|v| str_lit(v, src))
                    .map(|(s, _)| s)
            };
            let label = el
                .and_then(|e| label_of(e, src))
                .or_else(|| attr_text("aria-label"))
                .or_else(|| attr_text("title"));
            let props = || attrs.spread.then(|| "(props)".to_string());
            let mut action_call = None;
            let (kind, handler) = if name == "button" || name.ends_with("Button") {
                let h = attrs
                    .get("onClick")
                    .flatten()
                    .map(|v| handler_of(v, src))
                    .or_else(|| {
                        (attr_text("type").as_deref() == Some("submit") || inside_form(n, src))
                            .then(|| "(form submit)".to_string())
                    })
                    .or_else(props);
                (UiKind::Button, h)
            } else if name == "form" || name == "Form" {
                let action = attrs.get("action").flatten();
                if let Some(a) = action
                    && let Some((url, dynamic)) = str_lit(a, src)
                    && url.starts_with('/')
                {
                    let method = attr_text("method")
                        .map(|m| m.to_ascii_uppercase())
                        .unwrap_or_else(|| "POST".into());
                    action_call = Some((method, url, dynamic));
                }
                let h = attrs
                    .get("onSubmit")
                    .flatten()
                    .map(|v| handler_of(v, src))
                    .or_else(|| action.map(|a| handler_of(a, src)))
                    .or_else(props);
                (UiKind::Form, h)
            } else if matches!(name, "Link" | "NavLink") || name == "a" {
                let key = if name == "a" { "href" } else { "to" };
                let Some(v) = attrs.get(key).flatten() else {
                    return;
                };
                let target = match str_lit(v, src).or_else(|| {
                    kids(v)
                        .first()
                        .filter(|_| v.kind() == "jsx_expression")
                        .and_then(|i| str_lit(*i, src))
                }) {
                    Some((t, _)) => t,
                    None => "(dynamic)".into(),
                };
                // Only links inside this app: `<a href="https://...">` leaves it.
                if name == "a" && !target.starts_with('/') {
                    return;
                }
                (UiKind::Link, Some(target))
            } else {
                return;
            };
            let ui = UiAction {
                kind,
                label,
                handler,
                component: enclosing(n, src),
                loc: out.loc(line(n)),
                framework: Framework::React,
            };
            found.push((ui, action_call));
        });
        for (ui, action_call) in found {
            if let Some((method, url, dynamic)) = action_call {
                out.calls.push(Call {
                    target: EndpointKey {
                        kind: EndpointKind::Http,
                        method: Some(method),
                        name: normalize_path(&url),
                    },
                    enclosing: ui.component.clone(),
                    dynamic,
                    loc: ui.loc.clone(),
                    framework: Framework::React,
                });
            }
            out.ui.push(ui);
            out.frameworks.insert(Framework::React);
        }
    }
}
