//! What the extractors find in a tree: the facts the wiring checker and `verdict_scan` read.
//!
//! This file is the contract between the per-framework extractors and their readers. Every fact
//! carries a [`Loc`] so a finding can point at file:line.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A place in the tree: repo-relative path with `/` separators, 1-based line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Loc {
    /// Path relative to the project root, `/` separators.
    pub file: String,
    /// 1-based line.
    pub line: u32,
}

/// A source language the extractors parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lang {
    /// `.py`
    Python,
    /// `.ts`, `.mts`, `.cts`
    TypeScript,
    /// `.tsx`
    Tsx,
    /// `.js`, `.mjs`, `.cjs`
    JavaScript,
    /// `.jsx`
    Jsx,
    /// `.rs`
    Rust,
}

/// A framework an extractor reads. The wiring checker reports a stack with no extractor here as
/// unverified, never clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framework {
    /// FastAPI routes, APIRouter prefixes, include_router.
    FastApi,
    /// SQLAlchemy writes (session.add/delete/merge, insert/update/delete statements).
    SqlAlchemy,
    /// asyncpg writes (execute/executemany/copy with INSERT/UPDATE/DELETE).
    Asyncpg,
    /// Alembic migrations.
    Alembic,
    /// Typer CLI commands.
    Typer,
    /// MCP tool registrations in Python.
    McpPython,
    /// TypeScript/JavaScript `fetch` calls (and axios-style clients if read).
    Fetch,
    /// openapi-typescript generated clients (`client.GET("/path")` and friends).
    OpenapiTs,
    /// TanStack Start server functions (`createServerFn`).
    TanStackStart,
    /// TanStack Router file and code routes (pages).
    TanStackRouter,
    /// Kysely writes (insertInto/updateTable/deleteFrom/replaceInto).
    Kysely,
    /// MCP tool registrations in TypeScript.
    McpTs,
    /// Tauri `invoke` calls and `#[tauri::command]` handlers.
    Tauri,
    /// React buttons, forms and links.
    React,
}

/// What kind of thing an endpoint is. Calls and endpoints meet on an [`EndpointKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    /// An HTTP route. The key's name is the path, normalized by [`crate::http::normalize_path`].
    Http,
    /// A TanStack Start server function. The key's name is the exported binding name.
    ServerFn,
    /// A Tauri command. The key's name is the command name as `invoke` uses it.
    TauriCommand,
    /// A CLI command (Typer). The key's name is the command name as typed.
    CliCommand,
    /// An MCP tool. The key's name is the tool name.
    McpTool,
}

/// Where a call and an endpoint meet.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EndpointKey {
    /// The kind of endpoint.
    pub kind: EndpointKind,
    /// HTTP method in upper case for [`EndpointKind::Http`]; `None` means any method.
    pub method: Option<String>,
    /// The name the two sides share (see [`EndpointKind`]).
    pub name: String,
}

/// Something that can be reached: an HTTP route, a server function, a Tauri command, a CLI
/// command or an MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// How callers reach it.
    pub key: EndpointKey,
    /// The function that handles it, by name, if known.
    pub handler: Option<String>,
    /// The router it hangs on (`<file>:<variable>`, e.g. `app/routers/users.py:router`), when
    /// its full path depends on where that router is mounted. A link pass ([`crate::Extractor::link`])
    /// rewrites `key.name` with the mount prefixes and may then clear this.
    pub router: Option<String>,
    /// Where it is declared.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// A router mounted under a prefix (FastAPI `include_router`, an APIRouter's own `prefix=`,
