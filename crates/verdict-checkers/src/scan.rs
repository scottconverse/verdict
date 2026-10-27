//! `vdt scan` / `verdict_scan`: the wiring, regressions and unfinished checkers over the whole
//! tree, with no session and no claims, writing nothing to the project's ledger.
//!
//! Wiring with no claims cannot say NOT BUILT (nothing is named), so the scan lists what is wrong
//! in the tree itself, in the same five words:
//!
//! * BROKEN: every call that targets nothing (an HTTP call no route serves, an `invoke` no Tauri
//!   command serves, a server function that is not defined), and every route, command or control
//!   that names a handler defined nowhere.
//! * HALF BUILT: every route, server function or Tauri command nothing in the code calls, every
//!   button or form with no handler, and every stub (empty, `todo!()`, or only throws "not
//!   implemented") that something calls, renders or registers.
//! * SKIPPED: every source file in a stack no extractor in this build reads, and every item whose
//!   other end could be there. These are listed as unverified, never as clean.
//! * CONNECTED: calls that reach a defined, non-stub handler, and controls whose handler is
//!   defined. Counted, not listed.
//!
//! Regressions and unfinished run unchanged against a throwaway in-memory ledger whose session
//! has no baseline, so every line of every file counts as new: the scan reports every pattern in
//! the tree, not only this session's.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use verdict_core::timefmt::now_rfc3339;
use verdict_core::{Baseline, Finding, Lane, Ledger, Policy, Record, Writer};
use verdict_extract::{EndpointKind, Framework, ScanOptions, extract_tree};

use crate::Checker;
use crate::context::CheckContext;
use crate::regressions::Regressions;
use crate::unfinished::Unfinished;
use crate::wiring::{Index, Outcome, WORD_ORDER, Word};

/// One listed item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its word.
    pub word: Word,
    /// Plain words: what it is and why it got the word.
    pub detail: String,
    /// The most telling `file:line`.
    pub at: Option<(String, u32)>,
}

/// What a scan found.
#[derive(Debug, Clone)]
pub struct Scan {
    /// The project root, as given.
    pub root: String,
    /// Source files the extractors read.
    pub files_read: usize,
    /// Calls that target nothing, and handlers defined nowhere (BROKEN).
    pub broken: Vec<Item>,
    /// Routes, server functions and commands nothing calls (HALF BUILT).
    pub uncalled: Vec<Item>,
    /// Buttons and forms with no handler (HALF BUILT).
    pub no_handler: Vec<Item>,
    /// Stubs something calls, renders or registers (HALF BUILT).
    pub called_stubs: Vec<Item>,
    /// Items whose other end could be in an unread stack, and unread files (SKIPPED).
    pub skipped: Vec<Item>,
    /// Source files in stacks no extractor in this build reads.
    pub unread: Vec<String>,
    /// Number of CONNECTED items.
    pub connected: usize,
    /// The unfinished checker's findings, or why it could not run.
    pub unfinished: Result<Vec<Finding>, String>,
    /// The regressions checker's findings, or why it could not run.
    pub regressions: Result<Vec<Finding>, String>,
}

fn item(o: Outcome) -> Item {
    Item {
        word: o.word,
        at: o.at.into_iter().next(),
        detail: o.detail,
    }
}

