//! release: release-truth checks, from the project's own configuration.
//!
//! Everything here is configured; nothing assumes a web layout, a manifest name or a release
//! script. The policy's `release` block may name:
//!
//! * `tag` — the tag this candidate must carry. Every tag pointing at HEAD is listed; a missing one
//!   blocks at release.
//! * `version` and `version_files` — the version the candidate claims, and the files that must
//!   state it (a manifest line such as `version = "1.0.0"`). A mismatch blocks.
//! * `assets` — paths or simple globs (`dist/`, `dist/*`, `target/*.zip`) that must exist.
//! * `readme` — a Markdown file whose relative links must resolve.
//! * `checks` — commands from the project's release workflow, under the same human approval as
//!   every other project command; a failing check blocks.
//!
//! A release block with no configuration is `Unavailable`, never clean. The tag check cannot pass
//! before the human creates the tag: that is exactly why the release lane is the last gate.

use std::path::Path;

use regex::Regex;
use serde_json::Value;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::exec::run_command;

/// The release checker.
pub struct Release;

#[derive(Default)]
struct Config {
    tag: Option<String>,
    version: Option<String>,
    version_files: Vec<String>,
    assets: Vec<String>,
    readme: Option<String>,
    checks: Vec<String>,
}

fn strings(policy: &Policy, pointer: &str) -> Vec<String> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn string(policy: &Policy, pointer: &str) -> Option<String> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn config(policy: &Policy) -> Config {
    Config {
        tag: string(policy, "/release/tag"),
        version: string(policy, "/release/version"),
        version_files: strings(policy, "/release/version_files"),
        assets: strings(policy, "/release/assets"),
        readme: string(policy, "/release/readme"),
        checks: strings(policy, "/release/checks"),
    }
}