/// a TS router base path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    /// The app or router it is mounted on (`<file>:<variable>`); `None` for the router's own
    /// `prefix=` argument.
    pub parent: Option<String>,
    /// The router being mounted, as resolved by the extractor (`<file>:<variable>`), or as
    /// written when it could not be resolved.
    pub child: String,
    /// The prefix, as written (normalize with [`crate::http::normalize_path`] when joining).
    pub prefix: String,
    /// Where the mount is.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// A call from code to an endpoint (fetch, generated client, server function call, invoke, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Call {
    /// The endpoint it tries to reach.
    pub target: EndpointKey,
    /// The function or component the call sits in, if known.
    pub enclosing: Option<String>,
    /// True when part of the target could not be read statically (a computed URL segment). The
    /// readable part is in `target.name` with `{}` for each unknown segment.
    pub dynamic: bool,
    /// Where the call is.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// What a data write does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOp {
    /// INSERT, session.add, insertInto.
    Insert,
    /// UPDATE, updateTable.
    Update,
    /// DELETE, session.delete, deleteFrom.
    Delete,
    /// INSERT ... ON CONFLICT, merge, replaceInto.
    Upsert,
    /// Raw SQL or a write whose kind could not be read.
    Execute,
}

/// A change to stored data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataWrite {
    /// What it does.
    pub op: WriteOp,
    /// The table or model, if it could be read.
    pub table: Option<String>,
    /// The function it sits in, if known.
    pub enclosing: Option<String>,
    /// Where it is.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// A schema migration (Alembic revision, SQL migration file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Migration {
    /// Revision id, if the file declares one.
    pub revision: Option<String>,
    /// Tables it creates, alters or drops.
    pub tables: Vec<String>,
    /// Where it is.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// The kind of a UI control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiKind {
    /// A button (or anything with onClick acting as one).
    Button,
    /// A form (onSubmit / action).
    Form,
    /// A link to another page.
    Link,
}

/// A control a person can press.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiAction {
    /// Button, form or link.
    pub kind: UiKind,
    /// Its visible text, if it is a literal.
    pub label: Option<String>,
    /// The function it calls on click or submit, by name, if known. For a link, the target path.
    pub handler: Option<String>,
    /// The component it sits in.
    pub component: Option<String>,
    /// Where it is.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// A page a person can open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// The page's path, normalized by [`crate::http::normalize_path`].
    pub route: String,
    /// The component that renders it, if known.
    pub component: Option<String>,
    /// Where it is declared.
    pub loc: Loc,
    /// Which extractor found it.
    pub framework: Framework,
}

/// The kind of a definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefKind {
    /// A free function.
    Function,
    /// A method on a class, impl or object.
    Method,
    /// A class, struct or enum.
    Class,
    /// A React component.
    Component,
    /// A named constant or exported binding that is not a function.
    Constant,
}

/// A named definition in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    /// Its name as code refers to it (for a method, `Type.method`).
    pub name: String,
    /// What it is.
    pub kind: DefKind,
    /// Where it starts.
    pub loc: Loc,
    /// Its last line.
    pub end_line: u32,
    /// True when its body only raises not-implemented, is `pass`/`...`, `todo!()`,
    /// `unimplemented!()`, or throws a not-implemented error.
    pub is_stub: bool,
}

/// A plain call from one function to another, used to trace page -> call -> route -> data change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref {
    /// The function or component the call sits in, if known.
    pub caller: Option<String>,
    /// The name called (last segment for `a.b.c()`: `c`; the full text is not kept).
    pub callee: String,
    /// Where the call is.
    pub loc: Loc,
}

/// An import statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    /// The module as written (`app.routers.users`, `./api`, `@tanstack/react-start`).
    pub module: String,
    /// Names imported from it (empty for `import x`).
    pub names: Vec<String>,
    /// Where it is.
    pub loc: Loc,
}

