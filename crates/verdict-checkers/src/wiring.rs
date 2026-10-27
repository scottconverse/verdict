//! wiring: does the thing a claim names exist, and is its chain whole?
//!
//! The checker reads the tree once per run with [`verdict_extract::extract_tree`] (tree-sitter,
//! static reads only; the user's code is never run) and gives every claim, plan line and
//! done-when item that names a URL, a file or a symbol exactly one of five words. A claim that
//! names several things gets the worst word among them (BROKEN, then NOT BUILT, then HALF BUILT,
//! then SKIPPED, then CONNECTED).
//!
//! * **CONNECTED**: the named thing exists, and every link of its chain the extractors can read
//!   is whole: a route or command has a handler that is defined and is not a stub; when the claim
//!   says a person can use it, something calls it; when the claim says it saves or stores, the
//!   handler reaches a data write; a named file holds no broken call, no control without a
//!   handler and is not all stubs. A file that is not source code only has to exist.
//! * **BROKEN**: a call targets an endpoint no route, server function or command serves (and the
//!   serving side is fully read), or a route, command or control names a handler that is defined
//!   nowhere.
//! * **NOT BUILT**: the named thing is not there at all: no file with that path, no route,
//!   page, call or definition with that name, and the name appears in no source file, with a
//!   complete read of the tree.
//! * **HALF BUILT**: it exists but a link is missing: a route or command nothing calls when the
//!   claim says a person can use it, a control with no handler, a handler or function that is a
//!   stub (empty, `todo!()`, or only throws "not implemented"), or a handler that never reaches
//!   the data change the claim names.
//! * **SKIPPED**: the named thing is, or could be, in a stack no extractor in this build reads (a
//!   language or server framework it does not parse), or the facts are incomplete or truncated
//!   where it could be. SKIPPED is never clean.
//!
//! Findings: `wiring:broken.<hash>`, `wiring:not_built.<hash>`, `wiring:half_built.<hash>`, one
//! per claim, with the claim's ledger pointer and file:line pointers, and the path or the reason
//! in the title. A finding blocks (severity `Blocker`) only on a **ticked** item: a final-message,
//! commit-message or notes-file claim, a done-when item, or a plan line or prompt translation
//! that starts with a checked box (`[x]`). Any other item's finding is a `Nit` that shows in the
//! check's counts and does not block.
//!
//! CONNECTED adds no finding; its path is written into the check's detail. SKIPPED adds no
//! finding either: when no blocking finding settles the run, a SKIPPED item makes the whole check
//! `check_unavailable` (the verdict reads it as not clean, PARTIAL when the lane requires
//! wiring), with every item's word and reason in the record.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::LazyLock;

use regex::Regex;
use verdict_core::part1c::extract_names;
use verdict_core::{ClaimSource, Finding, Lane, Policy, Severity, Tier};
use verdict_extract::http::{methods_match, normalize_path, paths_match};
use verdict_extract::syntax::{is_unsupported_source, lang_of};
use verdict_extract::{
    Call, DataWrite, DefKind, Definition, Endpoint, EndpointKind, Facts, Framework, Lang, Ref,
    ScanOptions, UiAction, UiKind, extract_tree,
};

use crate::Checker;
use crate::context::{CheckContext, ClaimRef, Draft, Unavailable, finish};
use crate::paths;

/// The wiring checker.
pub struct Wiring;

/// One of the five words. Ordered from best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Word {
    /// The named thing exists and its chain is whole.
    Connected,
    /// It is in a stack no extractor reads, or the facts are incomplete where it could be.
    Skipped,
    /// It exists but a link of its chain is missing.
    HalfBuilt,
    /// It is not there.
    NotBuilt,
    /// A call targets nothing, or a handler is defined nowhere.
    Broken,
}

impl Word {
    /// The word as the report prints it.
    pub fn label(self) -> &'static str {
        match self {
            Word::Connected => "CONNECTED",
            Word::Skipped => "SKIPPED",
            Word::HalfBuilt => "HALF BUILT",
            Word::NotBuilt => "NOT BUILT",
            Word::Broken => "BROKEN",
        }
    }

    /// The finding rule, for the three words that are findings.
    pub fn rule(self) -> Option<&'static str> {
        match self {
            Word::Broken => Some("broken"),
            Word::NotBuilt => Some("not_built"),
            Word::HalfBuilt => Some("half_built"),
            Word::Connected | Word::Skipped => None,
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Word::Broken => {
                "add the route, command or handler the call names, or point the call at one that exists"
            }
            Word::NotBuilt => "build it, or correct the claim",
            Word::HalfBuilt => "finish the missing link named in the title, or correct the claim",
            _ => "",
        }
    }
}

/// The five counts in the order the report prints them.
pub const WORD_ORDER: [Word; 5] = [
    Word::Connected,
    Word::Broken,
    Word::NotBuilt,
    Word::HalfBuilt,
    Word::Skipped,
];

/// What one name (or one tree item) came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The word.
    pub word: Word,
    /// The path (CONNECTED) or the reason (any other word), in plain words.
    pub detail: String,
    /// file:line places that show it, most telling first.
    pub at: Vec<(String, u32)>,
}

impl Outcome {
    fn new(word: Word, detail: impl Into<String>, at: Vec<(String, u32)>) -> Outcome {
        Outcome {
            word,
            detail: detail.into(),
            at,
        }
    }
}

/// The worst of several outcomes (the first one among equals).
fn worst(outcomes: Vec<Outcome>) -> Option<Outcome> {
    let mut best: Option<Outcome> = None;
    for o in outcomes {
        if best.as_ref().is_none_or(|b| o.word > b.word) {
            best = Some(o);
        }
    }
    best
}

/// The best of several outcomes (the first one among equals).
fn best(outcomes: Vec<Outcome>) -> Option<Outcome> {
    let mut out: Option<Outcome> = None;
    for o in outcomes {
        if out.as_ref().is_none_or(|b| o.word < b.word) {
            out = Some(o);
        }
    }
    out
}

static PERSON_FACING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:users?|persons?|people|customers?|visitors?|editors?|admins?|someone|anyone|can|lets?|allows?|able to|click\w*|press\w*|tap\w*|submit\w*)\b",
    )
    .expect("static regex")
});

static DATA_CHANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:sav(?:e|es|ed|ing)|stor(?:e|es|ed|ing)|persist\w*|insert\w*|upsert\w*|writes? to|written to|wrote to)\b",
    )
    .expect("static regex")
});

static NEGATIVE_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:delet\w*|remov\w*|drop(?:ped|ping)?|renam\w*|moved?|moving|replac\w*|deprecat\w*|got rid)\b",
    )
    .expect("static regex")
});

static CHECKED_BOX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]\s*)?\[[xX]\]").expect("static regex"));