impl Checker for Release {
    fn name(&self) -> &'static str {
        "release"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Asset, Tier::Static, Tier::Ran]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let cfg = config(&ctx.policy);
        if cfg.tag.is_none()
            && cfg.version.is_none()
            && cfg.assets.is_empty()
            && cfg.readme.is_none()
            && cfg.checks.is_empty()
        {
            return Err(Unavailable::new(
                "no release checks are configured (set release.tag, release.version_files, \
                 release.assets, release.readme or release.checks)",
            ));
        }
        if cfg.version.is_some() && cfg.version_files.is_empty() {
            return Err(Unavailable::new(
                "release.version is configured but release.version_files is empty, so the version \
                 cannot be checked against any file",
            ));
        }
        let mut drafts: Vec<Draft> = Vec::new();

        if let Some(tag) = &cfg.tag {
            let at_head = crate::changes::git(&ctx.root, &["tag", "--points-at", "HEAD"])
                .map(|b| String::from_utf8_lossy(&b).to_string())
                .unwrap_or_default();
            let tags: Vec<&str> = at_head
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            ctx.note(format!(
                "release tags on HEAD: {}",
                if tags.is_empty() {
                    "none".to_string()
                } else {
                    tags.join(", ")
                }
            ));
            if !tags.iter().any(|t| *t == tag) {
                drafts.push(Draft {
                    rule: "tag-missing",
                    severity: Severity::Blocker,
                    title: format!(
                        "the release candidate is not tagged {tag} (tags on HEAD: {})",
                        if tags.is_empty() {
                            "none".to_string()
                        } else {
                            tags.join(", ")
                        }
                    ),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("git tag --points-at HEAD ({tag})"),
                        at_head.as_bytes(),
                    )],
                    tier: Tier::Asset,
                    fix_hint: Some(
                        "create the tag only after the human approves the release".into(),
                    ),
                    path: String::new(),
                    line: 0,
                });
            }
        }

        if let (Some(version), true) = (&cfg.version, !cfg.version_files.is_empty()) {
            let re = Regex::new(r#"(?m)^\s*"?version"?\s*[:=]\s*"([^"]+)"#).expect("static regex");
            for file in &cfg.version_files {
                let Some(text) = ctx.read_text(file) else {
                    drafts.push(Draft {
                        rule: "version-file-missing",
                        severity: Severity::Blocker,
                        title: format!("{file} (a release version file) is missing"),
                        claim_seq: None,
                        evidence: vec![verdict_core::EvidencePointer::command(
                            &format!("release version file {file}"),
                            b"missing",
                        )],
                        tier: Tier::Static,
                        fix_hint: Some("restore the file or correct release.version_files".into()),
                        path: String::new(),
                        line: 0,
                    });
                    continue;
                };
                match re.captures(&text) {
                    None => drafts.push(Draft {
                        rule: "version-not-found",
                        severity: Severity::Blocker,
                        title: format!("{file} states no version"),
                        claim_seq: None,
                        evidence: vec![verdict_core::EvidencePointer::command(
                            &format!("release version in {file}"),
                            text.as_bytes(),
                        )],
                        tier: Tier::Static,
                        fix_hint: Some("state the release version in the manifest".into()),
                        path: String::new(),
                        line: 0,
                    }),
                    Some(c) => {
                        let found = c.get(1).map(|m| m.as_str()).unwrap_or("");
                        if found != version {
                            let at = c.get(0).map(|m| m.start()).unwrap_or(0);
                            let line = text[..at].matches('\n').count() as u64 + 1;
                            drafts.push(Draft {
                                rule: "version-mismatch",
                                severity: Severity::Blocker,
                                title: format!(
                                    "{file} states version {found}, but the release is {version}"
                                ),
                                claim_seq: None,
                                evidence: vec![ctx.line_pointer(file, line)],
                                tier: Tier::Static,
                                fix_hint: Some(
                                    "make every version file state the release version".into(),
                                ),
                                path: file.clone(),
                                line,
                            });
                        }
                    }
                }
            }
        }

        if !cfg.assets.is_empty() {
            for pattern in &cfg.assets {
                let found = asset_exists(&ctx.root, pattern);
                ctx.note(format!(
                    "release asset {}: {}",
                    pattern,
                    if found { "present" } else { "missing" }
                ));
                if !found {
                    drafts.push(Draft {
                        rule: "asset-missing",
                        severity: Severity::Blocker,
                        title: format!(
                            "the release asset {pattern} does not exist in the candidate"
                        ),
                        claim_seq: None,
                        evidence: vec![verdict_core::EvidencePointer::command(
                            &format!("release asset {pattern}"),
                            b"not found",
                        )],
                        tier: Tier::Asset,
                        fix_hint: Some("build the asset or correct release.assets".into()),
                        path: String::new(),
                        line: 0,
                    });
                }
            }
        }

        if let Some(readme) = &cfg.readme {
            if let Some(text) = ctx.read_text(readme) {
                let link = Regex::new(r"\[[^\]]*\]\(([^)\s]+)\)").expect("static regex");
                let dir = Path::new(readme)
                    .parent()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                for c in link.captures_iter(&text) {
                    let target = &c[1];
                    if target.starts_with("http://")
                        || target.starts_with("https://")
                        || target.starts_with('#')
                        || target.starts_with("mailto:")
                        || target.starts_with('/')
                    {
                        continue;
                    }
                    let path = target.split(['#', '?']).next().unwrap_or("");
                    if path.is_empty() {
                        continue;
                    }
                    let joined = if dir.is_empty() {
                        path.to_string()
                    } else {
                        format!("{dir}/{path}")
                    };
                    let normalized = normalize(&joined);
                    if !ctx.tree().paths.contains(&normalized) {
                        let at = c.get(0).map(|m| m.start()).unwrap_or(0);
                        let line = text[..at].matches('\n').count() as u64 + 1;
                        drafts.push(Draft {
                            rule: "broken-link",
                            severity: Severity::Major,
                            title: format!(
                                "{readme}:{line} links to {target}, which is not in the candidate"
                            ),
                            claim_seq: None,
                            evidence: vec![ctx.line_pointer(readme, line)],
                            tier: Tier::Static,
                            fix_hint: Some("fix the link or add the file it names".into()),
                            path: readme.clone(),
                            line,
                        });
                    }
                }
            } else {
                drafts.push(Draft {
                    rule: "readme-missing",
                    severity: Severity::Blocker,
                    title: format!("{readme} (the configured release README) is missing"),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("release readme {readme}"),
                        b"missing",
                    )],
                    tier: Tier::Static,
                    fix_hint: Some("restore the README or correct release.readme".into()),
                    path: String::new(),
                    line: 0,
                });
            }
        }

        if !cfg.checks.is_empty() {
            if !ctx.exec_approved {
                ctx.unestablished(
                    "release checks were not run: the human has not approved this repo's commands",
                );
            } else {
                let timeout = ctx.timeout;
                for check in &cfg.checks {
                    let run = run_command(check, &ctx.root, timeout);
                    if run.success() {
                        ctx.note(format!("release check passed: {check}"));
                        ctx.reached(Tier::Ran);
                    } else {
                        let why = run
                            .spawn_error
                            .clone()
                            .or_else(|| {
                                run.problem().or_else(|| {
                                    run.exit
                                        .map(|c| format!("it exited {c}"))
                                        .or_else(|| Some("it did not finish".into()))
                                })
                            })
                            .unwrap_or_else(|| "it failed".into());
                        drafts.push(Draft {
                            rule: "check-failed",
                            severity: Severity::Blocker,
                            title: format!("release check `{check}` did not pass: {why}"),
                            claim_seq: None,
                            evidence: vec![verdict_core::EvidencePointer::command(
                                check,
                                run.tail.as_bytes(),
                            )],
                            tier: Tier::Ran,
                            fix_hint: Some("make the project's own release check pass".into()),
                            path: String::new(),
                            line: 0,
                        });
                    }
                }
            }
        }
        Ok(finish(self.name(), drafts))
    }
}

