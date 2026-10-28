//! What a checker is handed: the session's facts from the ledger, the policy, the baseline, and
//! lazy views of the working tree. A checker reads this and returns findings; it never writes
//! to the ledger (the runner does) and never calls another checker.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use verdict_core::exec_approval::{ApprovalChoice, commands_hash, parse_choice};
use verdict_core::{
    Baseline, ClaimSource, DecisionKind, EvidencePointer, Finding, Lane, Ledger, Policy, Record,
    Severity, Tier, Writer,
};

use crate::changes::{self, Changes, FileChange, Status};
use crate::tree::TreeIndex;

/// A checker could not run. The runner records `check_unavailable`, never a clean check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    /// Why, in plain words.
    pub reason: String,
}

impl Unavailable {
    /// Build one.
    pub fn new(reason: impl Into<String>) -> Self {
        Unavailable {
            reason: reason.into(),
        }
    }
}

/// A claim record of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRef {
    /// Its sequence number in the ledger.
    pub seq: u64,
    /// Where it came from.
    pub source: ClaimSource,
    /// The claim text.
    pub text: String,
    /// The URLs, files and symbols it names, as recorded.
    pub names: Vec<String>,
}

/// Where the human's decision about running the project's commands stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecApproval {
    /// The human has not decided.
    NotDecided,
    /// The human said no.
    Refused,
    /// An approval with no record of which commands it covered (older ledgers). Covers nothing.
    Unbound,
    /// The human approved a different set of commands than the policy lists now.
    Changed,
    /// The human approved exactly the commands the policy lists now.
    Current,
}

/// One command exec ran, kept as its own record in the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The command line.
    pub command: String,
    /// The tier it earns.
    pub tier: Tier,
    /// How it ended, in words (`exit 0`, `exit 3`, `timed out after 60s`).
    pub ended: String,
    /// Hash of both output streams in full.
    pub output_hash: String,
    /// The last bytes of output.
    pub tail: String,
    /// Anything wrong with the run beyond its exit code (output lost, processes left behind).
    pub problem: Option<String>,
}

/// Everything a checker may read.
pub struct CheckContext {
    /// The project root. Checkers never read above it.
    pub root: PathBuf,
    /// The session id.
    pub session: String,
    /// The lane being run.
    pub lane: Lane,
    /// The effective policy.
    pub policy: Policy,
    /// The session's baseline.
    pub baseline: Baseline,
    /// The session's claim records, in order.
    pub claims: Vec<ClaimRef>,
    /// The session's prompts, verbatim.
    pub prompts: Vec<String>,
    /// True when the human has approved running exactly the commands the policy lists now.
    pub exec_approved: bool,
    /// Where the human's decision stands (why `exec_approved` is what it is).
    pub exec_approval: ExecApproval,
    /// Sequence numbers of claims the human accepted with an `accept_prompt_translation`
    /// decision. Only these claims, never the agent's own words, can stand for something asked for.
    pub accepted_claims: BTreeSet<u64>,
    /// Longest any one command may run.
    pub timeout: Duration,
    changes: OnceCell<Result<Changes, String>>,
    tree: OnceCell<TreeIndex>,
    base_paths: OnceCell<Result<BTreeSet<String>, String>>,
    notes: RefCell<Vec<String>>,
    reached: Cell<Option<Tier>>,
    exec_tail: RefCell<Option<String>>,
    unestablished: RefCell<Vec<String>>,
    receipts: RefCell<Vec<Receipt>>,
    skipped: RefCell<Option<String>>,
}