static QUOTED_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^[\w.$]+\(\s*["'`]([\w:./-]+)["'`]"#).expect("static regex"));

/// Server frameworks whose routes no extractor in this build reads: a project that imports one
/// cannot prove an HTTP call has no route.
const UNREAD_SERVERS: &[&str] = &[
    "express", "hono", "fastify", "koa", "@nestjs/", "next", "next/", "elysia", "h3", "nitro",
    "@hapi/", "restify", "polka",
];

/// Folder prefixes that mark a path as on the machine, not in the project.
const ABSOLUTE_PREFIXES: &[&str] = &[
    "/home/", "/Users/", "/tmp/", "/etc/", "/usr/", "/var/", "/opt/", "/mnt/", "/root/", "/bin/",
    "/c/", "/d/",
];

/// True when the item counts as done: a final-message, commit-message or notes-file claim, a
/// done-when item, or a plan line / prompt translation with a checked box.
pub fn is_ticked(claim: &ClaimRef) -> bool {
    match claim.source {
        ClaimSource::FinalMessage
        | ClaimSource::CommitMessage
        | ClaimSource::NotesFile
        | ClaimSource::DoneWhen => true,
        ClaimSource::PlanLine | ClaimSource::PromptTranslation => CHECKED_BOX.is_match(&claim.text),
    }
}

/// The name the BFS and the lookups key on: `Notes.save` -> `save`; names with spaces (an
/// inline route handler, `GET /api/x`) or a `tool:` prefix stay whole.
pub(crate) fn key(name: &str) -> &str {
    if name.contains(' ') || name.starts_with("tool:") {
        return name;
    }
    let n = name.strip_suffix("()").unwrap_or(name);
    let n = n.rsplit("::").next().unwrap_or(n);
    n.rsplit('.').next().unwrap_or(n)
}

fn loc_s(file: &str, line: u32) -> String {
    format!("{file}:{line}")
}

fn is_fn_kind(k: DefKind) -> bool {
    matches!(k, DefKind::Function | DefKind::Method | DefKind::Component)
}

fn kind_label(ep: &Endpoint) -> String {
    match ep.key.kind {
        EndpointKind::Http => format!(
            "route {} {}",
            ep.key.method.as_deref().unwrap_or("ANY"),
            ep.key.name
        ),
        EndpointKind::ServerFn => format!("server function {}", ep.key.name),
        EndpointKind::TauriCommand => format!("Tauri command {}", ep.key.name),
        EndpointKind::CliCommand => format!("command {}", ep.key.name),
        EndpointKind::McpTool => format!("MCP tool {}", ep.key.name),
    }
}

fn call_label(c: &Call) -> String {
    match c.target.kind {
        EndpointKind::Http => format!(
            "call {} {}",
            c.target.method.as_deref().unwrap_or("ANY"),
            c.target.name
        ),
        EndpointKind::ServerFn => format!("call to server function {}", c.target.name),
        EndpointKind::TauriCommand => format!("invoke(\"{}\")", c.target.name),
        EndpointKind::CliCommand => format!("call to command {}", c.target.name),
        EndpointKind::McpTool => format!("call to MCP tool {}", c.target.name),
    }
}

fn ui_label(u: &UiAction) -> String {
    let kind = match u.kind {
        UiKind::Button => "button",
        UiKind::Form => "form",
        UiKind::Link => "link",
    };
    match &u.label {
        Some(l) if !l.is_empty() => format!("{kind} \"{l}\""),
        _ => kind.to_string(),
    }
}

/// Does `call` target `ep`?
pub(crate) fn call_hits(call: &Call, ep: &Endpoint) -> bool {
    if call.target.kind != ep.key.kind {
        return false;
    }
    match ep.key.kind {
        EndpointKind::Http => {
            paths_match(&call.target.name, &ep.key.name)
                && methods_match(call.target.method.as_deref(), ep.key.method.as_deref())
        }
        _ => call.target.name == ep.key.name,
    }
}

/// The state of a named handler.
enum Handler<'a> {
    Whole(&'a Definition),
    Stub(&'a Definition),
    Unknown(String),
    Missing,
}

/// The facts of one run, indexed for lookups. Shared by the checker and `vdt scan`.
pub(crate) struct Index<'a> {
    pub facts: &'a Facts,
    text: &'a BTreeMap<String, String>,
    readable: BTreeSet<Lang>,
    /// Source files no extractor in this build reads: other languages, and languages this build
    /// parses with no extractor yet.
    pub unread: Vec<String>,
    /// `Err(why)` when an HTTP call's missing route cannot be proven missing.
    pub server: Result<(), String>,
    /// `Err(why)` when a Tauri command's absence cannot be proven.
    pub tauri: Result<(), String>,
    schema: bool,
    defs: BTreeMap<String, Vec<&'a Definition>>,
    callers: BTreeMap<String, Vec<&'a Ref>>,
    imported: BTreeSet<String>,
    edges: BTreeMap<String, BTreeSet<String>>,
    writes_in: BTreeMap<String, Vec<&'a DataWrite>>,
}

impl<'a> Index<'a> {
    pub fn new(facts: &'a Facts, text: &'a BTreeMap<String, String>) -> Index<'a> {
        let readable: BTreeSet<Lang> = verdict_extract::registry()
            .iter()
            .flat_map(|x| x.langs().iter().copied())
            .collect();
        let mut unread: Vec<String> = facts
            .files
            .iter()
            .filter(|f| !readable.contains(&f.lang) && !paths::is_noise_path(&f.path))
            .map(|f| f.path.clone())
            .collect();
        unread.extend(
            facts
                .unsupported
                .iter()
                .filter(|p| !paths::is_noise_path(p))
                .cloned(),
        );
        unread.sort();
        unread.dedup();

        let mut defs: BTreeMap<String, Vec<&Definition>> = BTreeMap::new();
        for d in facts.defs() {
            defs.entry(key(&d.name).to_string()).or_default().push(d);
        }
        let mut callers: BTreeMap<String, Vec<&Ref>> = BTreeMap::new();
        let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for r in facts.refs() {
            callers
                .entry(key(&r.callee).to_string())
                .or_default()
                .push(r);
            if let Some(c) = &r.caller {
                edges
                    .entry(key(c).to_string())
                    .or_default()
                    .insert(key(&r.callee).to_string());
            }
        }
        for c in facts.calls() {
            let Some(enc) = &c.enclosing else { continue };
            for ep in facts.endpoints().filter(|e| call_hits(c, e)) {
                if let Some(h) = &ep.handler {
                    edges
                        .entry(key(enc).to_string())
                        .or_default()
                        .insert(key(h).to_string());
                }
            }
        }
        let mut writes_in: BTreeMap<String, Vec<&DataWrite>> = BTreeMap::new();
        for w in facts.writes() {
            if let Some(e) = &w.enclosing {
                writes_in.entry(key(e).to_string()).or_default().push(w);
            }
        }
        let imported: BTreeSet<String> = facts
            .files
            .iter()
            .flat_map(|f| f.imports.iter().flat_map(|i| i.names.iter().cloned()))
            .collect();

        let complete = if facts.truncated {
            Err(
                "the file list stopped at its limit, so a missing route cannot be proven missing"
                    .to_string(),
            )
        } else if let Some(i) = facts.incomplete.first() {
            Err(format!(
                "{} source file(s) could not be read completely (first: {}: {})",
                facts.incomplete.len(),
                i.path,
                i.reason
            ))
        } else {
            Ok(())
        };
        let server = complete.clone().and_then(|_| {
            if let Some(p) = unread.first() {
                return Err(format!(
                    "{} source file(s) are in a stack no extractor in this build reads (first: {p}), so a route could be served there",
                    unread.len()
                ));
            }
            let unread_server = facts.files.iter().find_map(|f| {
                f.imports.iter().find_map(|i| {
                    UNREAD_SERVERS
                        .iter()
                        .find(|s| {
                            if s.ends_with('/') {
                                i.module.starts_with(*s)
                            } else {
                                i.module == **s || i.module.starts_with(&format!("{s}/"))
                            }
                        })
                        .map(|s| (f.path.clone(), s.trim_end_matches('/').to_string()))
                })
            });
            if let Some((p, s)) = unread_server {
                return Err(format!(
                    "{p} imports {s}, a server framework no extractor in this build reads"
                ));
            }
            let served = facts
                .endpoints()
                .any(|e| e.key.kind == EndpointKind::Http && e.framework != Framework::OpenapiTs);
            if !served {
                return Err(
                    "no server route was found in the code read, so the server is somewhere this build cannot see".to_string(),
                );
            }
            Ok(())
        });
        let tauri = complete.and_then(|_| {
            if !readable.contains(&Lang::Rust) {
                return Err("no Rust extractor in this build".to_string());
            }
            if !facts.files.iter().any(|f| f.lang == Lang::Rust) {
                return Err(
                    "no Rust source was found, so the commands are somewhere this build cannot see"
                        .to_string(),
                );
            }
            Ok(())
        });
        let schema = facts
            .endpoints()
            .any(|e| e.framework == Framework::OpenapiTs);
        Index {
            facts,
            text,
            readable,
            unread,
            server,
            tauri,
            schema,
            defs,
            callers,
            imported,
            edges,
            writes_in,
        }
    }

    /// True when the language has an extractor in this build.
    pub fn reads(&self, lang: Lang) -> bool {
        self.readable.contains(&lang)
    }

    fn word_re(ident: &str) -> Option<Regex> {
        Regex::new(&format!(r"\b{}\b", regex::escape(ident))).ok()
    }

    /// Up to `limit` places where the identifier appears as a whole word in a source file.
    fn mentions(&self, ident: &str, limit: usize) -> Vec<(String, u32)> {
        let mut out = Vec::new();
        let Some(re) = Self::word_re(ident) else {
            return out;
        };
        for (path, text) in self.text {
            if !(paths::is_code(path) || lang_of(path).is_some()) || paths::is_noise_path(path) {
                continue;
            }
            if !re.is_match(text) {
                continue;
            }
            for (i, l) in text.lines().enumerate() {
                if re.is_match(l) {
                    out.push((path.clone(), i as u32 + 1));
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }

    fn defs_of(&self, name: &str) -> Vec<&'a Definition> {
        self.defs.get(key(name)).cloned().unwrap_or_default()
    }

    fn handler(&self, h: &str) -> Handler<'a> {
        let defs = self.defs_of(h);
        if !defs.is_empty() {
            let fns: Vec<&Definition> = defs
                .iter()
                .copied()
                .filter(|d| is_fn_kind(d.kind))
                .collect();
            if !fns.is_empty() && fns.iter().all(|d| d.is_stub) {
                return Handler::Stub(fns[0]);
            }
            let pick = fns.iter().copied().find(|d| !d.is_stub).unwrap_or(defs[0]);
            return Handler::Whole(pick);
        }
        if self.imported.contains(key(h)) {
            return Handler::Unknown(format!(
                "{h} is imported from code the extractors did not trace"
            ));
        }
        if h.contains(' ') || h.starts_with("tool:") {
            return Handler::Unknown(format!("the inline handler {h} could not be read"));
        }
        if self.mentions(key(h), 2).len() <= 1 {
            Handler::Missing
        } else {
            Handler::Unknown(format!(
                "{h} is named in the code but its definition could not be read"
            ))
        }
    }

    /// Every call that targets `ep`.
    pub fn callers_of(&self, ep: &Endpoint) -> Vec<&'a Call> {
        self.facts.calls().filter(|c| call_hits(c, ep)).collect()
    }

    fn endpoints_for(&self, call: &Call) -> Vec<&'a Endpoint> {
        self.facts
            .endpoints()
            .filter(|e| call_hits(call, e))
            .collect()
    }

    /// What the call's caller chain looks like, from the page down: `page /notes -> button
    /// "Save" (f:l) -> call in saveNote (f:l)`.
    fn caller_desc(&self, c: &Call) -> String {
        let mut parts = Vec::new();
        let enc = c.enclosing.as_deref();
        let control = enc.and_then(|e| {
            self.facts
                .ui()
                .find(|u| u.handler.as_deref().map(key) == Some(key(e)))
                .or_else(|| self.facts.ui().find(|u| u.component.as_deref() == Some(e)))
        });
        let component = control
            .and_then(|u| u.component.as_deref())
            .or(enc)
            .map(str::to_string);
        if let Some(comp) = &component
            && let Some(p) = self
                .facts
                .pages()
                .find(|p| p.component.as_deref() == Some(comp.as_str()))
        {
            parts.push(format!(
                "page {} ({})",
                p.route,
                loc_s(&p.loc.file, p.loc.line)
            ));
        }
        if let Some(u) = control {
            parts.push(format!(
                "{} ({})",
                ui_label(u),
                loc_s(&u.loc.file, u.loc.line)
            ));
        }
        parts.push(format!(
            "{} in {} ({})",
            call_label(c),
            enc.unwrap_or("module code"),
            loc_s(&c.loc.file, c.loc.line)
        ));
        parts.join(" -> ")
    }

    /// Breadth-first from a function (by key) over calls, cross-boundary calls and writes; the
    /// first write reached (to `table` when given) and the functions on the way.
    fn reach_write(
        &self,
        start: &str,
        table: Option<&str>,
    ) -> Option<(Vec<String>, &'a DataWrite)> {
        let start = key(start).to_string();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();
        queue.push_back((start.clone(), vec![start.clone()]));
        seen.insert(start);
        while let Some((node, path)) = queue.pop_front() {
            if let Some(ws) = self.writes_in.get(&node) {
                let hit = ws.iter().copied().find(|w| match table {
                    Some(t) => w
                        .table
                        .as_deref()
                        .is_some_and(|x| x.eq_ignore_ascii_case(t)),
                    None => true,
                });
                if let Some(w) = hit {
                    return Some((path, w));
                }
            }
            if path.len() > 8 {
                continue;
            }
            if let Some(next) = self.edges.get(&node) {
                for n in next {
                    if seen.insert(n.clone()) {
                        let mut p = path.clone();
                        p.push(n.clone());
                        queue.push_back((n.clone(), p));
                    }
                }
            }
        }
        None
    }

    /// The table the claim names, if it names one any write touches.
    fn named_table(&self, text: &str) -> Option<String> {
        let lower = text.to_ascii_lowercase();
        let tables: BTreeSet<String> = self
            .facts
            .writes()
            .filter_map(|w| w.table.clone())
            .collect();
        tables.into_iter().find(|t| paths::mentions(&lower, t))
    }

    /// Follow a handler to the data change the claim names.
    fn data_link(&self, h: &str, text: &str) -> Result<Option<String>, Outcome> {
        if !DATA_CHANGE.is_match(text) {
            return Ok(None);
        }
        let table = self.named_table(text);
        match self.reach_write(h, table.as_deref()) {
            Some((path, w)) => Ok(Some(
                format!(
                    "{}{:?} {} ({})",
                    if path.len() > 1 {
                        format!("{} -> ", path[1..].join(" -> "))
                    } else {
                        String::new()
                    },
                    w.op,
                    w.table.as_deref().unwrap_or("(table not read)"),
                    loc_s(&w.loc.file, w.loc.line)
                )
                .replace("Insert ", "insert ")
                .replace("Update ", "update ")
                .replace("Delete ", "delete ")
                .replace("Upsert ", "upsert ")
                .replace("Execute ", "write "),
            )),
            None if self.facts.writes().next().is_some() => Err(Outcome::new(
                Word::HalfBuilt,
                format!(
                    "the claim says it saves or stores{}, but {h} never reaches a data write the extractors read",
                    table.map(|t| format!(" to {t}")).unwrap_or_default()
                ),
                Vec::new(),
            )),
            None => Err(Outcome::new(
                Word::Skipped,
                format!(
                    "the claim says it saves or stores, and no data write in this project is in a library the extractors read, so whether {h} reaches one is unknown"
                ),
                Vec::new(),
            )),
        }
    }

    /// The word for one endpoint, against the claim's text (empty for a scan).
    pub fn eval_endpoint(&self, ep: &Endpoint, text: &str) -> Outcome {
        let here = (ep.loc.file.clone(), ep.loc.line);
        let label = format!("{} ({})", kind_label(ep), loc_s(&ep.loc.file, ep.loc.line));
        let callers = self.callers_of(ep);
        let Some(h) = &ep.handler else {
            let why = if ep.framework == Framework::OpenapiTs {
                format!(
                    "{label}: only the generated OpenAPI schema declares it; the server that serves it is not read"
                )
            } else {
                format!("{label}: its handler could not be read")
            };
            return Outcome::new(Word::Skipped, why, vec![here]);
        };
        let def = match self.handler(h) {
            Handler::Missing => {
                return Outcome::new(
                    Word::Broken,
                    format!("{label} names the handler {h}, which is defined nowhere"),
                    vec![here],
                );
            }
            Handler::Unknown(why) => {
                return Outcome::new(Word::Skipped, format!("{label}: {why}"), vec![here]);
            }
            Handler::Stub(d) => {
                return Outcome::new(
                    Word::HalfBuilt,
                    format!(
                        "{label}: its handler {h} is a stub ({})",
                        loc_s(&d.loc.file, d.loc.line)
                    ),
                    vec![(d.loc.file.clone(), d.loc.line), here],
                );
            }
            Handler::Whole(d) => d,
        };
        let callable = matches!(
            ep.key.kind,
            EndpointKind::Http | EndpointKind::ServerFn | EndpointKind::TauriCommand
        );
        if callable && callers.is_empty() && PERSON_FACING.is_match(text) {
            return Outcome::new(
                Word::HalfBuilt,
                format!(
                    "nothing in the code calls the {label}, and the claim says a person can use it"
                ),
                vec![here],
            );
        }
        let write = match self.data_link(h, text) {
            Ok(w) => w,
            Err(mut o) => {
                o.detail = format!("{label}: {}", o.detail);
                o.at = vec![(def.loc.file.clone(), def.loc.line), here];
                return o;
            }
        };
        let mut chain = Vec::new();
        if let Some(c) = callers.first() {
            chain.push(self.caller_desc(c));
        }
        chain.push(label);
        if !h.contains(' ') && !h.starts_with("tool:") || def.loc != ep.loc {
            chain.push(format!(
                "handler {} ({})",
                h,
                loc_s(&def.loc.file, def.loc.line)
            ));
        }
        if let Some(w) = write {
            chain.push(w);
        }
        let mut at = vec![here];
        if let Some(c) = callers.first() {
            at.insert(0, (c.loc.file.clone(), c.loc.line));
        }
        Outcome::new(Word::Connected, chain.join(" -> "), at)
    }

    /// `Some` when the call reaches nothing (BROKEN, or SKIPPED when that cannot be proven);
    /// `None` when an endpoint serves it.
    pub fn broken_call(&self, c: &Call) -> Option<Outcome> {
        let here = vec![(c.loc.file.clone(), c.loc.line)];
        let label = format!("{} ({})", call_label(c), loc_s(&c.loc.file, c.loc.line));
        let served = self.endpoints_for(c);
        if served.iter().any(|e| e.framework != Framework::OpenapiTs) {
            return None;
        }
        if c.target.kind == EndpointKind::Http {
            if c.framework == Framework::OpenapiTs && self.schema {
                return if served.is_empty() {
                    Some(Outcome::new(
                        Word::Broken,
                        format!(
                            "{label} targets a path the generated OpenAPI schema does not have"
                        ),
                        here,
                    ))
                } else {
                    match &self.server {
                        Ok(()) => Some(Outcome::new(
                            Word::Broken,
                            format!(
                                "{label}: the schema lists it, but no route in the code serves it"
                            ),
                            here,
                        )),
                        Err(why) => Some(Outcome::new(
                            Word::Skipped,
                            format!(
                                "{label}: the schema lists it; whether a route serves it is unknown: {why}"
                            ),
                            here,
                        )),
                    }
                };
            }
            if !served.is_empty() {
                return Some(Outcome::new(
                    Word::Skipped,
                    format!(
                        "{label}: only the generated OpenAPI schema declares it; the server is not read"
                    ),
                    here,
                ));
            }
            if c.dynamic {
                return Some(Outcome::new(
                    Word::Skipped,
                    format!("{label}: the URL is built at run time, so its route cannot be read"),
                    here,
                ));
            }
            return Some(match &self.server {
                Ok(()) => Outcome::new(
                    Word::Broken,
                    format!("{label} targets a route no code serves"),
                    here,
                ),
                Err(why) => Outcome::new(
                    Word::Skipped,
                    format!("{label} targets no route found; {why}"),
                    here,
                ),
            });
        }
        if c.target.kind == EndpointKind::TauriCommand {
            return Some(match &self.tauri {
                Ok(()) => Outcome::new(
                    Word::Broken,
                    format!("{label} targets a Tauri command no #[tauri::command] function serves"),
                    here,
                ),
                Err(why) => Outcome::new(
                    Word::Skipped,
                    format!("{label} targets no command found; {why}"),
                    here,
                ),
            });
        }
        if c.target.kind == EndpointKind::ServerFn {
            return Some(Outcome::new(
                Word::Broken,
                format!("{label} targets a server function that is not defined"),
                here,
            ));
        }
        Some(Outcome::new(
            Word::Skipped,
            format!("{label}: the extractors do not read where it is served"),
            here,
        ))
    }

    /// Controls in the tree with a problem: no handler (HALF BUILT) or a handler defined nowhere
    /// (BROKEN). Links are not controls with handlers.
    pub fn control_problem(&self, u: &UiAction) -> Option<Outcome> {
        if u.kind == UiKind::Link {
            return None;
        }
        let here = vec![(u.loc.file.clone(), u.loc.line)];
        let label = format!(
            "the {} ({}){}",
            ui_label(u),
            loc_s(&u.loc.file, u.loc.line),
            u.component
                .as_deref()
                .map(|c| format!(" in {c}"))
                .unwrap_or_default()
        );
        match u.handler.as_deref() {
            None => Some(Outcome::new(
                Word::HalfBuilt,
                format!("{label} has no handler"),
                here,
            )),
            Some(h) if h.starts_with('(') => None,
            Some(h) => {
                if !self.defs_of(h).is_empty() || self.imported.contains(key(h)) {
                    return None;
                }
                if self.mentions(key(h), 2).len() <= 1 {
                    Some(Outcome::new(
                        Word::Broken,
                        format!("{label} calls {h}, which is defined nowhere"),
                        here,
                    ))
                } else {
                    None
                }
            }
        }
    }

    /// True when something calls, renders, registers or routes to the definition.
    pub fn is_used_def(&self, d: &Definition) -> bool {
        self.is_used(&d.name, d)
    }

    fn is_used(&self, ident: &str, d: &Definition) -> bool {
        let k = key(ident);
        let by_ref = self.callers.get(k).is_some_and(|rs| {
            rs.iter().any(|r| {
                r.caller.as_deref().map(key) != Some(key(&d.name)) || r.loc.file != d.loc.file
            })
        });
        by_ref
            || self
                .facts
                .ui()
                .any(|u| u.handler.as_deref().map(key) == Some(k))
            || self
                .facts
                .pages()
                .any(|p| p.component.as_deref().map(key) == Some(k))
            || self
                .facts
                .endpoints()
                .any(|e| e.handler.as_deref().map(key) == Some(k))
    }

    fn eval_def(&self, ident: &str, defs: &[&'a Definition], text: &str) -> Outcome {
        let fns: Vec<&Definition> = defs
            .iter()
            .copied()
            .filter(|d| is_fn_kind(d.kind))
            .collect();
        let first = fns.first().copied().unwrap_or(defs[0]);
        let here = (first.loc.file.clone(), first.loc.line);
        let label = format!(
            "{} ({})",
            first.name,
            loc_s(&first.loc.file, first.loc.line)
        );
        if !fns.is_empty() && fns.iter().all(|d| d.is_stub) {
            return Outcome::new(
                Word::HalfBuilt,
                format!("{label} is a stub: its body does nothing yet"),
                vec![here],
            );
        }
        let def = fns.iter().copied().find(|d| !d.is_stub).unwrap_or(first);
        if !fns.is_empty() && PERSON_FACING.is_match(text) && !self.is_used(ident, def) {
            return Outcome::new(
                Word::HalfBuilt,
                format!(
                    "nothing in the code calls {label}, and the claim says a person can use it"
                ),
                vec![here],
            );
        }
        let write = if fns.is_empty() {
            None
        } else {
            match self.data_link(&def.name, text) {
                Ok(w) => w,
                Err(mut o) => {
                    o.detail = format!("{label}: {}", o.detail);
                    o.at = vec![here];
                    return o;
                }
            }
        };
        let mut chain = Vec::new();
        let k = key(ident);
        if let Some(r) = self.callers.get(k).and_then(|rs| {
            rs.iter()
                .find(|r| r.caller.as_deref().map(key) != Some(key(&def.name)))
        }) {
            chain.push(format!(
                "{} ({})",
                r.caller.as_deref().unwrap_or("module code"),
                loc_s(&r.loc.file, r.loc.line)
            ));
        } else if let Some(u) = self
            .facts
            .ui()
            .find(|u| u.handler.as_deref().map(key) == Some(k))
        {
            chain.push(format!(
                "{} ({})",
                ui_label(u),
                loc_s(&u.loc.file, u.loc.line)
            ));
        } else if let Some(p) = self
            .facts
            .pages()
            .find(|p| p.component.as_deref().map(key) == Some(k))
        {
            chain.push(format!(
                "page {} ({})",
                p.route,
                loc_s(&p.loc.file, p.loc.line)
            ));
        }
        chain.push(format!(
            "{} ({})",
            def.name,
            loc_s(&def.loc.file, def.loc.line)
        ));
        if let Some(w) = write {
            chain.push(w);
        }
        Outcome::new(
            Word::Connected,
            chain.join(" -> "),
            vec![(def.loc.file.clone(), def.loc.line)],
        )
    }

    /// True when anything in the facts carries this identifier.
    fn known(&self, ident: &str) -> bool {
        let k = key(ident);
        self.defs.contains_key(k)
            || self.callers.contains_key(k)
            || self.imported.contains(k)
            || self
                .facts
                .endpoints()
                .any(|e| e.key.name == ident || e.handler.as_deref().map(key) == Some(k))
            || self.facts.calls().any(|c| c.target.name == ident)
            || self
                .facts
                .ui()
                .any(|u| u.handler.as_deref().map(key) == Some(k))
    }

    /// The word for a symbol name. `None` when it is not worth a word (a prose word that only
    /// looks like a name).
    pub fn eval_symbol(&self, ident: &str, explicit: bool, text: &str) -> Option<Outcome> {
        if !explicit && !self.known(ident) {
            return None;
        }
        let k = key(ident);
        // A command, server function or tool by its own name.
        let named: Vec<&Endpoint> = self
            .facts
            .endpoints()
            .filter(|e| e.key.kind != EndpointKind::Http && e.key.name == ident)
            .collect();
        if !named.is_empty() {
            return best(named.iter().map(|e| self.eval_endpoint(e, text)).collect());
        }
        // A handler of a route, command or tool.
        let handled: Vec<&Endpoint> = self
            .facts
            .endpoints()
            .filter(|e| e.handler.as_deref().map(key) == Some(k))
            .collect();
        if !handled.is_empty() {
            return best(
                handled
                    .iter()
                    .map(|e| self.eval_endpoint(e, text))
                    .collect(),
            );
        }
        // Calls to a command or server function nobody serves.
        let calls: Vec<Outcome> = self
            .facts
            .calls()
            .filter(|c| c.target.kind != EndpointKind::Http && c.target.name == ident)
            .filter_map(|c| self.broken_call(c))
            .collect();
        if let Some(o) = worst(calls) {
            return Some(o);
        }
        let defs = self.defs_of(ident);
        if !defs.is_empty() {
            return Some(self.eval_def(ident, &defs, text));
        }
        // A control whose handler is this name, defined nowhere.
        if let Some(o) = self
            .facts
            .ui()
            .filter(|u| u.handler.as_deref().map(key) == Some(k))
            .find_map(|u| self.control_problem(u))
        {
            return Some(o);
        }
        let found = self.mentions(k, 1);
        if let Some((p, l)) = found.first() {
            let unread = lang_of(p).is_none_or(|lang| !self.reads(lang));
            return Some(Outcome::new(
                Word::Skipped,
                if unread {
                    format!(
                        "{ident} appears at {}, in a stack no extractor in this build reads",
                        loc_s(p, *l)
                    )
                } else {
                    format!(
                        "{ident} appears at {}, but not as a definition, route, command or control the extractors read",
                        loc_s(p, *l)
                    )
                },
                vec![(p.clone(), *l)],
            ));
        }
        if self.facts.truncated || !self.facts.incomplete.is_empty() {
            return Some(Outcome::new(
                Word::Skipped,
                format!("{ident} was not found, but the read of the tree was incomplete"),
                Vec::new(),
            ));
        }
        Some(Outcome::new(
            Word::NotBuilt,
            format!("{ident} is defined and used nowhere in the project"),
            Vec::new(),
        ))
    }

    /// The word for a route or a local URL. `local` is false for a URL on another host, which
    /// gets a word only when it matches something here.
    pub fn eval_route(
        &self,
        path: &str,
        method: Option<&str>,
        local: bool,
        text: &str,
    ) -> Option<Outcome> {
        let eps: Vec<&Endpoint> = self
            .facts
            .endpoints()
            .filter(|e| {
                e.key.kind == EndpointKind::Http
                    && paths_match(&e.key.name, path)
                    && methods_match(e.key.method.as_deref(), method)
            })
            .collect();
        let mut found: Vec<Outcome> = eps.iter().map(|e| self.eval_endpoint(e, text)).collect();
        for p in self.facts.pages().filter(|p| paths_match(&p.route, path)) {
            let here = (p.loc.file.clone(), p.loc.line);
            let label = format!("page {} ({})", p.route, loc_s(&p.loc.file, p.loc.line));
            found.push(match &p.component {
                None => Outcome::new(
                    Word::Skipped,
                    format!("{label}: the component that renders it could not be read"),
                    vec![here],
                ),
                Some(c) => match self.handler(c) {
                    Handler::Whole(d) => Outcome::new(
                        Word::Connected,
                        format!(
                            "{label} -> component {c} ({})",
                            loc_s(&d.loc.file, d.loc.line)
                        ),
                        vec![here, (d.loc.file.clone(), d.loc.line)],
                    ),
                    Handler::Stub(d) => Outcome::new(
                        Word::HalfBuilt,
                        format!(
                            "{label}: its component {c} is a stub ({})",
                            loc_s(&d.loc.file, d.loc.line)
                        ),
                        vec![(d.loc.file.clone(), d.loc.line), here],
                    ),
                    Handler::Missing => Outcome::new(
                        Word::Broken,
                        format!("{label} renders {c}, which is defined nowhere"),
                        vec![here],
                    ),
                    Handler::Unknown(why) => {
                        Outcome::new(Word::Skipped, format!("{label}: {why}"), vec![here])
                    }
                },
            });
        }
        if let Some(o) = best(found) {
            return Some(o);
        }
        let calls: Vec<Outcome> = self
            .facts
            .calls()
            .filter(|c| {
                c.target.kind == EndpointKind::Http
                    && paths_match(&c.target.name, path)
                    && methods_match(c.target.method.as_deref(), method)
            })
            .filter_map(|c| self.broken_call(c))
            .collect();
        if let Some(o) = worst(calls) {
            return Some(o);
        }
        if !local {
            return None;
        }
        let shown = format!(
            "{}{path}",
            method.map(|m| format!("{m} ")).unwrap_or_default()
        );
        Some(match &self.server {
            Ok(()) => Outcome::new(
                Word::NotBuilt,
                format!("no route, page or call for {shown} exists in the code"),
                Vec::new(),
            ),
            Err(why) => Outcome::new(
                Word::Skipped,
                format!("no route, page or call for {shown} was found; {why}"),
                Vec::new(),
            ),
        })
    }

    /// The word for a file that exists at `path`.
    pub fn eval_file(&self, path: &str, text: &str) -> Outcome {
        let here = vec![(path.to_string(), 1)];
        if let Some(i) = self.facts.incomplete.iter().find(|i| i.path == path) {
            return Outcome::new(
                Word::Skipped,
                format!("{path} could not be read completely: {}", i.reason),
                here,
            );
        }
        if is_unsupported_source(path) {
            return Outcome::new(
                Word::Skipped,
                format!(
                    "{path} is .{} source, which no extractor in this build reads",
                    paths::ext(path)
                ),
                here,
            );
        }
        let Some(ff) = self.facts.file(path) else {
            if lang_of(path).is_some() {
                return Outcome::new(
                    Word::Skipped,
                    format!("{path} was not read (ignored by git or over a limit)"),
                    here,
                );
            }
            return Outcome::new(
                Word::Connected,
                format!("{path} exists (not source code; nothing to wire)"),
                here,
            );
        };
        if !self.reads(ff.lang) {
            return Outcome::new(
                Word::Skipped,
                format!(
                    "{path} is {:?} source, and this build has no extractor for it yet",
                    ff.lang
                ),
                here,
            );
        }
        let mut found: Vec<Outcome> = Vec::new();
        for ep in &ff.endpoints {
            found.push(self.eval_endpoint(ep, text));
        }
        for c in &ff.calls {
            if let Some(o) = self.broken_call(c) {
                found.push(o);
            }
        }
        for u in &ff.ui {
            if let Some(o) = self.control_problem(u) {
                found.push(o);
            }
        }
        let fns: Vec<&Definition> = ff.defs.iter().filter(|d| is_fn_kind(d.kind)).collect();
        if !fns.is_empty() && fns.iter().all(|d| d.is_stub) {
            found.push(Outcome::new(
                Word::HalfBuilt,
                format!("every function in {path} is a stub"),
                vec![(path.to_string(), fns[0].loc.line)],
            ));
        }
        if ff.parse_errors {
            found.push(Outcome::new(
                Word::Skipped,
                format!("{path} has syntax the parser could not read"),
                here.clone(),
            ));
        }
        match worst(found) {
            Some(o) if o.word != Word::Connected => o,
            _ => Outcome::new(
                Word::Connected,
                format!(
                    "{path}: {} route(s) or command(s), {} call(s), {} control(s), {} definition(s) read; nothing broken",
                    ff.endpoints.len(),
                    ff.calls.len(),
                    ff.ui.len(),
                    ff.defs.len()
                ),
                here,
            ),
        }
    }
}

/// How a name from a claim is looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Name {
    Route { path: String, local: bool },
    File(String),
    Symbol(String),
}

