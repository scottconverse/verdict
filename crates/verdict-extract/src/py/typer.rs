//! Typer: `@app.command()` on a `typer.Typer()` variable and `add_typer(sub, name=...)` groups.
//! A command is a `CliCommand` endpoint named as typed (`users add`). Typer names a command after
//! its function, lower-cased, with `_` turned into `-`, unless the decorator gives a name.

use std::collections::BTreeSet;

use super::common::{
    Bindings, assigned_calls, call_args, chain, eval_str, func_name, module_consts, receiver_ref,
    var_ref,
};
use super::graph::{Linker, Style};
use crate::model::*;
use crate::syntax::{for_each_node, line, text};
use crate::{Extractor, Source};

/// Typer commands and groups.
pub struct TyperCli;

const OTHER_CLI: &[&str] = &["ArgumentParser", "Group", "group", "Command"];

impl Extractor for TyperCli {
    fn name(&self) -> &'static str {
        "typer"
    }

    fn langs(&self) -> &'static [Lang] {
        &[Lang::Python]
    }

    fn extract(&self, src: &Source, out: &mut FileFacts) {
        let s = src.text;
        // The app may be imported from a file that does not import typer itself.
        let mentions = s.contains("typer") || s.contains("Typer");
        if !mentions
            && !s.contains(".command(")
            && !s.contains(".add_typer(")
            && !s.contains("__main__")
        {
            return;
        }
        let root = src.tree.root_node();
        let b = Bindings::parse(root, s);
        let consts = module_consts(root, s);
        let file = src.path;
        let hint = s.contains("typer") || s.contains("Typer");

        let mut defined: BTreeSet<String> = BTreeSet::new();
        let mut foreign: BTreeSet<String> = BTreeSet::new();
        let mut mounts: Vec<Mount> = Vec::new();
        let mut typers: Vec<(String, u32)> = Vec::new();
        for (name, ctor, call) in assigned_calls(root, s) {
            if defined.contains(&name) || foreign.contains(&name) {
                continue;
            }
            let last = ctor.last().map(String::as_str).unwrap_or("");
            if last == "Typer" {
                let args = call_args(call, s);
                let group = match args.keyword("name") {
                    Some(n) => {
                        eval_str(n, s, &consts, None).unwrap_or_else(|| format!("?{}", text(n, s)))
                    }
                    None => String::new(),
                };
                let id = format!("{file}:{name}");
                defined.insert(name);
                mounts.push(Mount {
                    parent: None,
                    child: id.clone(),
                    prefix: group,
                    loc: out.loc(line(call)),
                    framework: Framework::Typer,
                });
                typers.push((id, line(call)));
            } else if OTHER_CLI.contains(&last) {
                foreign.insert(name);
            }
        }

        let mut endpoints: Vec<Endpoint> = Vec::new();
        let mut invoked: Vec<(String, u32)> = Vec::new();
        let mut has_children: Vec<String> = Vec::new();
        for_each_node(root, |n| match n.kind() {
            "decorated_definition" => {
                let Some(def) = n.child_by_field_name("definition") else {
                    return;
                };
                if def.kind() != "function_definition" {
                    return;
                }
                let fname = func_name(def, s);
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
                    if text(meth, s) != "command" {
                        continue;
                    }
                    let Some(oc) = chain(obj, s) else { continue };
                    let Some(router) = receiver_ref(&oc, file, &b, &defined, &foreign, hint) else {
                        continue;
                    };
                    let name = match call.map(|c| call_args(c, s)).and_then(|a| a.get(0, "name")) {
                        Some(n) => {
                            eval_str(n, s, &consts, Some("{}")).unwrap_or_else(|| "{}".into())
                        }
                        None => plain.to_lowercase().replace('_', "-"),
                    };
                    endpoints.push(Endpoint {
                        key: EndpointKey {
                            kind: EndpointKind::CliCommand,
                            method: None,
                            name,
                        },
                        handler: Some(fname.clone()),
                        router: Some(router),
                        loc: out.loc(line(d)),
                        framework: Framework::Typer,
                    });
                }
            }
            "call" => {
                note_invocation(n, s, file, &b, &defined, &foreign, &mut invoked);
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
                if text(meth, s) != "add_typer" {
                    return;
                }
                let args = call_args(n, s);
                let Some(child) = args.get(0, "typer_instance") else {
                    return;
                };
                let Some(child_ref) = chain(child, s).and_then(|c| var_ref(&c, file, &b)) else {
                    return;
                };
                let Some(oc) = chain(obj, s) else { return };
                let Some(parent) = receiver_ref(&oc, file, &b, &defined, &foreign, hint) else {
                    return;
                };
                has_children.push(parent.clone());
                let prefix = match args.keyword("name") {
                    Some(nm) => eval_str(nm, s, &consts, None)
                        .unwrap_or_else(|| format!("?{}", text(nm, s))),
                    None => String::new(),
                };
                mounts.push(Mount {
                    parent: Some(parent),
                    child: child_ref,
                    prefix,
                    loc: out.loc(line(n)),
                    framework: Framework::Typer,
                });
            }
            _ => {}
        });

        // A Typer that is run, or that has groups added to it, is a CLI entry point.
        for (id, l) in typers {
            if invoked.iter().any(|(r, _)| *r == id) || has_children.contains(&id) {
                mounts.push(Mount {
                    parent: Some(id.clone()),
                    child: id,
                    prefix: String::new(),
                    loc: out.loc(l),
                    framework: Framework::Typer,
                });
            }
        }
        for (r, l) in invoked {
            if !r.rsplit_once(':').is_some_and(|(l, _)| l == file) && r.contains(':') {
                mounts.push(Mount {
                    parent: Some(r.clone()),
                    child: r,
                    prefix: String::new(),
                    loc: out.loc(l),
                    framework: Framework::Typer,
                });
            }
        }

        if !endpoints.is_empty() || !mounts.is_empty() {
            out.frameworks.insert(Framework::Typer);
        }
        out.endpoints.extend(endpoints);
        out.mounts.extend(mounts);
    }

    fn link(&self, facts: &mut Facts) {
        let linker = Linker::new(facts, Framework::Typer, Style::Cli, true);
        linker.rewrite(facts, Framework::Typer, EndpointKind::CliCommand, true);
    }
}

/// Notes `app()` and `typer.run(app)`: the named group is being run, so it is the entry point.
/// A name imported from another module counts only in a file that has a `__main__` guard or is
/// itself `__main__.py`, so ordinary imported function calls are not taken for entry points.
fn note_invocation(
    call: tree_sitter::Node,
    s: &str,
    file: &str,
    b: &Bindings,
    defined: &BTreeSet<String>,
    foreign: &BTreeSet<String>,
    invoked: &mut Vec<(String, u32)>,
) {
    let Some(f) = call.child_by_field_name("function") else {
        return;
    };
    let main_file = s.contains("__main__");
    let mut consider = |parts: &[String]| {
        if let Some(r) = receiver_ref(parts, file, b, defined, foreign, false) {
            let local = r.rsplit_once(':').is_some_and(|(l, _)| l == file);
            if local || main_file {
                invoked.push((r, line(call)));
            }
        }
    };
    if f.kind() == "identifier" {
        consider(&[text(f, s).to_string()]);
        return;
    }
    let Some(c) = chain(f, s) else { return };
    if c.last().map(String::as_str) == Some("run")
        && b.head_from(&c, "typer")
        && let Some(arg) = call_args(call, s).pos.first()
        && let Some(ac) = chain(*arg, s)
    {
        consider(&ac);
    }
}
