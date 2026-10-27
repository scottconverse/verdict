//! Helpers every Python extractor shares: call arguments, dotted names, import bindings, string
//! evaluation, scope names and a few lookups. Nothing here imports or runs the user's Python.

use std::collections::BTreeMap;
use tree_sitter::Node;

use crate::syntax::{for_each_node, text};

/// Positional and keyword arguments of a call.
pub struct Args<'t> {
    /// Positional arguments in order (splats are not included).
    pub pos: Vec<Node<'t>>,
    /// Keyword arguments in order.
    pub kw: Vec<(String, Node<'t>)>,
    /// True when the call has a `*args` or `**kwargs` splat, so positions may be off.
    pub splat: bool,
}

impl<'t> Args<'t> {
    /// The argument at `idx` or named `name`, whichever is given.
    pub fn get(&self, idx: usize, name: &str) -> Option<Node<'t>> {
        if let Some((_, v)) = self.kw.iter().find(|(k, _)| k == name) {
            return Some(*v);
        }
        self.pos.get(idx).copied()
    }

    /// The keyword argument called `name`.
    pub fn keyword(&self, name: &str) -> Option<Node<'t>> {
        self.kw.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }
}

/// The arguments of a `call` node.
pub fn call_args<'t>(call: Node<'t>, src: &str) -> Args<'t> {
    let mut out = Args {
        pos: Vec::new(),
        kw: Vec::new(),
        splat: false,
    };
    let Some(list) = call.child_by_field_name("arguments") else {
        return out;
    };
    if list.kind() != "argument_list" {
        out.pos.push(list);
        return out;
    }
    let mut cur = list.walk();
    for a in list.named_children(&mut cur) {
        match a.kind() {
            "comment" => {}
            "keyword_argument" => {
                if let (Some(n), Some(v)) = (
                    a.child_by_field_name("name"),
                    a.child_by_field_name("value"),
                ) {
                    out.kw.push((text(n, src).to_string(), v));
                }
            }
            "list_splat" | "dictionary_splat" => out.splat = true,
            _ => out.pos.push(a),
        }
    }
    out
}

/// A dotted name `a.b.c` (identifiers and attribute accesses only).
pub fn chain(n: Node, src: &str) -> Option<Vec<String>> {
    match n.kind() {
        "identifier" => Some(vec![text(n, src).to_string()]),
        "attribute" => {
            let mut c = chain(n.child_by_field_name("object")?, src)?;
            c.push(text(n.child_by_field_name("attribute")?, src).to_string());
            Some(c)
        }
        _ => None,
    }
}

/// The dotted name a call's function expression names (`a.b.c(...)` gives `a.b.c`).
pub fn call_chain(call: Node, src: &str) -> Option<Vec<String>> {
    chain(call.child_by_field_name("function")?, src)
}

/// Strip `await` and parentheses from an expression.
pub fn unwrap_expr(mut n: Node) -> Node {
    loop {
        match n.kind() {
            "await" | "parenthesized_expression" => match n.named_child(0) {
                Some(c) => n = c,
                None => return n,
            },
            _ => return n,
        }
    }
}

/// `a.b` joined with a dot; relative module prefixes (`.`, `..x`) stay as written.
pub fn join_module(module: &str, name: &str) -> String {
    if module.ends_with('.') {
        format!("{module}{name}")
    } else {
        format!("{module}.{name}")
    }
}

/// What a local name was bound to by an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// `import a.b as x` (`x` is the module `a.b`) or `import a.b` (`a` is the module `a`).
    Module(String),
    /// `from m import n [as k]`.
    From {
        /// The module as written (relative prefixes kept).
        module: String,
        /// The imported name.
        name: String,
    },
}

/// Every name an import binds in a file.
#[derive(Debug, Default)]
pub struct Bindings {
    map: BTreeMap<String, Binding>,
}