fn classify(name: &str) -> Option<Name> {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let rest = n.split_once("//").map(|(_, r)| r).unwrap_or("");
        let host = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let local = host.starts_with("localhost")
            || host.starts_with("127.0.0.1")
            || host.starts_with("0.0.0.0")
            || host.starts_with("[::1]");
        return Some(Name::Route {
            path: normalize_path(n),
            local,
        });
    }
    if let Some(c) = QUOTED_CALL.captures(n) {
        return Some(Name::Symbol(c[1].to_string()));
    }
    if n.starts_with('/') && !n.starts_with("//") {
        if ABSOLUTE_PREFIXES.iter().any(|p| n.starts_with(p)) {
            return None;
        }
        let last = n.rsplit('/').next().unwrap_or("");
        if last.contains('.') {
            return None; // an absolute file path on the machine
        }
        return Some(Name::Route {
            path: normalize_path(n),
            local: true,
        });
    }
    if n.starts_with('~') || n.starts_with("..") || n.contains(":\\") || n.contains(":/") {
        return None;
    }
    let ident = |s: &str| {
        !s.is_empty()
            && !s.starts_with(|c: char| c.is_ascii_digit())
            && s.chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    };
    if let Some(call) = n.strip_suffix("()") {
        return call
            .split(['.', ':'])
            .filter(|s| !s.is_empty())
            .all(ident)
            .then(|| Name::Symbol(key(call).to_string()));
    }
    if n.contains("::") {
        return n
            .split("::")
            .all(ident)
            .then(|| Name::Symbol(key(n).to_string()));
    }
    if n.contains('/') || n.contains('\\') {
        return Some(Name::File(paths::normalize(n)));
    }
    if let Some((stem, ext)) = n.rsplit_once('.') {
        if lang_of(n).is_some()
            || is_unsupported_source(n)
            || matches!(
                ext.to_ascii_lowercase().as_str(),
                "md" | "json"
                    | "toml"
                    | "yml"
                    | "yaml"
                    | "html"
                    | "css"
                    | "sql"
                    | "txt"
                    | "lock"
                    | "sh"
                    | "ps1"
                    | "env"
                    | "ini"
                    | "cfg"
                    | "xml"
                    | "csv"
            )
        {
            return Some(Name::File(paths::normalize(n)));
        }
        if stem.split('.').all(ident) && ident(ext) {
            return Some(Name::Symbol(ext.to_string()));
        }
        return None;
    }
    ident(n).then(|| Name::Symbol(n.to_string()))
}

