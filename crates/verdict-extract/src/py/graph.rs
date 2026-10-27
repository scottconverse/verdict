//! The cross-file half of FastAPI, Typer and Python MCP: turn the mounts each file recorded into
//! full paths (or full command names) for every endpoint.
//!
//! A router, app or server is named `<file>:<variable>` (for example `app/routers/users.py:router`).
//! Inside one file a name that was imported is written `<module>:<variable>` with the module as
//! the import wrote it (`app.routers.users`, `.users`); this pass turns it into a file id by
//! looking the module up among the scanned files. Nothing is guessed: a module that is not in
//! the tree, or that two files could both be, stays as written.
//!
//! Mount records use three shapes:
//! - `parent: None` is a router's (or Typer group's) own `prefix=` / `name=`.
//! - `parent == Some(child)` marks the id as an app root (a `FastAPI()`, a `Typer()`, a server).
//! - anything else is one router mounted on another under a prefix. A prefix that starts with `?`
//!   could not be read statically.

use std::collections::{BTreeMap, BTreeSet};

use crate::http::join;
use crate::model::*;

/// How prefixes combine.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// URL paths: `/api` + `/users` + `/{id}`.
    Path,
    /// Command names typed with spaces: `db users add`.
    Cli,
}

/// What a chain of mounts ends at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Root {
    /// An app: everything under it is served.
    App,
    /// A router nothing mounts: its routes are not reachable from any app we can see.
    Router,
    /// A name we know nothing about (a parameter, a third-party import).
    Unknown,
}

/// Finds the file a module name means.
pub struct Resolver {
    files: BTreeSet<String>,
    imports: BTreeMap<String, Vec<(String, Vec<String>)>>,
}

fn pj(base: &str, rel: &str) -> String {
    match (base.is_empty(), rel.is_empty()) {
        (true, _) => rel.to_string(),
        (_, true) => base.to_string(),
        _ => format!("{base}/{rel}"),
    }
}

fn common_dirs(a: &str, b: &str) -> usize {
    let da: Vec<&str> = a.split('/').collect();
    let db: Vec<&str> = b.split('/').collect();
    da.iter()
        .take(da.len().saturating_sub(1))
        .zip(db.iter().take(db.len().saturating_sub(1)))
        .take_while(|(x, y)| x == y)
        .count()
}

impl Resolver {
    fn new(facts: &Facts) -> Resolver {
        let mut files = BTreeSet::new();
        let mut imports = BTreeMap::new();
        for f in facts.files.iter().filter(|f| f.lang == Lang::Python) {
            files.insert(f.path.clone());
            imports.insert(
                f.path.clone(),
                f.imports
                    .iter()
                    .map(|i| (i.module.clone(), i.names.clone()))
                    .collect(),
            );
        }
        Resolver { files, imports }
    }

    /// The file a module means when imported from `from_file`.
    pub fn module_to_file(&self, from_file: &str, module: &str) -> Option<String> {
        if module.starts_with('.') {
            let dots = module.chars().take_while(|c| *c == '.').count();
            let rest = &module[dots..];
            let mut dir: Vec<&str> = from_file.split('/').collect();
            dir.pop();
            for _ in 1..dots {
                dir.pop()?;
            }
            let base = dir.join("/");
            let rel = rest.replace('.', "/");
            let target = pj(&base, &rel);
            let plain = format!("{target}.py");
            let pkg = pj(&target, "__init__.py");
            if !rest.is_empty() && self.files.contains(&plain) {
                return Some(plain);
            }
            return self.files.contains(&pkg).then_some(pkg);
        }
        let rel = module.replace('.', "/");
        let plain = format!("{rel}.py");
        let pkg = format!("{rel}/__init__.py");
        let mut found: Vec<&String> = self
            .files
            .iter()
            .filter(|f| {
                [&plain, &pkg]
                    .iter()
                    .any(|s| *f == *s || f.ends_with(&format!("/{s}")))
            })
            .collect();
        match found.len() {
            0 => None,
            1 => Some(found[0].clone()),
            _ => {
                found.sort_by_key(|f| std::cmp::Reverse(common_dirs(f, from_file)));
                let best = common_dirs(found[0], from_file);
                (common_dirs(found[1], from_file) < best).then(|| found[0].clone())
            }
        }
    }