/// Does a configured asset exist? The tree index skips build folders (`dist`, `target`), and a
/// release asset usually lives in one, so this asks the filesystem directly. A pattern with a
/// wildcard is matched against the children of the folder before the wildcard (`dist/*.zip`); a
/// `**` pattern walks.
fn asset_exists(root: &Path, pattern: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./");
    if !pattern.contains('*') && !pattern.contains('?') {
        let path = root.join(pattern);
        return if pattern.ends_with('/') {
            path.is_dir()
        } else {
            path.exists()
        };
    }
    let literal = pattern.split(['*', '?']).next().unwrap_or("");
    let dir = if literal.ends_with('/') {
        literal.trim_end_matches('/')
    } else {
        literal.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
    };
    let start = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    if !start.exists() {
        return false;
    }
    if pattern.contains("**") {
        return walkdir::WalkDir::new(&start)
            .max_depth(8)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .any(|e| {
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .unwrap_or(e.path())
                    .to_string_lossy()
                    .replace('\\', "/");
                asset_matches(pattern, &rel)
            });
    }
    let Ok(entries) = std::fs::read_dir(&start) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|e| {
        let rel = e
            .path()
            .strip_prefix(root)
            .unwrap_or(&e.path())
            .to_string_lossy()
            .replace('\\', "/");
        asset_matches(pattern, &rel)
    })
}

/// A configured asset: an exact path, a folder (`dist/`), or a simple glob (`dist/*.zip`).
fn asset_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    if pattern.ends_with('/') {
        return path.starts_with(&pattern);
    }
    if !pattern.contains('*') && !pattern.contains('?') {
        return path == pattern;
    }
    let escaped = regex::escape(&pattern)
        .replace(r"\*\*", "\u{1}")
        .replace(r"\*", "[^/]*")
        .replace(r"\?", "[^/]")
        .replace('\u{1}', ".*");
    Regex::new(&format!("^{escaped}$")).is_ok_and(|re| re.is_match(path))
}

fn normalize(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in replaced.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_assets_match_exactly_or_by_a_simple_glob() {
        assert!(asset_matches("dist/app.bin", "dist/app.bin"));
        assert!(!asset_matches("dist/app.bin", "dist/other.bin"));
        assert!(asset_matches("dist/", "dist/app.bin"));
        assert!(asset_matches("target/*.zip", "target/verdict.zip"));
        assert!(!asset_matches("target/*.zip", "target/sub/verdict.zip"));
        assert!(asset_matches("**/*.zip", "target/sub/verdict.zip"));
    }
}
