//! claims_vs_code: what the agent said, looked up in the tree.
//!
//! Every claim from the final message, a commit message or a notes file that names a file or a
//! symbol is checked against the working tree. A contradiction blocks (severity `Blocker`):
//!
//! * `missing-file`: the claim names a file that is not in the tree
//! * `missing-symbol`: the claim names a symbol that appears in no source file
//! * `still-exists`: the claim says a file was removed or renamed, and it is still there
//! * `still-defined`: the claim says a symbol was removed, and it is still defined
//!
//! URLs are not resolved (no network, and no route extraction until the tree-sitter extractor
//! exists); the check's detail says so. Names are read with `verdict_core::part1c::extract_names`.

use std::sync::LazyLock;

use regex::Regex;
use verdict_core::part1c::extract_names;
use verdict_core::{ClaimSource, Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, ClaimRef, Draft, Unavailable, finish};
use crate::paths;

/// The claims_vs_code checker.
pub struct ClaimsVsCode;

static NEGATIVE_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:delet\w*|remov\w*|drop(?:ped|ping)?|renam\w*|moved?|moving|replac\w*|deprecat\w*|got rid)\b")
        .expect("static regex")
});

const FILE_EXTS: &[&str] = &[
    "rs", "toml", "md", "json", "js", "mjs", "cjs", "ts", "tsx", "jsx", "py", "html", "htm", "css",
    "yml", "yaml", "sh", "ps1", "bat", "cmd", "sql", "lock", "txt", "go", "java", "kt", "c", "h",
    "cpp", "hpp", "cs", "rb", "php", "vue", "svelte", "ini", "cfg", "env", "xml", "csv", "log",
    "swift", "dart", "lua",
];

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Url,
    File,
    Symbol,
    Other,
}

fn classify(name: &str) -> Kind {
    let l = name.to_ascii_lowercase();
    if l.starts_with("http://") || l.starts_with("https://") {
        return Kind::Url;
    }
    let ident = |s: &str| {
        !s.is_empty()
            && !s.starts_with(|c: char| c.is_ascii_digit())
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if let Some(call) = name.strip_suffix("()") {
        return if call.split('.').all(ident) {
            Kind::Symbol
        } else {
            Kind::Other
        };
    }
    if name.contains("::") {
        return if name.split("::").all(ident) {
            Kind::Symbol
        } else {
            Kind::Other
        };
    }
    if name.ends_with(['/', '\\']) {
        return Kind::File;
    }
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if let Some((stem, ext)) = last.rsplit_once('.')
        && !stem.is_empty()
        && FILE_EXTS.contains(&ext.to_ascii_lowercase().as_str())
    {
        return Kind::File;
    }
    if name.contains('/') || name.contains('\\') {
        return Kind::File;
    }
    if ident(name) {
        return Kind::Symbol;
    }
    Kind::Other
}

/// The identifier to search for: the last segment of `a::b`, `a.b()`, `call()`.
fn symbol_ident(name: &str) -> String {
    let n = name.strip_suffix("()").unwrap_or(name);
    n.rsplit("::")
        .next()
        .unwrap_or(n)
        .rsplit('.')
        .next()
        .unwrap_or(n)
        .to_string()
}

/// Does the claim direct a removal or rename at `name` ("removed old.txt", "deleted the file
/// `old.txt`")?
fn directed_removal(text: &str, name: &str) -> bool {
    let pat = format!(
        r#"(?i)\b(?:delet\w*|remov\w*|drop(?:ped|ping)?|renam\w*)\s+(?:[\w-]+\s+){{0,3}}[`'"]?{}"#,
        regex::escape(name)
    );
    Regex::new(&pat).is_ok_and(|re| re.is_match(text))
}

fn file_exists(ctx: &CheckContext, name: &str) -> bool {
    let n = paths::normalize(name);
    let n = n.trim_end_matches('/');
    let tree = ctx.tree();
    if n.ends_with('.') || n.is_empty() {
        return false;
    }
    if tree.has_path(n) {
        return true;
    }
    let lower = n.to_ascii_lowercase();
    tree.paths.iter().any(|p| {
        let pl = p.to_ascii_lowercase();
        pl == lower
            || pl.starts_with(&format!("{lower}/"))
            || (!lower.contains('/') && paths::file_name(&pl) == lower)
            || (lower.contains('/') && pl.ends_with(&format!("/{lower}")))
    })
}

fn is_source_for_symbols(path: &str) -> bool {
    paths::is_code(path)
        || matches!(
            paths::ext(path).as_str(),
            "html" | "htm" | "css" | "json" | "toml" | "yaml" | "yml"
        )
}

/// First `(path, line)` where the identifier appears as a whole word in a source file.
fn find_symbol(ctx: &CheckContext, ident: &str) -> Option<(String, u64)> {
    let re = Regex::new(&format!(r"\b{}\b", regex::escape(ident))).ok()?;
    for (path, text) in &ctx.tree().text {
        if !is_source_for_symbols(path) || paths::is_noise_path(path) {
            continue;
        }
        for (i, l) in text.lines().enumerate() {
            if re.is_match(l) {
                return Some((path.clone(), i as u64 + 1));
            }
        }
    }
    None
}

/// First place the identifier is defined.
fn find_definition(ctx: &CheckContext, ident: &str) -> Option<(String, u64)> {
    let re = Regex::new(&format!(
        r"\b(?:fn|def|function|class|struct|enum|trait|interface|type|const|let|var|func|impl)\s+{}\b",
        regex::escape(ident)
    ))
    .ok()?;
    for (path, text) in &ctx.tree().text {
        if !is_source_for_symbols(path) || paths::is_noise_path(path) {
            continue;
        }
        for (i, l) in text.lines().enumerate() {
            if re.is_match(l) {
                return Some((path.clone(), i as u64 + 1));
            }
        }
    }
    None
}

impl Checker for ClaimsVsCode {
    fn name(&self) -> &'static str {
        "claims_vs_code"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Static]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let claims: Vec<&ClaimRef> = ctx
            .claims
            .iter()
            .filter(|c| {
                matches!(
                    c.source,
                    ClaimSource::FinalMessage | ClaimSource::CommitMessage | ClaimSource::NotesFile
                )
            })
            .collect();
        if claims.is_empty() {
            return Err(Unavailable::new(
                "this session has no final-message, commit-message or notes-file claims to look up",
            ));
        }
        ctx.note(
            "unverified: URLs are not resolved (no network, no route extraction yet); \
             names that start with / or point above the project are not looked up",
        );
        let mut drafts: Vec<Draft> = Vec::new();
        for claim in claims {
            let mut names = claim.names.clone();
            for n in extract_names(&claim.text) {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            let any_negative = NEGATIVE_VERB.is_match(&claim.text);
            for name in &names {
                match classify(name) {
                    Kind::File => {
                        let n = name.trim();
                        if n.starts_with('/')
                            || n.starts_with('~')
                            || n.starts_with("..")
                            || n.contains(":\\")
                            || n.contains(":/")
                        {
                            continue;
                        }
                        let exists = file_exists(ctx, n);
                        let directed = directed_removal(&claim.text, name);
                        if !exists && !any_negative {
                            drafts.push(self.draft(
                                ctx,
                                claim,
                                "missing-file",
                                format!("the claim names {name}, which is not in the project"),
                                "create the file or correct the claim",
                                None,
                            ));
                        } else if exists && directed {
                            let at = ctx
                                .tree()
                                .paths
                                .iter()
                                .find(|p| {
                                    paths::normalize(p).eq_ignore_ascii_case(&paths::normalize(n))
                                })
                                .cloned();
                            let ptr = at.map(|p| ctx.line_pointer(&p, 1));
                            drafts.push(self.draft(
                                ctx,
                                claim,
                                "still-exists",
                                format!("the claim says {name} was removed or renamed, but it is still in the project"),
                                "remove it or correct the claim",
                                ptr,
                            ));
                        }
                    }
                    Kind::Symbol => {
                        let ident = symbol_ident(name);
                        if ident.len() < 3 {
                            continue;
                        }
                        let found = find_symbol(ctx, &ident);
                        let directed = directed_removal(&claim.text, name)
                            || directed_removal(&claim.text, &ident);
                        match found {
                            None if !any_negative && !ctx.tree().truncated => {
                                drafts.push(self.draft(
                                    ctx,
                                    claim,
                                    "missing-symbol",
                                    format!(
                                        "the claim names {name}, which appears in no source file"
                                    ),
                                    "write it or correct the claim",
                                    None,
                                ));
                            }
                            Some(_) if directed => {
                                if let Some((p, l)) = find_definition(ctx, &ident) {
                                    drafts.push(self.draft(
                                        ctx,
                                        claim,
                                        "still-defined",
                                        format!("the claim says {name} was removed, but it is still defined at {p}:{l}"),
                                        "remove it or correct the claim",
                                        Some(ctx.line_pointer(&p, l)),
                                    ));
                                }
                            }
                            _ => {}
                        }
                    }
                    Kind::Url | Kind::Other => {}
                }
            }
        }
        Ok(finish(self.name(), drafts))
    }
}