    fn resolve_var(
        &self,
        file: &str,
        var: &str,
        depth: usize,
        exists: &dyn Fn(&str) -> bool,
    ) -> String {
        let id = format!("{file}:{var}");
        if exists(&id) || depth >= 8 {
            return id;
        }
        if let Some(list) = self.imports.get(file) {
            for (module, names) in list {
                if names.iter().any(|n| n == var)
                    && let Some(f2) = self.module_to_file(file, module)
                {
                    let id2 = self.resolve_var(&f2, var, depth + 1, exists);
                    if exists(&id2) {
                        return id2;
                    }
                }
            }
        }
        id
    }

    /// Turn a reference into a file id when its module is in the tree; otherwise leave it as
    /// written.
    pub fn resolve(&self, from_file: &str, r: &str, exists: &dyn Fn(&str) -> bool) -> String {
        let Some((left, var)) = r.rsplit_once(':') else {
            return r.to_string();
        };
        if left.ends_with(".py") {
            return r.to_string();
        }
        match self.module_to_file(from_file, left) {
            Some(file) => self.resolve_var(&file, var, 0, exists),
            None => r.to_string(),
        }
    }
}

/// Mounts of one framework, resolved across files.
pub struct Linker {
    resolver: Resolver,
    style: Style,
    kinds: BTreeMap<String, Root>,
    own: BTreeMap<String, Option<String>>,
    edges: BTreeMap<String, Vec<(String, Option<String>)>>,
    known: BTreeSet<String>,
}

fn readable(prefix: &str) -> Option<String> {
    (!prefix.starts_with('?')).then(|| prefix.to_string())
}

fn join_cli(a: &str, b: &str) -> String {
    match (a.trim().is_empty(), b.trim().is_empty()) {
        (true, _) => b.trim().to_string(),
        (_, true) => a.trim().to_string(),
        _ => format!("{} {}", a.trim(), b.trim()),
    }
}

impl Linker {
    /// Read and resolve every mount of `fw` in `facts` (rewriting each mount's `child` and
    /// `parent` to file ids where they can be).
    pub fn new(facts: &mut Facts, fw: Framework, style: Style, promote_sole_root: bool) -> Linker {
        let resolver = Resolver::new(facts);
        let mut known: BTreeSet<String> = BTreeSet::new();
        for m in facts.files.iter().flat_map(|f| f.mounts.iter()) {
            if m.framework != fw {
                continue;
            }
            for id in m.parent.iter().chain(std::iter::once(&m.child)) {
                if id.rsplit_once(':').is_some_and(|(l, _)| l.ends_with(".py")) {
                    known.insert(id.clone());
                }
            }
        }
        {
            let exists = |id: &str| known.contains(id);
            for f in facts.files.iter_mut() {
                let path = f.path.clone();
                for m in f.mounts.iter_mut().filter(|m| m.framework == fw) {
                    let was_self = m.parent.as_ref() == Some(&m.child);
                    m.child = resolver.resolve(&path, &m.child, &exists);
                    if was_self {
                        m.parent = Some(m.child.clone());
                    } else if let Some(p) = &m.parent {
                        m.parent = Some(resolver.resolve(&path, p, &exists));
                    }
                }
            }
        }
        let mut kinds: BTreeMap<String, Root> = BTreeMap::new();
        let mut own: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut edges: BTreeMap<String, Vec<(String, Option<String>)>> = BTreeMap::new();
        for m in facts.files.iter().flat_map(|f| f.mounts.iter()) {
            if m.framework != fw {
                continue;
            }
            match &m.parent {
                None => {
                    own.entry(m.child.clone())
                        .or_insert_with(|| readable(&m.prefix));
                    kinds.entry(m.child.clone()).or_insert(Root::Router);
                }
                Some(p) if *p == m.child => {
                    kinds.insert(m.child.clone(), Root::App);
                }
                Some(p) => edges
                    .entry(m.child.clone())
                    .or_default()
                    .push((p.clone(), readable(&m.prefix))),
            }
        }
        for v in edges.values_mut() {
            v.sort();
            v.dedup();
        }
        if promote_sole_root && !kinds.values().any(|k| *k == Root::App) {
            // No entry point is marked. With exactly one group nothing mounts, that is the app.
            let roots: Vec<String> = kinds
                .keys()
                .filter(|id| !edges.contains_key(*id))
                .cloned()
                .collect();
            if let [only] = roots.as_slice() {
                kinds.insert(only.clone(), Root::App);
            }
        }
        Linker {
            resolver,
            style,
            kinds,
            own,
            edges,
            known,
        }
    }

    /// Resolve a reference written in `file`.
    pub fn resolve(&self, file: &str, r: &str) -> String {
        self.resolver
            .resolve(file, r, &|id| self.known.contains(id))
    }