impl Bindings {
    /// Read every `import` and `from ... import` in the tree.
    pub fn parse(root: Node, src: &str) -> Bindings {
        let mut map: BTreeMap<String, Binding> = BTreeMap::new();
        for_each_node(root, |n| match n.kind() {
            "import_statement" => {
                let mut cur = n.walk();
                for item in n.children_by_field_name("name", &mut cur) {
                    match item.kind() {
                        "dotted_name" => {
                            let full = clean(text(item, src));
                            let head = full.split('.').next().unwrap_or("").to_string();
                            map.entry(head.clone()).or_insert(Binding::Module(head));
                        }
                        "aliased_import" => {
                            if let (Some(name), Some(alias)) = (
                                item.child_by_field_name("name"),
                                item.child_by_field_name("alias"),
                            ) {
                                map.insert(
                                    text(alias, src).to_string(),
                                    Binding::Module(clean(text(name, src))),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            "import_from_statement" => {
                let Some(module) = n.child_by_field_name("module_name") else {
                    return;
                };
                let module = clean(text(module, src));
                let mut cur = n.walk();
                for item in n.children_by_field_name("name", &mut cur) {
                    let (name, local) = match item.kind() {
                        "dotted_name" => {
                            let t = clean(text(item, src));
                            (t.clone(), t)
                        }
                        "aliased_import" => {
                            let (Some(nm), Some(al)) = (
                                item.child_by_field_name("name"),
                                item.child_by_field_name("alias"),
                            ) else {
                                continue;
                            };
                            (clean(text(nm, src)), text(al, src).to_string())
                        }
                        _ => continue,
                    };
                    map.insert(
                        local,
                        Binding::From {
                            module: module.clone(),
                            name,
                        },
                    );
                }
            }
            _ => {}
        });
        Bindings { map }
    }

    /// The binding of a local name.
    pub fn get(&self, name: &str) -> Option<&Binding> {
        self.map.get(name)
    }

    /// The module a dotted name starts from, when its head was imported: the absolute or
    /// relative module the name lives in, and the last segment. `a.b.c` with `import a` gives
    /// `("a.b", "c")`. A bare name from `from m import n` gives `("m", "n")`. A bare module
    /// binding gives `None` (it is a module, not a variable).
    pub fn locate(&self, parts: &[String]) -> Option<(String, String)> {
        let (head, rest) = parts.split_first()?;
        let b = self.map.get(head)?;
        match (b, rest.split_last()) {
            (Binding::From { module, name }, None) => Some((module.clone(), name.clone())),
            (Binding::Module(_), None) => None,
            (Binding::Module(m), Some((last, mid))) => {
                let mut module = m.clone();
                for seg in mid {
                    module = join_module(&module, seg);
                }
                Some((module, last.clone()))
            }
            (Binding::From { module, name }, Some((last, mid))) => {
                let mut m = join_module(module, name);
                for seg in mid {
                    m = join_module(&m, seg);
                }
                Some((m, last.clone()))
            }
        }
    }

    /// True when the dotted name's head is bound to a module (or a name from a module) whose
    /// path starts with `prefix` (`sqlalchemy`, `mcp`, ...).
    pub fn head_from(&self, parts: &[String], prefix: &str) -> bool {
        let Some(head) = parts.first() else {
            return false;
        };
        match self.map.get(head) {
            Some(Binding::Module(m)) => starts_with_module(m, prefix),
            Some(Binding::From { module, .. }) => starts_with_module(module, prefix),
            None => false,
        }
    }

    /// For a bare name bound by `from <prefix...> import n`, the original name `n`.
    pub fn imported_name(&self, local: &str, prefix: &str) -> Option<String> {
        match self.map.get(local) {
            Some(Binding::From { module, name }) if starts_with_module(module, prefix) => {
                Some(name.clone())
            }
            _ => None,
        }
    }
}

fn starts_with_module(m: &str, prefix: &str) -> bool {
    m == prefix || m.starts_with(&format!("{prefix}."))
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Module-level `NAME = "string"` constants, in the order written.
pub fn module_consts(root: Node, src: &str) -> BTreeMap<String, String> {
    let mut consts = BTreeMap::new();
    let mut cur = root.walk();
    for stmt in root.named_children(&mut cur) {
        if stmt.kind() != "expression_statement" {
            continue;
        }
        let Some(a) = stmt.named_child(0).filter(|a| a.kind() == "assignment") else {
            continue;
        };
        let (Some(left), Some(right)) = (
            a.child_by_field_name("left"),
            a.child_by_field_name("right"),
        ) else {
            continue;
        };
        if left.kind() != "identifier" {
            continue;
        }
        if let Some(v) = eval_str(right, src, &consts, None) {
            consts.insert(text(left, src).to_string(), v);
        }
    }
    consts
}

/// Evaluate a string expression statically: literals, f-strings, implicit concatenation, `+` and
/// names of module constants. An interpolation or name that cannot be read makes the result
/// `None`, unless `placeholder` is given, in which case it is written in its place.
pub fn eval_str(
    n: Node,
    src: &str,
    consts: &BTreeMap<String, String>,
    placeholder: Option<&str>,
) -> Option<String> {
    match n.kind() {
        "string" => {
            let mut out = String::new();
            let mut cur = n.walk();
            for k in n.named_children(&mut cur) {
                match k.kind() {
                    "string_content" | "escape_sequence" => out.push_str(text(k, src)),
                    "interpolation" => {
                        let inner = k
                            .child_by_field_name("expression")
                            .and_then(|e| eval_str(e, src, consts, None));
                        match (inner, placeholder) {
                            (Some(v), _) => out.push_str(&v),
                            (None, Some(p)) => out.push_str(p),
                            (None, None) => return None,
                        }
                    }
                    _ => {}
                }
            }
            Some(out)
        }
        "concatenated_string" => {
            let mut out = String::new();
            let mut cur = n.walk();
            for k in n.named_children(&mut cur) {
                if k.kind() == "comment" {
                    continue;
                }
                out.push_str(&eval_str(k, src, consts, placeholder)?);
            }
            Some(out)
        }
        "binary_operator" => {
            let op = n.child_by_field_name("operator").map(|o| text(o, src));
            if op != Some("+") {
                return fallback(placeholder);
            }
            let l = eval_str(n.child_by_field_name("left")?, src, consts, placeholder);
            let r = eval_str(n.child_by_field_name("right")?, src, consts, placeholder);
            match (l, r) {
                (Some(l), Some(r)) => Some(l + &r),
                _ => fallback(placeholder),
            }
        }
        "parenthesized_expression" => eval_str(n.named_child(0)?, src, consts, placeholder),
        "identifier" => match consts.get(text(n, src)) {
            Some(v) => Some(v.clone()),
            None => fallback(placeholder),
        },
        _ => fallback(placeholder),
    }
}

fn fallback(placeholder: Option<&str>) -> Option<String> {
    placeholder.map(str::to_string)
}

/// The innermost class or function that directly contains a definition, when that is a class.
fn class_container(def: Node) -> Option<Node> {
    let mut p = def.parent()?;
    if p.kind() == "decorated_definition" {
        p = p.parent()?;
    }
    if p.kind() != "block" {
        return None;
    }
    let c = p.parent()?;
    (c.kind() == "class_definition").then_some(c)
}

/// A class's name, qualified by the classes that directly contain it (`Outer.Inner`).
pub fn class_name(n: Node, src: &str) -> String {
    let own = n
        .child_by_field_name("name")
        .map(|x| text(x, src))
        .unwrap_or("")
        .to_string();
    match class_container(n) {
        Some(c) => format!("{}.{own}", class_name(c, src)),
        None => own,
    }
}

/// A function's name as definitions list it: `Class.method` for a method, else its own name.
pub fn func_name(n: Node, src: &str) -> String {
    let own = n
        .child_by_field_name("name")
        .map(|x| text(x, src))
        .unwrap_or("")
        .to_string();
    match class_container(n) {
        Some(c) => format!("{}.{own}", class_name(c, src)),
        None => own,
    }
}

/// True when a function directly contains no class (it is a free function, not a method).
pub fn is_method(n: Node) -> bool {
    class_container(n).is_some()
}

/// The nearest function a node sits in (the node itself does not count).
pub fn enclosing_fn(n: Node) -> Option<Node> {
    let mut cur = n.parent();
    while let Some(c) = cur {
        if c.kind() == "function_definition" {
            return Some(c);
        }
        cur = c.parent();
    }
    None
}

/// The name of the function a node sits in, as definitions list it.
pub fn enclosing(n: Node, src: &str) -> Option<String> {
    enclosing_fn(n).map(|f| func_name(f, src))
}

/// The function a node sits in, or the module root when it is not in one.
pub fn scope_of<'t>(n: Node<'t>, root: Node<'t>) -> Node<'t> {
    enclosing_fn(n).unwrap_or(root)
}

/// The right-hand side of the latest `name = ...` before `before` inside `scope`, looking only at
/// assignments that sit directly in that scope (not in a nested function).
pub fn last_assignment<'t>(
    scope: Node<'t>,
    name: &str,
    before: usize,
    src: &str,
) -> Option<Node<'t>> {
    let mut best: Option<Node<'t>> = None;
    for_each_node(scope, |n| {
        if n.kind() != "assignment" || n.start_byte() >= before {
            return;
        }
        let (Some(l), Some(r)) = (
            n.child_by_field_name("left"),
            n.child_by_field_name("right"),
        ) else {
            return;
        };
        if l.kind() != "identifier" || text(l, src) != name {
            return;
        }
        let owner = enclosing_fn(n).map(|f| f.id());
        let want = (scope.kind() == "function_definition").then(|| scope.id());
        if owner != want {
            return;
        }
        if best.is_none_or(|b| b.start_byte() < n.start_byte()) {
            best = Some(r);
        }
    });
    best
}

/// The type annotation (as written) of a parameter of `func`.
pub fn param_annotation(func: Node, name: &str, src: &str) -> Option<String> {
    let params = func.child_by_field_name("parameters")?;
    let mut cur = params.walk();
    for p in params.named_children(&mut cur) {
        let (pn, ty) = match p.kind() {
            "typed_parameter" => (p.named_child(0), p.child_by_field_name("type")),
            "typed_default_parameter" => {
                (p.child_by_field_name("name"), p.child_by_field_name("type"))
            }
            _ => continue,
        };
        if let (Some(pn), Some(ty)) = (pn, ty)
            && pn.kind() == "identifier"
            && text(pn, src) == name
        {
            return Some(text(ty, src).trim().to_string());
        }
    }
    None
}

/// True for `CamelCase` names (a class, as opposed to a function or constant).
pub fn is_camel(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && s.chars().any(|c| c.is_lowercase())
}

/// True when a decorator expression's dotted name ends with one of `names`.
pub fn has_decorator(def: Node, names: &[&str], src: &str) -> bool {
    let Some(p) = def.parent().filter(|p| p.kind() == "decorated_definition") else {
        return false;
    };
    let mut cur = p.walk();
    for d in p.children(&mut cur) {
        if d.kind() != "decorator" {
            continue;
        }
        let Some(e) = d.named_child(0) else { continue };
        let e = if e.kind() == "call" {
            match e.child_by_field_name("function") {
                Some(f) => f,
                None => continue,
            }
        } else {
            e
        };
        if let Some(c) = chain(e, src)
            && c.last().is_some_and(|l| names.contains(&l.as_str()))
        {
            return true;
        }
    }
    false
}

/// A reference to a router-like variable, for the link pass. A name defined in this file is
/// `<file>:<name>`. A name imported from another module is `<module>:<name>` with the module as
/// written (relative prefixes kept), resolved to a file later. `None` for anything that is not a
/// plain name or an imported dotted name.
pub fn var_ref(parts: &[String], file: &str, bindings: &Bindings) -> Option<String> {
    if parts.len() == 1 {
        return Some(match bindings.locate(parts) {
            Some((m, n)) => format!("{m}:{n}"),
            None if bindings.get(&parts[0]).is_some() => return None,
            None => format!("{file}:{}", parts[0]),
        });
    }
    let (m, v) = bindings.locate(parts)?;
    Some(format!("{m}:{v}"))
}

/// Every `name = Some.call(...)` in the file, in order, with the called dotted name.
pub fn assigned_calls<'t>(root: Node<'t>, src: &str) -> Vec<(String, Vec<String>, Node<'t>)> {
    let mut out = Vec::new();
    for_each_node(root, |n| {
        if n.kind() != "assignment" {
            return;
        }
        let (Some(l), Some(r)) = (
            n.child_by_field_name("left"),
            n.child_by_field_name("right"),
        ) else {
            return;
        };
        let r = unwrap_expr(r);
        if l.kind() != "identifier" || r.kind() != "call" {
            return;
        }
        if let Some(c) = call_chain(r, src) {
            out.push((text(l, src).to_string(), c, r));
        }
    });
    out
}

/// The router-like variable a decorator or call hangs on, as a reference for the link pass.
///
/// A name defined in this file is `<file>:<name>`; a name that was imported is
/// `<module>:<name>`. A name assigned from some other framework's constructor (`foreign`) is
/// `None`. A name bound by neither is a parameter or a global from somewhere we cannot see: it
/// is read only when `unbound_ok` (the file shows signs of the framework), and then stays
/// unresolved.
pub fn receiver_ref(
    parts: &[String],
    file: &str,
    bindings: &Bindings,
    defined: &std::collections::BTreeSet<String>,
    foreign: &std::collections::BTreeSet<String>,
    unbound_ok: bool,
) -> Option<String> {
    if parts.len() == 1 {
        let name = &parts[0];
        if foreign.contains(name) {
            return None;
        }
        if defined.contains(name) {
            return Some(format!("{file}:{name}"));
        }
        if let Some((m, n)) = bindings.locate(parts) {
            return Some(format!("{m}:{n}"));
        }
        if bindings.get(name).is_some() {
            return None;
        }
        return unbound_ok.then(|| format!("{file}:{name}"));
    }
    let (m, v) = bindings.locate(parts)?;
    Some(format!("{m}:{v}"))
}
