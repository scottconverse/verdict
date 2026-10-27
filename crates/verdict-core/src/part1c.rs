//! Helpers the MCP server and the localhost page share (added in Part 1c).
//!
//! Nothing here writes to the ledger. It turns text and records into answers:
//!
//! * [`check_claim_line`] and [`check_done_when`] decide whether a line the agent sent is checkable
//!   enough to record, and say why not when it is not.
//! * [`open_decisions`] lists the human's open decisions, derived from the ledger alone.
//! * [`current_mode`] reads the shadow or enforce setting the human chose.
//! * [`one_line`] summarizes a record for the page's record log.

use crate::ledger::{Ledger, LedgerError};
use crate::model::{ClaimSource, DecisionKind, Record, Writer};

/// The session a caller means: the one asked for, else the most recent. `Ok(None)` when the
/// ledger has no session yet; `Err(Invalid)` when a named session does not exist.
pub fn resolve_session(
    ledger: &Ledger,
    wanted: Option<&str>,
) -> Result<Option<String>, LedgerError> {
    let mut latest = None;
    for s in ledger.records()? {
        if let Record::Session { id, .. } = &s.record {
            if wanted == Some(id.as_str()) {
                return Ok(Some(id.clone()));
            }
            latest = Some(id.clone());
        }
    }
    match wanted {
        Some(id) => Err(LedgerError::Invalid(format!(
            "no session {id} in this ledger"
        ))),
        None => Ok(latest),
    }
}

const FILE_EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "json", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "html", "htm", "css",
    "yml", "yaml", "sh", "ps1", "bat", "cmd", "sql", "lock", "txt", "go", "java", "kt", "c", "h",
    "cpp", "hpp", "cs", "rb", "php", "vue", "svelte", "ini", "cfg", "env", "xml", "csv", "log",
    "swift", "dart", "lua",
];

fn trim_punct(t: &str) -> &str {
    t.trim_matches(|c: char| {
        matches!(
            c,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '"'
                | '\''
                | '`'
        )
    })
}

fn is_url(t: &str) -> bool {
    let l = t.to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://")) && t.len() > 8
}

fn has_known_extension(t: &str) -> bool {
    let last = t.rsplit(['/', '\\']).next().unwrap_or(t);
    match last.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                && stem
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '@' | '+'))
        }
        None => false,
    }
}

fn is_file(t: &str) -> bool {
    if is_url(t) {
        return false;
    }
    let has_sep = t.contains('/') || t.contains('\\');
    if has_sep {
        let starts_pathy = t.starts_with("./")
            || t.starts_with("../")
            || t.starts_with('/')
            || t.starts_with("~/")
            || t.starts_with(".\\")
            || t.starts_with("..\\")
            || t.as_bytes().get(1) == Some(&b':');
        let has_alnum = t.chars().any(|c| c.is_alphanumeric());
        return has_alnum && (has_known_extension(t) || starts_pathy || t.ends_with(['/', '\\']));
    }
    has_known_extension(t)
}