impl ClaimsVsCode {
    fn draft(
        &self,
        ctx: &CheckContext,
        claim: &ClaimRef,
        rule: &'static str,
        title: String,
        hint: &str,
        extra: Option<verdict_core::EvidencePointer>,
    ) -> Draft {
        let mut evidence = vec![ctx.claim_pointer(claim)];
        evidence.extend(extra);
        Draft {
            rule,
            severity: Severity::Blocker,
            title,
            claim_seq: Some(claim.seq),
            evidence,
            tier: Tier::Static,
            fix_hint: Some(hint.to_string()),
            path: format!("claim/{:012}", claim.seq),
            line: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_classified() {
        assert_eq!(classify("https://x.dev/a"), Kind::Url);
        assert_eq!(classify("src/lib.rs"), Kind::File);
        assert_eq!(classify("README.md"), Kind::File);
        assert_eq!(classify("charge_card"), Kind::Symbol);
        assert_eq!(classify("Billing::charge"), Kind::Symbol);
        assert_eq!(classify("run()"), Kind::Symbol);
        assert_eq!(classify("obj.method"), Kind::Other);
    }

    #[test]
    fn removal_must_be_aimed_at_the_name() {
        assert!(directed_removal(
            "removed old.txt and added new.txt",
            "old.txt"
        ));
        assert!(!directed_removal(
            "removed old.txt and added new.txt",
            "new.txt"
        ));
        assert!(directed_removal("deleted the unused file `a.rs`", "a.rs"));
    }

    #[test]
    fn symbol_identifiers() {
        assert_eq!(symbol_ident("Billing::charge"), "charge");
        assert_eq!(symbol_ident("run()"), "run");
        assert_eq!(symbol_ident("charge_card"), "charge_card");
    }
}