/// Everything found in one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFacts {
    /// Repo-relative path, `/` separators.
    pub path: String,
    /// Its language.
    pub lang: Lang,
    /// True when tree-sitter reported syntax errors; facts may be partial.
    pub parse_errors: bool,
    /// Frameworks the file uses (an extractor adds its framework when it finds anything).
    pub frameworks: BTreeSet<Framework>,
    /// Endpoints declared here.
    pub endpoints: Vec<Endpoint>,
    /// Calls to endpoints made here.
    pub calls: Vec<Call>,
    /// Data writes made here.
    pub writes: Vec<DataWrite>,
    /// Migrations declared here.
    pub migrations: Vec<Migration>,
    /// UI controls here.
    pub ui: Vec<UiAction>,
    /// Pages declared here.
    pub pages: Vec<Page>,
    /// Named definitions.
    pub defs: Vec<Definition>,
    /// Plain function calls.
    pub refs: Vec<Ref>,
    /// Imports.
    pub imports: Vec<Import>,
    /// Routers mounted under prefixes.
    pub mounts: Vec<Mount>,
}

impl FileFacts {
    /// An empty record for `path`.
    pub fn new(path: &str, lang: Lang) -> Self {
        FileFacts {
            path: path.to_string(),
            lang,
            parse_errors: false,
            frameworks: BTreeSet::new(),
            endpoints: Vec::new(),
            calls: Vec::new(),
            writes: Vec::new(),
            migrations: Vec::new(),
            ui: Vec::new(),
            pages: Vec::new(),
            defs: Vec::new(),
            refs: Vec::new(),
            imports: Vec::new(),
            mounts: Vec::new(),
        }
    }

    /// A [`Loc`] in this file.
    pub fn loc(&self, line: u32) -> Loc {
        Loc {
            file: self.path.clone(),
            line,
        }
    }
}

/// A file that could not be read completely, and why.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Incomplete {
    /// Repo-relative path, or `""` for the walk as a whole.
    pub path: String,
    /// Why (too large, not UTF-8, read error, walk error).
    pub reason: String,
}

/// Everything found in a tree.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// One entry per parsed file, sorted by path.
    pub files: Vec<FileFacts>,
    /// Files that could not be read completely. A reader must not treat anything these files
    /// might hold as absent.
    pub incomplete: Vec<Incomplete>,
    /// Source files in languages no extractor parses (by extension), sorted. Their stacks read
    /// unverified.
    pub unsupported: Vec<String>,
    /// True when the walk stopped at the file limit; absence of anything is then unknown.
    pub truncated: bool,
}

impl Facts {
    /// True when every source file was listed and read.
    pub fn is_complete(&self) -> bool {
        !self.truncated && self.incomplete.is_empty()
    }

    /// Every endpoint in the tree.
    pub fn endpoints(&self) -> impl Iterator<Item = &Endpoint> {
        self.files.iter().flat_map(|f| f.endpoints.iter())
    }

    /// Every call in the tree.
    pub fn calls(&self) -> impl Iterator<Item = &Call> {
        self.files.iter().flat_map(|f| f.calls.iter())
    }

    /// Every data write in the tree.
    pub fn writes(&self) -> impl Iterator<Item = &DataWrite> {
        self.files.iter().flat_map(|f| f.writes.iter())
    }

    /// Every UI control in the tree.
    pub fn ui(&self) -> impl Iterator<Item = &UiAction> {
        self.files.iter().flat_map(|f| f.ui.iter())
    }

    /// Every page in the tree.
    pub fn pages(&self) -> impl Iterator<Item = &Page> {
        self.files.iter().flat_map(|f| f.pages.iter())
    }

    /// Every definition in the tree.
    pub fn defs(&self) -> impl Iterator<Item = &Definition> {
        self.files.iter().flat_map(|f| f.defs.iter())
    }

    /// Every plain call in the tree.
    pub fn refs(&self) -> impl Iterator<Item = &Ref> {
        self.files.iter().flat_map(|f| f.refs.iter())
    }

    /// Every framework seen anywhere.
    pub fn frameworks(&self) -> BTreeSet<Framework> {
        self.files
            .iter()
            .flat_map(|f| f.frameworks.iter().copied())
            .collect()
    }

    /// The facts for one file.
    pub fn file(&self, path: &str) -> Option<&FileFacts> {
        self.files
            .binary_search_by(|f| f.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.files[i])
    }
}