fn is_symbol(t: &str) -> bool {
    if t.len() < 3 || is_url(t) {
        return false;
    }
    let ident = |s: &str| {
        !s.is_empty()
            && !s.starts_with(|c: char| c.is_ascii_digit())
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if t.contains("::") {
        return t.split("::").filter(|p| !p.is_empty()).all(ident) && t.split("::").count() >= 2;
    }
    if let Some(call) = t.strip_suffix("()") {
        return call.split('.').all(ident);
    }
    if !ident(t) {
        return false;
    }
    let snake = t.contains('_') && t.split('_').filter(|p| !p.is_empty()).count() >= 2;
    let humps = t.chars().filter(|c| c.is_uppercase()).count();
    let camel = t.starts_with(|c: char| c.is_alphabetic())
        && t.chars().any(|c| c.is_lowercase())
        && humps >= 2;
    let lower_camel = t.starts_with(|c: char| c.is_lowercase()) && humps >= 1;
    snake || camel || lower_camel
}

/// Pull the URLs, files and symbols a line names, in order of appearance, without repeats.
///
/// A URL starts with `http://` or `https://`. A file has a known extension or an unmistakable
/// path shape (`./x`, `src/`, `C:\x`). A symbol is `snake_case`, `CamelCase`, `a::b` or `call()`.
/// Anything in backticks without spaces also counts as a name.
pub fn extract_names(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut push = |n: &str| {
        let n = n.trim();
        if !n.is_empty() && !found.iter().any(|f| f == n) {
            found.push(n.to_string());
        }
    };
    let mut rest = text;
    let mut stripped = String::new();
    while let Some(start) = rest.find('`') {
        stripped.push_str(&rest[..start]);
        stripped.push(' ');
        let after = &rest[start + 1..];
        match after.find('`') {
            Some(end) => {
                let span = after[..end].trim();
                if !span.is_empty() && !span.contains(char::is_whitespace) {
                    push(span);
                }
                rest = &after[end + 1..];
            }
            None => {
                rest = "";
                break;
            }
        }
    }
    stripped.push_str(rest);
    for raw in stripped.split_whitespace() {
        let token = if is_url(raw.trim_matches(|c: char| matches!(c, '(' | '[' | '"' | '\''))) {
            raw.trim_matches(|c: char| matches!(c, '(' | '[' | '"' | '\''))
                .trim_end_matches(|c: char| {
                    matches!(
                        c,
                        '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '"' | '\''
                    )
                })
        } else {
            trim_punct(raw)
        };
        if token.is_empty() {
            continue;
        }
        if is_url(token) || is_file(token) || is_symbol(token) {
            push(token);
        }
    }
    found
}

/// Check one claim line before it is recorded. `given` is the names the agent says the line
/// names; each must appear in the line. With none given, the names are read from the line.
/// Returns the names to record, or the reason the line is refused.
pub fn check_claim_line(text: &str, given: &[String]) -> Result<Vec<String>, String> {
    let line = text.trim();
    if line.is_empty() {
        return Err("the claim line is empty".into());
    }
    if line.contains('\n') {
        return Err("send one checkable line per claim, not several lines".into());
    }
    if !given.is_empty() {
        let mut out: Vec<String> = Vec::new();
        for n in given {
            let n = n.trim();
            if n.is_empty() {
                return Err("a name in `names` is blank".into());
            }
            if !line.contains(n) {
                return Err(format!(
                    "the name {n:?} does not appear in the claim line; names must be words from the line itself"
                ));
            }
            if !out.iter().any(|o| o == n) {
                out.push(n.to_string());
            }
        }
        return Ok(out);
    }
    let names = extract_names(line);
    if names.is_empty() {
        return Err(
            "this line names no URL, file or symbol, so nothing could ever be checked \
                    against it; rewrite it to name one (for example a file path, a page URL, or \
                    a function name)"
                .into(),
        );
    }
    Ok(names)
}

const VAGUE: &[&str] = &[
    "better",
    "faster",
    "nicer",
    "cleaner",
    "easier",
    "prettier",
    "smoother",
    "improved",
    "improves",
    "improve",
    "good",
    "great",
    "fine",
    "well",
    "properly",
    "correctly",
    "right",
    "nice",
    "clean",
    "polished",
    "works",
    "working",
    "feels",
];

fn has_literal(line: &str) -> bool {
    line.chars().any(|c| c.is_ascii_digit())
        || line.contains('`')
        || line.matches('"').count() >= 2
        || line.matches('\'').count() >= 2
}

/// Check a "done when" line: how the human will know the work is done. A condition that cannot
/// be wrong is refused. Returns the names (URLs, files, symbols, backticked commands) to record.
pub fn check_done_when(text: &str) -> Result<Vec<String>, String> {
    let line = text.trim();
    if line.is_empty() {
        return Err("the done-when line is empty".into());
    }
    if line.contains('\n') {
        return Err("send one done-when line, not several lines".into());
    }
    let mut names = extract_names(line);
    // Backticked commands (with spaces) are concrete too.
    let mut rest = line;
    while let Some(s) = rest.find('`') {
        let after = &rest[s + 1..];
        let Some(e) = after.find('`') else { break };
        let span = after[..e].trim();
        if !span.is_empty() && !names.iter().any(|n| n == span) {
            names.push(span.to_string());
        }
        rest = &after[e + 1..];
    }
    let lower = line.to_lowercase();
    let vague_hit = lower
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| VAGUE.contains(w));
    if names.is_empty() && !has_literal(line) {
        return Err(format!(
            "{} cannot be wrong: it names no page, file, command, number or exact words a person \
             could look at. Say what the human will see, for example the URL and the text that \
             appears, or the command and its exact output.",
            if let Some(w) = vague_hit {
                format!("\"{w}\"")
            } else {
                "this condition".to_string()
            }
        ));
    }
    if let Some(w) = vague_hit
        && !has_literal(line)
        && matches!(
            w,
            "better"
                | "faster"
                | "nicer"
                | "cleaner"
                | "easier"
                | "prettier"
                | "smoother"
                | "improved"
                | "improves"
                | "improve"
        )
    {
        return Err(format!(
            "\"{w}\" has no measure, so the condition cannot be wrong. Say how much (a number or a \
             limit) or exactly what changes (exact text, a command's output)."
        ));
    }
    Ok(names)
}