impl CheckContext {
    /// Read the session's facts out of the ledger.
    pub fn from_ledger(
        ledger: &Ledger,
        root: &Path,
        session: &str,
        lane: Lane,
        timeout: Duration,
        policy: Policy,
    ) -> Result<CheckContext, String> {
        let records = ledger.records().map_err(|e| e.to_string())?;
        let mut baseline: Option<Baseline> = None;
        let mut claims = Vec::new();
        let mut prompts = Vec::new();
        let mut exec_choice: Option<ApprovalChoice> = None;
        let mut accepted_claims = BTreeSet::new();
        for s in &records {
            if let Record::Decision {
                decision,
                choice,
                who: Writer::Human,
                ..
            } = &s.record
            {
                match decision {
                    // Decisions are project-wide: the latest answer about the repo's commands
                    // stands, and an approval covers only the commands it names (by hash).
                    DecisionKind::RunProjectCommands => exec_choice = Some(parse_choice(choice)),
                    DecisionKind::AcceptPromptTranslation => {
                        if let Ok(n) = choice.trim().parse::<u64>() {
                            accepted_claims.insert(n);
                        }
                    }
                    _ => {}
                }
            }
            if s.session_id.as_deref() != Some(session) {
                continue;
            }
            match &s.record {
                Record::Session { baseline: b, .. } => baseline = Some(b.clone()),
                Record::Claim {
                    source,
                    text,
                    names,
                    ..
                } => claims.push(ClaimRef {
                    seq: s.seq,
                    source: *source,
                    text: text.clone(),
                    names: names.clone(),
                }),
                Record::Prompt { text, .. } => prompts.push(text.clone()),
                _ => {}
            }
        }
        let baseline = baseline.ok_or_else(|| format!("no session {session} in this ledger"))?;
        let exec_approval = match exec_choice {
            None => ExecApproval::NotDecided,
            Some(ApprovalChoice::Refused) => ExecApproval::Refused,
            Some(ApprovalChoice::Unbound) => ExecApproval::Unbound,
            Some(ApprovalChoice::Approved(h)) if h == commands_hash(&policy) => {
                ExecApproval::Current
            }
            Some(ApprovalChoice::Approved(_)) => ExecApproval::Changed,
        };
        Ok(CheckContext {
            root: root.to_path_buf(),
            session: session.to_string(),
            lane,
            policy,
            baseline,
            claims,
            prompts,
            exec_approved: exec_approval == ExecApproval::Current,
            exec_approval,
            accepted_claims,
            timeout,
            changes: OnceCell::new(),
            tree: OnceCell::new(),
            base_paths: OnceCell::new(),
            notes: RefCell::new(Vec::new()),
            reached: Cell::new(None),
            exec_tail: RefCell::new(None),
            unestablished: RefCell::new(Vec::new()),
            receipts: RefCell::new(Vec::new()),
            skipped: RefCell::new(None),
        })
    }

    /// The commit the diff is taken against, when the baseline is a git repository.
    pub fn base_commit(&self) -> Option<String> {
        let git = self.baseline.git.as_ref()?;
        Some(
            git.head_commit
                .clone()
                .unwrap_or_else(|| changes::EMPTY_TREE.to_string()),
        )
    }