/// True for a name the claim writes as a code name on purpose: in backticks, snake_case, a path
/// (`a::b`) or a call. Plain CamelCase prose words ("TypeScript") are not.
fn explicit(text: &str, name: &str) -> bool {
    text.contains(&format!("`{name}`"))
        || name.contains('_')
        || name.contains("::")
        || name.contains('(')
}

/// The HTTP verb written right before the path in the claim, if any.
fn method_before(text: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"\b(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s+[`'"]?{}"#,
        regex::escape(name)
    ))
    .ok()?;
    re.captures(text).map(|c| c[1].to_string())
}

/// Files in the tree a claimed file name points at (exact first), and whether it names a folder.
fn find_files(ctx: &CheckContext, name: &str) -> (Vec<String>, bool) {
    let n = name.trim_end_matches('/');
    let lower = n.to_ascii_lowercase();
    let tree = ctx.tree();
    let mut exact = Vec::new();
    let mut other = Vec::new();
    let mut folder = false;
    for p in &tree.paths {
        if p.starts_with(".verdict/") {
            continue;
        }
        let pl = p.to_ascii_lowercase();
        if pl == lower {
            exact.push(p.clone());
        } else if pl.starts_with(&format!("{lower}/")) || pl.contains(&format!("/{lower}/")) {
            folder = true;
        } else if (!lower.contains('/') && paths::file_name(&pl) == lower)
            || (lower.contains('/') && pl.ends_with(&format!("/{lower}")))
        {
            other.push(p.clone());
        }
    }
    exact.extend(other);
    (exact, folder)
}