/// A decision the human has not answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDecision {
    /// Stable key, for example `install_hooks:claude_code` or `accept_finding_not_real:F-1`.
    pub key: String,
    /// Which decision of the design's table.
    pub decision: DecisionKind,
    /// What is being asked, in plain words.
    pub question: String,
    /// What happens if nobody answers.
    pub default_if_unanswered: String,
    /// The ledger record it is about, if any.
    pub record_seq: Option<u64>,
}

/// Hosts whose hooks the human may install, as recorded in the decision's `choice`.
pub const HOOK_HOSTS: [&str; 2] = ["claude_code", "codex"];

/// The human's open decisions, derived from the ledger alone. `session` limits the
/// session-specific ones (prompt translations, findings) to one session; the project-wide ones
/// (install hooks) are always considered.
///
/// This is a read. Nothing here, and nothing in MCP, can answer a decision.
pub fn open_decisions(
    ledger: &Ledger,
    session: Option<&str>,
) -> Result<Vec<OpenDecision>, LedgerError> {
    let all = ledger.records()?;
    let answered = |kind: DecisionKind, choice: &str| {
        all.iter().any(|s| {
            matches!(&s.record, Record::Decision { decision, choice: c, who: Writer::Human, .. }
                if *decision == kind && c == choice)
        })
    };
    let mut out = Vec::new();
    for host in HOOK_HOSTS {
        if !answered(DecisionKind::InstallHooks, host) {
            out.push(OpenDecision {
                key: format!("install_hooks:{host}"),
                decision: DecisionKind::InstallHooks,
                question: format!("Install the {host} hooks in this project?"),
                default_if_unanswered: "not installed; recorded".into(),
                record_seq: None,
            });
        }
    }
    let in_scope = |s: &crate::ledger::Stored| match session {
        Some(id) => s.session_id.as_deref() == Some(id),
        None => true,
    };
    // Prompt text per session, to see whether a claim names something the human typed.
    for s in all.iter().filter(|s| in_scope(s)) {
        match &s.record {
            Record::Claim {
                source: ClaimSource::PromptTranslation,
                text,
                names,
                ..
            } => {
                let prompts: Vec<String> = all
                    .iter()
                    .filter(|p| p.session_id == s.session_id)
                    .filter_map(|p| match &p.record {
                        Record::Prompt { text, .. } => Some(text.to_lowercase()),
                        _ => None,
                    })
                    .collect();
                let foreign: Vec<&String> = names
                    .iter()
                    .filter(|n| {
                        let n = n.to_lowercase();
                        (is_url(&n) || is_file(&n)) && !prompts.iter().any(|p| p.contains(&n))
                    })
                    .collect();
                if !foreign.is_empty()
                    && !answered(DecisionKind::AcceptPromptTranslation, &s.seq.to_string())
                {
                    out.push(OpenDecision {
                        key: format!("accept_prompt_translation:{}", s.seq),
                        decision: DecisionKind::AcceptPromptTranslation,
                        question: format!(
                            "The agent's claim names {} which the prompt did not mention: \"{}\". Accept it?",
                            foreign
                                .iter()
                                .map(|n| n.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                            text
                        ),
                        default_if_unanswered: "the prompt stays NEEDS YOU".into(),
                        record_seq: Some(s.seq),
                    });
                }
            }
            Record::Finding(f) if !answered(DecisionKind::AcceptFindingNotReal, &f.id) => {
                out.push(OpenDecision {
                    key: format!("accept_finding_not_real:{}", f.id),
                    decision: DecisionKind::AcceptFindingNotReal,
                    question: format!("Accept finding \"{}\" as not real?", f.title),
                    default_if_unanswered: "the finding stays open".into(),
                    record_seq: Some(s.seq),
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// `shadow` unless the human's latest mode-switch decision says otherwise.
pub fn current_mode(ledger: &Ledger) -> Result<String, LedgerError> {
    let mut mode = "shadow".to_string();
    for s in ledger.records()? {
        if let Record::Decision {
            decision: DecisionKind::ModeSwitch,
            choice,
            who: Writer::Human,
            ..
        } = &s.record
        {
            mode = choice.clone();
        }
    }
    Ok(mode)
}

fn clip(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{cut}...")
    }
}

/// A one-line summary of a record, for the page's record log.
pub fn one_line(record: &Record) -> String {
    match record {
        Record::Session {
            agent,
            model,
            vendor,
            policy_version,
            ..
        } => format!("session opened: {agent}/{model} [{vendor}], policy v{policy_version}"),
        Record::Prompt { ordinal, text, .. } => format!("prompt {ordinal}: {}", clip(text, 160)),
        Record::Claim {
            source,
            text,
            names,
            ..
        } => format!(
            "claim ({}): {} [names: {}]",
            source.as_str(),
            clip(text, 160),
            names.join(", ")
        ),
        Record::ToolCall { tool, target, .. } => format!("tool call: {tool} {}", clip(target, 120)),
        Record::Check {
            name,
            tier,
            outcome,
            detail,
            ..
        } => format!(
            "check {name}: {outcome:?}, tier {}{}",
            tier.as_str(),
            detail
                .as_ref()
                .map(|d| format!(" ({})", clip(d, 120)))
                .unwrap_or_default()
        ),
        Record::Finding(f) => format!(
            "finding {} [{:?}]: {}",
            f.id,
            f.severity,
            clip(&f.title, 160)
        ),
        Record::Hypothesis(f) => format!(
            "hypothesis {} (no evidence, counts for nothing): {}",
            f.id,
            clip(&f.title, 160)
        ),
        Record::Decision {
            decision,
            choice,
            reason,
            ..
        } => format!(
            "human decision {} = {}{}",
            decision.as_str(),
            clip(choice, 80),
            reason
                .as_ref()
                .map(|r| format!(" (reason: {})", clip(r, 120)))
                .unwrap_or_default()
        ),
        Record::Verdict { word, .. } => format!("verdict {word:?}"),
        Record::RefusedWrite {
            attempted_by,
            attempted_kind,
            attempted_decision,
            reason,
            ..
        } => format!(
            "REFUSED: {attempted_by} tried to write a {attempted_kind}{}: {}",
            attempted_decision
                .map(|d| format!(" ({})", d.as_str()))
                .unwrap_or_default(),
            clip(reason, 160)
        ),
        Record::CheckUnavailable { component, reason } => {
            format!("check unavailable: {component}: {}", clip(reason, 160))
        }
        Record::Correction {
            target_seq, reason, ..
        } => format!("correction of record {target_seq}: {}", clip(reason, 160)),
    }
}