/// Run the scan over `root`. `home` is where the user policy is read from (`None`: the user's
/// home folder). Nothing is written anywhere.
pub fn scan(root: &Path, home: Option<&Path>) -> Scan {
    let policy = Policy::load(root, home).or_else(|_| Policy::builtin()).ok();
    let ctx = policy.and_then(|p| throwaway_context(root, p).ok());

    let facts = extract_tree(root, &ScanOptions::default());
    let fallback_tree;
    let text = match &ctx {
        Some(c) => &c.tree().text,
        None => {
            fallback_tree = crate::tree::TreeIndex::build(root);
            &fallback_tree.text
        }
    };
    let idx = Index::new(&facts, text);

    let mut broken = Vec::new();
    let mut uncalled = Vec::new();
    let mut no_handler = Vec::new();
    let mut called_stubs = Vec::new();
    let mut skipped = Vec::new();
    let mut connected = 0usize;
    let mut seen: BTreeSet<(Word, String, u32)> = BTreeSet::new();
    let mut push = |list: &mut Vec<Item>, o: Outcome| {
        let at = o.at.first().cloned().unwrap_or_default();
        if seen.insert((o.word, at.0, at.1)) {
            list.push(item(o));
        }
    };

    // Calls that reach nothing.
    for c in facts.calls() {
        if let Some(o) = idx.broken_call(c) {
            match o.word {
                Word::Broken => push(&mut broken, o),
                _ => push(&mut skipped, o),
            }
        }
    }
    // Routes, server functions, commands and tools.
    for ep in facts.endpoints() {
        if ep.framework == Framework::OpenapiTs {
            continue; // the generated schema is not a route
        }
        let o = idx.eval_endpoint(ep, "");
        match o.word {
            Word::Broken => push(&mut broken, o),
            Word::HalfBuilt => push(&mut called_stubs, o),
            Word::Skipped => push(&mut skipped, o),
            Word::NotBuilt => {}
            Word::Connected => {
                let callable = matches!(
                    ep.key.kind,
                    EndpointKind::Http | EndpointKind::ServerFn | EndpointKind::TauriCommand
                );
                if callable && idx.callers_of(ep).is_empty() {
                    push(
                        &mut uncalled,
                        Outcome {
                            word: Word::HalfBuilt,
                            detail: format!("nothing in the code calls it: {}", o.detail),
                            at: o.at,
                        },
                    );
                } else {
                    connected += 1;
                }
            }
        }
    }
    // Buttons, forms and links.
    for u in facts.ui() {
        match idx.control_problem(u) {
            Some(o) if o.word == Word::Broken => push(&mut broken, o),
            Some(o) => push(&mut no_handler, o),
            None if u.kind != verdict_extract::UiKind::Link => connected += 1,
            None => {}
        }
    }
    // Stubs something uses.
    for d in facts.defs() {
        if d.is_stub && idx.is_used_def(d) {
            push(
                &mut called_stubs,
                Outcome {
                    word: Word::HalfBuilt,
                    detail: format!(
                        "{} ({}:{}) is a stub, and the code calls, renders or registers it",
                        d.name, d.loc.file, d.loc.line
                    ),
                    at: vec![(d.loc.file.clone(), d.loc.line)],
                },
            );
        }
    }
    for p in &idx.unread {
        push(
            &mut skipped,
            Outcome {
                word: Word::Skipped,
                detail: format!("{p}: a stack no extractor in this build reads"),
                at: vec![(p.clone(), 0)],
            },
        );
    }
    for i in &facts.incomplete {
        push(
            &mut skipped,
            Outcome {
                word: Word::Skipped,
                detail: format!("{}: not read completely: {}", i.path, i.reason),
                at: vec![(i.path.clone(), 0)],
            },
        );
    }

    let run = |c: &dyn Checker| -> Result<Vec<Finding>, String> {
        match &ctx {
            Some(ctx) => c.run(ctx).map_err(|u| u.reason),
            None => Err("the throwaway session could not be set up".to_string()),
        }
    };
    let unfinished = run(&Unfinished);
    let regressions = run(&Regressions);

    Scan {
        root: root.display().to_string(),
        files_read: facts.files.len(),
        broken,
        uncalled,
        no_handler,
        called_stubs,
        skipped,
        unread: idx.unread.clone(),
        connected,
        unfinished,
        regressions,
    }
}