    /// The policy's `exclude` list.
    pub fn exclude(&self) -> Vec<String> {
        self.policy
            .value
            .get("exclude")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The working tree index (walks the project folder once).
    pub fn tree(&self) -> &TreeIndex {
        self.tree.get_or_init(|| TreeIndex::build(&self.root))
    }

    /// Read a working-tree file as text (`None` if missing or not text).
    pub fn read_text(&self, path: &str) -> Option<String> {
        if let Some(t) = self.tree().text.get(path) {
            return Some(t.clone());
        }
        let bytes = std::fs::read(self.root.join(path)).ok()?;
        if bytes.contains(&0) {
            return None;
        }
        Some(String::from_utf8_lossy(&bytes).to_string())
    }

    /// What changed since the baseline. Computed once.
    pub fn changes(&self) -> Result<&Changes, Unavailable> {
        self.changes
            .get_or_init(|| self.compute_changes())
            .as_ref()
            .map_err(|e| Unavailable::new(e.clone()))
    }

    fn compute_changes(&self) -> Result<Changes, String> {
        let mut caveats = Vec::new();
        if let Some(git) = &self.baseline.git {
            let base = self.base_commit().unwrap_or_default();
            if git.uncommitted_files > 0 {
                caveats.push(format!(
                    "the baseline already had {} uncommitted file(s); their lines cannot be \
                     told apart from this session's and count as this session's",
                    git.uncommitted_files
                ));
            }
            let mut c = changes::git_changes(&self.root, &base)?;
            c.caveats = caveats;
            return Ok(c);
        }
        // Folder baseline: a content hash only.
        let now = verdict_core::baseline_snapshot(&self.root);
        if now.hash == self.baseline.hash {
            return Ok(Changes {
                files: Vec::new(),
                line_level: true,
                baseline_known: true,
                caveats: vec!["the folder still hashes the same as at the baseline".into()],
            });
        }
        caveats.push(
            "the baseline is a folder hash with no per-file data: line-level diff was not \
             possible, so whole files were compared and every line counts as added"
                .to_string(),
        );
        let mut files = Vec::new();
        for (path, text) in &self.tree().text {
            if path.starts_with(".verdict/") {
                continue;
            }
            files.push(FileChange {
                path: path.clone(),
                status: Status::Unknown,
                added: changes::numbered_lines(text),
                removed: Vec::new(),
                binary: false,
            });
        }
        Ok(Changes {
            files,
            line_level: false,
            baseline_known: false,
            caveats,
        })
    }

    /// Paths present at the baseline commit. Needs a git baseline.
    pub fn baseline_paths(&self) -> Result<&BTreeSet<String>, Unavailable> {
        let base = self.base_commit().ok_or_else(|| {
            Unavailable::new("the baseline is a folder hash: it holds no list of its files")
        })?;
        self.base_paths
            .get_or_init(|| changes::baseline_paths(&self.root, &base))
            .as_ref()
            .map_err(|e| Unavailable::new(e.clone()))
    }

    /// A file's text as it was at the baseline commit.
    pub fn baseline_text(&self, path: &str) -> Option<String> {
        changes::baseline_text(&self.root, &self.base_commit()?, path)
    }

    /// Record a plain-words note for the check's detail (what could not be read, caveats).
    pub fn note(&self, text: impl Into<String>) {
        let t = text.into();
        let mut n = self.notes.borrow_mut();
        if !n.contains(&t) {
            n.push(t);
        }
    }

    /// The notes recorded so far.
    pub fn notes(&self) -> Vec<String> {
        self.notes.borrow().clone()
    }

    /// Say which tier this run actually reached (the runner records it on the check).
    pub fn reached(&self, tier: Tier) {
        self.reached.set(Some(tier));
    }

    /// The tier set by [`CheckContext::reached`].
    pub fn reached_tier(&self) -> Option<Tier> {
        self.reached.get()
    }

    /// Keep a bounded tail of command output for the check's detail.
    pub fn set_exec_tail(&self, text: String) {
        *self.exec_tail.borrow_mut() = Some(text);
    }

    /// The tail set by [`CheckContext::set_exec_tail`].
    pub fn exec_tail(&self) -> Option<String> {
        self.exec_tail.borrow().clone()
    }

    /// A `file:line` pointer for the line as it is now. If the file or line is gone, the nearest
    /// existing line; if the file is empty or gone, a command pointer that says what was read.
    pub fn line_pointer(&self, path: &str, line: u64) -> EvidencePointer {
        if let Ok(p) = EvidencePointer::file_line(&self.root, path, line.max(1)) {
            return p;
        }
        if let Some(text) = self.read_text(path) {
            let n = text.lines().count() as u64;
            if n > 0
                && let Ok(p) = EvidencePointer::file_line(&self.root, path, line.clamp(1, n))
            {
                return p;
            }
        }
        EvidencePointer::command(&format!("read {path}:{line}"), b"")
    }

    /// A pointer to something git saw change, for evidence that no longer exists in the tree
    /// (a deleted file). The hash covers the status line.
    pub fn diff_pointer(&self, status_line: &str) -> EvidencePointer {
        EvidencePointer::command(
            "git diff --name-status (session baseline)",
            status_line.as_bytes(),
        )
    }

    /// A pointer to a claim record: the ledger record and the hash of its text.
    pub fn claim_pointer(&self, claim: &ClaimRef) -> EvidencePointer {
        EvidencePointer::command(
            &format!("ledger record {}", claim.seq),
            claim.text.as_bytes(),
        )
    }
}

/// A finding before it has an id.
#[derive(Debug, Clone)]
pub struct Draft {
    /// Short rule name; becomes part of the id.
    pub rule: &'static str,
    /// Severity. `Blocker` is what blocks; everything else does not.
    pub severity: Severity,
    /// Plain-words title.
    pub title: String,
    /// The claim record it bears on.
    pub claim_seq: Option<u64>,
    /// Evidence pointers (at least one).
    pub evidence: Vec<EvidencePointer>,
    /// Tier.
    pub tier: Tier,
    /// How to fix it.
    pub fix_hint: Option<String>,
    /// Where it is, for stable ordering.
    pub path: String,
    /// Line, for stable ordering.
    pub line: u64,
}

/// Turn drafts into findings: stable order, content-derived ids (the same inputs give the same
/// ids), duplicates folded.
pub fn finish(checker: &str, mut drafts: Vec<Draft>) -> Vec<Finding> {
    drafts.sort_by(|a, b| {
        (&a.path, a.line, a.rule, &a.title).cmp(&(&b.path, b.line, b.rule, &b.title))
    });
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for d in drafts {
        let mut h = blake3::Hasher::new();
        h.update(d.rule.as_bytes());
        h.update(b"\0");
        h.update(d.path.as_bytes());
        h.update(b"\0");
        h.update(&d.line.to_le_bytes());
        h.update(d.title.as_bytes());
        let hex = h.finalize().to_hex().to_string();
        // `<checker>:` prefix: the core reads the checker from it (verdict_core::verdict::checker_of).
        let id = format!("{checker}:{}.{}", d.rule, &hex[..8]);
        if !seen.insert(id.clone()) {
            continue;
        }
        out.push(Finding {
            id,
            severity: d.severity,
            title: d.title,
            claim_seq: d.claim_seq,
            evidence: d.evidence,
            tier: d.tier,
            blast: None,
            judgment: false,
            fix_hint: d.fix_hint,
        });
    }
    out
}

impl CheckContext {
    /// Forget the notes, tier and output tail of the previous checker.
    pub fn reset_run_state(&self) {
        self.notes.borrow_mut().clear();
        self.reached.set(None);
        *self.exec_tail.borrow_mut() = None;
        self.unestablished.borrow_mut().clear();
        self.receipts.borrow_mut().clear();
        *self.skipped.borrow_mut() = None;
    }