fn snippet(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= 80 {
        return one;
    }
    let cut: String = one.chars().take(77).collect();
    format!("{cut}...")
}

fn bounded(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 3).collect();
    format!("{cut}...")
}

/// One claim's word and what decided it.
struct Judged<'c> {
    claim: &'c ClaimRef,
    ticked: bool,
    name: String,
    outcome: Outcome,
}

impl Wiring {
    fn judge_name(
        &self,
        ctx: &CheckContext,
        idx: &Index,
        claim: &ClaimRef,
        name: &str,
    ) -> Option<Outcome> {
        let text = &claim.text;
        match classify(name)? {
            Name::Route { path, local } => {
                let method = method_before(text, name);
                idx.eval_route(&path, method.as_deref(), local, text)
            }
            Name::File(f) => {
                let (found, folder) = find_files(ctx, &f);
                if let Some(p) = found.first() {
                    return Some(idx.eval_file(p, text));
                }
                if folder {
                    return Some(Outcome::new(
                        Word::Connected,
                        format!("the folder {f} exists"),
                        Vec::new(),
                    ));
                }
                if NEGATIVE_VERB.is_match(text) {
                    return None;
                }
                if ctx.tree().truncated {
                    return Some(Outcome::new(
                        Word::Skipped,
                        format!("{f} was not found, but the file list stopped at its limit"),
                        Vec::new(),
                    ));
                }
                Some(Outcome::new(
                    Word::NotBuilt,
                    format!("{f} is not in the project"),
                    Vec::new(),
                ))
            }
            Name::Symbol(s) => {
                if s.chars().count() < 3 {
                    return None;
                }
                let out = idx.eval_symbol(&s, explicit(text, name), text)?;
                if out.word == Word::NotBuilt && NEGATIVE_VERB.is_match(text) {
                    return None;
                }
                Some(out)
            }
        }
    }
}