/// A context over a throwaway in-memory ledger holding one session with no baseline. Nothing
/// touches the project's `.verdict` folder.
fn throwaway_context(root: &Path, policy: Policy) -> Result<CheckContext, String> {
    let mut ledger = Ledger::open_in_memory().map_err(|e| e.to_string())?;
    let id = verdict_core::session::new_session_id();
    ledger
        .append(
            Writer::Adapter,
            Some(&id),
            Record::Session {
                id: id.clone(),
                started_at: now_rfc3339(),
                baseline: Baseline {
                    hash: "scan:no-baseline".into(),
                    is_git_repo: false,
                    git: None,
                    note: Some("vdt scan: no session, every line counts".into()),
                    folder: None,
                },
                agent: "vdt".into(),
                model: "none".into(),
                vendor: "none".into(),
                policy_version: policy.version,
                policy_hash: policy.hash.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
    CheckContext::from_ledger(
        &ledger,
        root,
        &id,
        Lane::Standard,
        Duration::from_secs(60),
        policy,
    )
}

/// Drop the whole-file caveat the checkers add to titles: in a scan every file is whole on
/// purpose, and the header says so once.
fn plain_title(t: &str) -> &str {
    match t.find(" (whole-file scan:") {
        Some(i) => &t[..i],
        None => t,
    }
}

impl Scan {
    /// How many items got each word.
    pub fn count(&self, w: Word) -> usize {
        match w {
            Word::Connected => self.connected,
            Word::Broken => self.broken.len(),
            Word::NotBuilt => 0,
            Word::HalfBuilt => {
                self.uncalled.len() + self.no_handler.len() + self.called_stubs.len()
            }
            Word::Skipped => self.skipped.len(),
        }
    }

    /// The one-line count per word, the way the report prints wiring.
    pub fn count_line(&self) -> String {
        WORD_ORDER
            .iter()
            .map(|w| {
                let n = self.count(*w);
                if *w == Word::NotBuilt {
                    format!("{n} {} (nothing is named without a plan)", w.label())
                } else {
                    format!("{n} {}", w.label())
                }
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    fn headline(&self) -> &'static str {
        let unfinished = self.unfinished.as_ref().map(|f| f.len()).unwrap_or(0);
        let regressions = self.regressions.as_ref().map(|f| f.len()).unwrap_or(0);
        if !self.broken.is_empty() || regressions > 0 {
            "PROBLEMS FOUND"
        } else if self.count(Word::HalfBuilt) > 0 || unfinished > 0 {
            "LOOSE ENDS FOUND"
        } else if !self.skipped.is_empty() || self.unfinished.is_err() || self.regressions.is_err()
        {
            "PARTIAL - nothing proven wrong in what was read"
        } else {
            "NOTHING FOUND in what was read"
        }
    }

    /// The report, in the words and layout of `vdt report`.
    pub fn render(&self) -> String {
        let mut o = String::new();
        let line = |o: &mut String, s: String| {
            o.push_str(&s);
            o.push('\n');
        };
        line(
            &mut o,
            format!(
                "SCAN: {}  (no session, no claims; nothing written to the ledger)",
                self.headline()
            ),
        );
        line(
            &mut o,
            "read from code: done  |  your build and tests: not run  |  your real app: not run"
                .into(),
        );
        line(
            &mut o,
            format!(
                "project {}  |  {} source file(s) read  |  {} in stacks this build does not read",
                self.root,
                self.files_read,
                self.unread.len()
            ),
        );

        let section = |o: &mut String, title: &str, items: &[Item]| {
            o.push_str(&format!("\n{title} ({})\n", items.len()));
            if items.is_empty() {
                o.push_str("  (none)\n");
            }
            for (i, it) in items.iter().enumerate() {
                o.push_str(&format!(
                    "  {}. {}: {}\n",
                    i + 1,
                    it.word.label(),
                    it.detail
                ));
            }
        };
        section(&mut o, "BROKEN CALLS AND HANDLERS", &self.broken);
        section(&mut o, "ROUTES AND COMMANDS NOTHING CALLS", &self.uncalled);
        section(&mut o, "CONTROLS WITH NO HANDLER", &self.no_handler);
        section(&mut o, "CALLED STUBS", &self.called_stubs);

        let findings = |o: &mut String, title: &str, r: &Result<Vec<Finding>, String>| match r {
            Ok(f) => {
                o.push_str(&format!("\n{title} ({})\n", f.len()));
                if f.is_empty() {
                    o.push_str("  (none)\n");
                }
                for (i, f) in f.iter().enumerate() {
                    o.push_str(&format!(
                        "  {}. [{:?}] {} ({}{})\n",
                        i + 1,
                        f.severity,
                        plain_title(&f.title),
                        f.tier.as_str(),
                        f.evidence
                            .first()
                            .map(|e| format!(", {}", e.describe()))
                            .unwrap_or_default()
                    ));
                }
            }
            Err(why) => {
                o.push_str(&format!("\n{title}\n  could not run: {why}\n"));
            }
        };
        findings(&mut o, "UNFINISHED", &self.unfinished);
        findings(&mut o, "REGRESSIONS", &self.regressions);

        o.push_str("\nCHECKS\n");
        o.push_str(&format!(
            "  wiring: Ran, tier static: {}\n",
            self.count_line()
        ));
        for (name, r) in [
            ("unfinished", &self.unfinished),
            ("regressions", &self.regressions),
        ] {
            match r {
                Ok(f) => o.push_str(&format!(
                    "  {name}: Ran, tier static: {} finding(s); every line of every file read (no baseline)\n",
                    f.len()
                )),
                Err(why) => o.push_str(&format!("  {name}: Unavailable: {why}\n")),
            }
        }

        o.push_str("\nNOT ESTABLISHED\n");
        for it in &self.skipped {
            o.push_str(&format!("  - unverified: {}\n", it.detail));
        }
        o.push_str(
            "  - NOT BUILT: a scan has no plan or claims, so it cannot say what is missing\n",
        );
        o.push_str(
            "  - whether the code builds, its tests pass or the app runs: nothing was run\n",
        );
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_title_drops_the_whole_file_caveat() {
        assert_eq!(
            plain_title("TODO in a.rs:3 (whole-file scan: the baseline is a folder hash)"),
            "TODO in a.rs:3"
        );
        assert_eq!(plain_title("x"), "x");
    }
}