    /// What kind of root an id is.
    pub fn kind(&self, id: &str) -> Root {
        self.kinds.get(id).copied().unwrap_or(Root::Unknown)
    }

    fn join(&self, a: &str, b: &str) -> String {
        match self.style {
            Style::Path => join(a, b),
            Style::Cli => join_cli(a, b),
        }
    }

    fn piece(&self, edge: &Option<String>, own: &Option<String>) -> Option<String> {
        match self.style {
            Style::Path => Some(join(edge.as_ref()?, own.as_ref()?)),
            Style::Cli => {
                let e = edge.as_ref()?;
                if e.trim().is_empty() {
                    own.clone()
                } else {
                    Some(e.clone())
                }
            }
        }
    }

    /// Every way the routes declared on `id` are reached: the full prefix (or `None` when a
    /// prefix on the way could not be read) and what the chain ends at.
    pub fn reach(&self, id: &str) -> Vec<(Option<String>, Root)> {
        self.compute(id, &mut Vec::new())
    }

    fn compute(&self, id: &str, visiting: &mut Vec<String>) -> Vec<(Option<String>, Root)> {
        if visiting.iter().any(|v| v == id) || visiting.len() > 32 {
            return vec![(None, Root::Unknown)];
        }
        let kind = self.kind(id);
        let own: Option<String> = match self.own.get(id) {
            Some(o) => o.clone(),
            None => Some(String::new()),
        };
        let Some(parents) = self.edges.get(id) else {
            let root_own = if kind == Root::App {
                Some(String::new())
            } else {
                own
            };
            return vec![(root_own, kind)];
        };
        visiting.push(id.to_string());
        let mut out: Vec<(Option<String>, Root)> = Vec::new();
        'outer: for (p, edge) in parents {
            for (pp, root) in self.compute(p, visiting) {
                let piece = self.piece(edge, &own);
                let joined = match (pp, piece) {
                    (Some(a), Some(b)) => Some(self.join(&a, &b)),
                    _ => None,
                };
                if !out.contains(&(joined.clone(), root)) {
                    out.push((joined, root));
                }
                if out.len() >= 32 {
                    break 'outer;
                }
            }
        }
        visiting.pop();
        out
    }

    /// The id's own prefix, empty when it has none or it cannot be read.
    pub fn own_prefix(&self, id: &str) -> String {
        self.own.get(id).and_then(|o| o.clone()).unwrap_or_default()
    }

    /// Rewrite the endpoints of `fw`/`kind` with their full names. An endpoint reached from an
    /// app gets one copy per path with `router` cleared. An endpoint that cannot be traced to an
    /// app (router never mounted, a prefix that cannot be read, a name we know nothing about)
    /// keeps its router and the name it has on its own router.
    pub fn rewrite(&self, facts: &mut Facts, fw: Framework, kind: EndpointKind, strict: bool) {
        for f in facts.files.iter_mut() {
            let path = f.path.clone();
            let old = std::mem::take(&mut f.endpoints);
            let mut new = Vec::with_capacity(old.len());
            for ep in old {
                let Some(r) = ep
                    .router
                    .clone()
                    .filter(|_| ep.framework == fw && ep.key.kind == kind)
                else {
                    new.push(ep);
                    continue;
                };
                let id = self.resolve(&path, &r);
                let imported = !r.rsplit_once(':').is_some_and(|(l, _)| l.ends_with(".py"));
                let in_tree = id.rsplit_once(':').is_some_and(|(l, _)| l.ends_with(".py"));
                if strict
                    && imported
                    && in_tree
                    && self.kind(&id) == Root::Unknown
                    && !self.edges.contains_key(&id)
                {
                    // Imported from a file we read, and that file never makes one of these:
                    // the name belongs to some other library.
                    continue;
                }
                let results = self.reach(&id);
                let mut names: BTreeSet<String> = BTreeSet::new();
                let mut all = true;
                for (p, root) in &results {
                    match (p, root) {
                        (Some(p), Root::App) => {
                            names.insert(self.join(p, &ep.key.name));
                        }
                        _ => all = false,
                    }
                }
                for name in names {
                    let mut e = ep.clone();
                    e.key.name = name;
                    e.router = None;
                    new.push(e);
                }
                if !all {
                    let mut e = ep.clone();
                    e.key.name = self.join(&self.own_prefix(&id), &ep.key.name);
                    e.router = Some(id);
                    new.push(e);
                }
            }
            f.endpoints = new;
        }
    }
}