    /// Say that policy did not require this check in this run. The runner records
    /// `skipped_by_policy`, never `ran`: not being required is not execution proof.
    pub fn skipped(&self, reason: impl Into<String>) {
        *self.skipped.borrow_mut() = Some(reason.into());
    }

    /// The reason [`CheckContext::skipped`] recorded, if any.
    pub fn skip_reason(&self) -> Option<String> {
        self.skipped.borrow().clone()
    }

    /// Say that a lookup could not be established (a search that was cut short, a file that could
    /// not be read). The runner then records `check_unavailable` for the checker, never a clean
    /// check, even when other findings were written.
    pub fn unestablished(&self, reason: impl Into<String>) {
        let r = reason.into();
        let mut u = self.unestablished.borrow_mut();
        if !u.contains(&r) {
            u.push(r);
        }
    }

    /// The reasons recorded by [`CheckContext::unestablished`].
    pub fn unestablished_reasons(&self) -> Vec<String> {
        self.unestablished.borrow().clone()
    }

    /// Keep a receipt for one command that ran; the runner writes each to the ledger.
    pub fn add_receipt(&self, r: Receipt) {
        self.receipts.borrow_mut().push(r);
    }

    /// The receipts recorded so far.
    pub fn receipts(&self) -> Vec<Receipt> {
        self.receipts.borrow().clone()
    }

    /// Read the tree and the diff now, so every checker in a run sees the same view (commands
    /// that exec runs cannot change what later checkers read).
    pub fn warm(&self) {
        let _ = self.tree();
        let _ = self.changes();
    }

    /// A digest of exactly what the warmed tree index holds: every cached path with the hash of
    /// the bytes it cached. `from_disk` computes it again from the working tree instead, so the
    /// two digests answer "is the cache still this tree's bytes?".
    pub fn tree_digest(&self, from_disk: bool) -> String {
        let mut h = blake3::Hasher::new();
        for (path, cached) in &self.tree().text {
            h.update(path.as_bytes());
            h.update(&[0]);
            let bytes = if from_disk {
                std::fs::read(self.root.join(path)).ok()
            } else {
                Some(cached.as_bytes().to_vec())
            };
            match bytes {
                Some(b) => h.update(blake3::hash(&b).as_bytes()),
                // A cached file the disk no longer has cannot be the same snapshot.
                None => h.update(&[0xff]),
            };
            h.update(&[0]);
        }
        h.finalize().to_hex().to_string()
    }

    /// True when the cache holds exactly the working tree's bytes. A tree that changed (and even
    /// one that changed and changed back) is only equal when the cached bytes are the tree's bytes
    /// now, which is what the recorded snapshot stamp must describe.
    pub fn cache_matches_tree(&self) -> bool {
        self.tree_digest(false) == self.tree_digest(true)
    }

    /// Hash of everything a checker could have read: the baseline, the policy, the diff, the
    /// tree's text, the claims, the prompts and the exec approval.
    pub fn inputs_hash(&self, checker: &str) -> String {
        let mut h = blake3::Hasher::new();
        h.update(b"verdict-checker-inputs-v1\n");
        for part in [
            checker,
            &self.baseline.hash,
            &self.policy.hash,
            &format!("{:?}", self.lane),
        ] {
            h.update(part.as_bytes());
            h.update(b"\0");
        }
        if let Some(Ok(c)) = self.changes.get() {
            h.update(c.fingerprint().as_bytes());
        }
        h.update(b"\0tree\0");
        for (path, text) in &self.tree().text {
            h.update(path.as_bytes());
            h.update(blake3::hash(text.as_bytes()).as_bytes());
        }
        h.update(b"\0claims\0");
        for c in &self.claims {
            h.update(&c.seq.to_le_bytes());
            h.update(c.text.as_bytes());
        }
        h.update(b"\0prompts\0");
        for p in &self.prompts {
            h.update(p.as_bytes());
            h.update(b"\0");
        }
        h.update(&[self.exec_approved as u8]);
        h.finalize().to_hex().to_string()
    }
}