impl Checker for Wiring {
    fn name(&self) -> &'static str {
        "wiring"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Static]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        // Which items name anything at all (cheap, before reading the tree).
        let mut named: Vec<(&ClaimRef, Vec<String>)> = Vec::new();
        for c in &ctx.claims {
            let mut names = c.names.clone();
            for n in extract_names(&c.text) {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            names.retain(|n| classify(n).is_some());
            if !names.is_empty() {
                named.push((c, names));
            }
        }
        if named.is_empty() {
            return Err(Unavailable::new(
                "no claim, plan line or done-when item of this session names a URL, file or symbol to trace",
            ));
        }

        let facts = extract_tree(&ctx.root, &ScanOptions::default());
        let idx = Index::new(&facts, &ctx.tree().text);

        let mut judged: Vec<Judged> = Vec::new();
        for (claim, names) in named {
            let mut per_name: Vec<(String, Outcome)> = Vec::new();
            for n in &names {
                if let Some(o) = self.judge_name(ctx, &idx, claim, n) {
                    per_name.push((n.clone(), o));
                }
            }
            let mut pick: Option<(String, Outcome)> = None;
            for (n, o) in per_name {
                if pick.as_ref().is_none_or(|(_, b)| o.word > b.word) {
                    pick = Some((n, o));
                }
            }
            if let Some((name, outcome)) = pick {
                judged.push(Judged {
                    claim,
                    ticked: is_ticked(claim),
                    name,
                    outcome,
                });
            }
        }
        if judged.is_empty() {
            return Err(Unavailable::new(
                "the names in this session's claims are prose words, not code names; nothing to trace",
            ));
        }

        let mut counts: BTreeMap<Word, usize> = BTreeMap::new();
        for j in &judged {
            *counts.entry(j.outcome.word).or_default() += 1;
        }
        let unticked_bad = judged
            .iter()
            .filter(|j| !j.ticked && j.outcome.word.rule().is_some())
            .count();
        let mut count_line = WORD_ORDER
            .iter()
            .map(|w| format!("{} {}", counts.get(w).copied().unwrap_or(0), w.label()))
            .collect::<Vec<_>>()
            .join("  ");
        if unticked_bad > 0 {
            count_line.push_str(&format!(
                " ({unticked_bad} unticked, {} not block)",
                if unticked_bad == 1 { "does" } else { "do" }
            ));
        }

        let mut lines = vec![count_line.clone()];
        for j in &judged {
            lines.push(format!(
                "claim {} ({}): {} - {}: {}",
                j.claim.seq,
                if j.ticked { "ticked" } else { "unticked" },
                j.outcome.word.label(),
                j.name,
                bounded(&j.outcome.detail, 400)
            ));
        }
        if !idx.unread.is_empty() {
            let shown: Vec<&str> = idx.unread.iter().take(5).map(String::as_str).collect();
            lines.push(format!(
                "unverified: {} source file(s) in stacks no extractor in this build reads ({}{})",
                idx.unread.len(),
                shown.join(", "),
                if idx.unread.len() > 5 { ", ..." } else { "" }
            ));
        }

        let mut drafts: Vec<Draft> = Vec::new();
        for j in &judged {
            let Some(rule) = j.outcome.word.rule() else {
                continue;
            };
            let mut evidence = vec![ctx.claim_pointer(j.claim)];
            for (p, l) in j.outcome.at.iter().take(3) {
                evidence.push(ctx.line_pointer(p, u64::from(*l)));
            }
            drafts.push(Draft {
                rule,
                severity: if j.ticked {
                    Severity::Blocker
                } else {
                    Severity::Nit
                },
                title: format!(
                    "{}: \"{}\": {}",
                    j.outcome.word.label(),
                    snippet(&j.claim.text),
                    bounded(&j.outcome.detail, 400)
                ),
                claim_seq: Some(j.claim.seq),
                evidence,
                tier: Tier::Static,
                fix_hint: Some(j.outcome.word.hint().to_string()),
                path: format!("claim/{:012}", j.claim.seq),
                line: 0,
            });
        }

        let skipped = counts.get(&Word::Skipped).copied().unwrap_or(0);
        let settled = drafts.iter().any(|d| d.severity == Severity::Blocker);
        if skipped > 0 && !settled {
            return Err(Unavailable::new(format!(
                "{skipped} item(s) are SKIPPED, so wiring is not established: {}",
                lines.join("; ")
            )));
        }
        for l in lines {
            ctx.note(l);
        }
        Ok(finish(self.name(), drafts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_classified() {
        assert_eq!(
            classify("/api/notes"),
            Some(Name::Route {
                path: "/api/notes".into(),
                local: true
            })
        );
        assert_eq!(
            classify("http://localhost:3000/notes/1"),
            Some(Name::Route {
                path: "/notes/1".into(),
                local: true
            })
        );
        assert!(matches!(
            classify("https://example.com/x"),
            Some(Name::Route { local: false, .. })
        ));
        assert_eq!(classify("/home/me/notes.txt"), None);
        assert_eq!(classify("/tmp/x"), None);
        assert_eq!(
            classify("src/App.tsx"),
            Some(Name::File("src/App.tsx".into()))
        );
        assert_eq!(classify("README.md"), Some(Name::File("README.md".into())));
        assert_eq!(
            classify("save_note"),
            Some(Name::Symbol("save_note".into()))
        );
        assert_eq!(classify("notes::save()"), Some(Name::Symbol("save".into())));
        assert_eq!(
            classify("invoke(\"save_note\")"),
            Some(Name::Symbol("save_note".into()))
        );
        assert_eq!(classify("Notes.save"), Some(Name::Symbol("save".into())));
    }

    #[test]
    fn ticked_items() {
        let c = |source, text: &str| ClaimRef {
            seq: 1,
            source,
            text: text.into(),
            names: Vec::new(),
        };
        assert!(is_ticked(&c(ClaimSource::FinalMessage, "done")));
        assert!(is_ticked(&c(ClaimSource::DoneWhen, "x")));
        assert!(is_ticked(&c(ClaimSource::PlanLine, "- [x] save works")));
        assert!(is_ticked(&c(ClaimSource::PlanLine, "[X] save works")));
        assert!(!is_ticked(&c(ClaimSource::PlanLine, "- [ ] save works")));
        assert!(!is_ticked(&c(ClaimSource::PlanLine, "save works")));
    }

    #[test]
    fn worst_word_wins() {
        let o = |w| Outcome::new(w, "", Vec::new());
        let got = worst(vec![o(Word::Connected), o(Word::Broken), o(Word::Skipped)]).unwrap();
        assert_eq!(got.word, Word::Broken);
        let got = best(vec![o(Word::HalfBuilt), o(Word::Connected)]).unwrap();
        assert_eq!(got.word, Word::Connected);
    }

    #[test]
    fn method_is_read_from_the_claim() {
        assert_eq!(
            method_before("POST /api/notes saves a note", "/api/notes").as_deref(),
            Some("POST")
        );
        assert_eq!(method_before("the /api/notes route", "/api/notes"), None);
    }
}
