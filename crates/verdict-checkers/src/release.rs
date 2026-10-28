//! release: release-truth checks, from the project's own configuration.
//!
//! Everything here is configured; nothing assumes a web layout, a manifest name or a release
//! script. The policy's `release` block may name:
//!
//! * `checks` — the project's own release-truth commands. They run **first** (the design says so),
//!   under the declared shell and the same human approval as every other project command; a
//!   failing one blocks.
//! * `tag` — the tag this candidate must carry. Every tag pointing at HEAD is listed; a missing one
//!   blocks at release.
//! * `version` and `version_files` — the version the candidate claims, and the files that must
//!   state it. The version is read from the manifest's own package/project section, never from the
//!   first `version` key in the file (which could be a dependency).
//! * `assets` — paths or simple globs (`dist/`, `dist/*.zip`) that must exist. When the release
//!   also configures `asset_hashes` (`{"dist/app.zip": "sha256hex"}`) or ships an adjacent
//!   `<asset>.sha256`, the file is hashed (SHA-256) and a mismatch blocks.
//! * `published` — a saved `gh release view`-shaped JSON file the project records, listing the
//!   published release's assets and optional digests. Every candidate asset must be listed, and a
//!   digest that disagrees with the local file blocks.
//! * `product` — the product identity: `{"name": "verdict", "manifest": "Cargo.toml",
//!   "lockfile": "Cargo.lock"}`. The manifest must name the product and the lockfile must pin its
//!   version.
//! * `readme` — a Markdown file whose relative links must resolve, plus `readme_commands` that
//!   must appear in it verbatim.
//! * `suite` — optional member repositories: `{"members": [{"path": "libs/a",
//!   "dependency_files": ["Cargo.lock"], "product_version": "1.2.0", "release_record": "..."}]}`.
//!   Shared dependency pins must agree across the members, and each member's product version must
//!   agree with the release.
//!
//! A release block with no configuration is `Unavailable`, never clean. The tag check cannot pass
//! before the human creates the tag: that is exactly why the release lane is the last gate.

use std::collections::BTreeMap;
use std::path::Path;

use regex::Regex;
use serde_json::Value;
use verdict_core::exec_approval::policy_shell;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::exec::run_command_in;

/// The release checker.
pub struct Release;

#[derive(Default)]
struct Product {
    name: String,
    manifest: Option<String>,
    lockfile: Option<String>,
}

#[derive(Default, Clone)]
struct Member {
    path: String,
    dependency_files: Vec<String>,
    product_version: Option<String>,
    release_record: Option<String>,
}

#[derive(Default)]
struct Config {
    tag: Option<String>,
    version: Option<String>,
    version_files: Vec<String>,
    assets: Vec<String>,
    asset_hashes: BTreeMap<String, String>,
    published: Option<String>,
    product: Option<Product>,
    readme: Option<String>,
    readme_commands: Vec<String>,
    suite: Vec<Member>,
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
    let product = policy
        .value
        .pointer("/release/product")
        .and_then(Value::as_object)
        .and_then(|o| {
            let name = o.get("name")?.as_str()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(Product {
                name,
                manifest: o
                    .get("manifest")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                lockfile: o
                    .get("lockfile")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            })
        });
    let asset_hashes = policy
        .value
        .pointer("/release/asset_hashes")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    v.as_str()
                        .map(|s| (k.clone(), s.trim().to_ascii_lowercase()))
                })
                .collect()
        })
        .unwrap_or_default();
    let suite = policy
        .value
        .pointer("/suite/members")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let path = m.get("path")?.as_str()?.trim().to_string();
                    if path.is_empty() {
                        return None;
                    }
                    Some(Member {
                        path,
                        dependency_files: m
                            .get("dependency_files")
                            .and_then(Value::as_array)
                            .map(|d| {
                                d.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                        product_version: m
                            .get("product_version")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        release_record: m
                            .get("release_record")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Config {
        tag: string(policy, "/release/tag"),
        version: string(policy, "/release/version"),
        version_files: strings(policy, "/release/version_files"),
        assets: strings(policy, "/release/assets"),
        asset_hashes,
        published: string(policy, "/release/published"),
        product,
        readme: string(policy, "/release/readme"),
        readme_commands: strings(policy, "/release/readme_commands"),
        suite,
        checks: strings(policy, "/release/checks"),
    }
}

impl Config {
    fn is_empty(&self) -> bool {
        self.tag.is_none()
            && self.version.is_none()
            && self.assets.is_empty()
            && self.published.is_none()
            && self.product.is_none()
            && self.readme.is_none()
            && self.suite.is_empty()
            && self.checks.is_empty()
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
        if cfg.is_empty() {
            return Err(Unavailable::new(
                "no release checks are configured (set release.tag, release.version_files, \
                 release.assets, release.product, release.published, release.readme or \
                 release.checks)",
            ));
        }
        if cfg.version.is_some() && cfg.version_files.is_empty() {
            return Err(Unavailable::new(
                "release.version is configured but release.version_files is empty, so the version \
                 cannot be checked against any file",
            ));
        }
        let mut drafts: Vec<Draft> = Vec::new();

        // 1. The project's own release-truth commands run first, as the design requires.
        if !cfg.checks.is_empty() {
            if !ctx.exec_approved {
                ctx.unestablished(
                    "release checks were not run: the human has not approved this repo's commands",
                );
            } else {
                let shell = policy_shell(&ctx.policy).map_err(|e| {
                    Unavailable::new(format!(
                        "the project's declared shell cannot be used: {e}; the release checks did \
                         not run"
                    ))
                })?;
                let timeout = ctx.timeout;
                for check in &cfg.checks {
                    let run = run_command_in(check, &ctx.root, timeout, shell);
                    if run.success() {
                        ctx.note(format!(
                            "release check passed: {check} (output blake3 {})",
                            run.output_hash
                        ));
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

        // 2. Tag on HEAD.
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

        // 3. The version every version file states.
        if let (Some(version), true) = (&cfg.version, !cfg.version_files.is_empty()) {
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
                match stated_version(file, &text) {
                    None => drafts.push(Draft {
                        rule: "version-not-found",
                        severity: Severity::Blocker,
                        title: format!(
                            "{file} states no package/project version (a bare `version` key is not \
                             enough)"
                        ),
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
                    Some((found, line)) => {
                        if found != *version {
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

        // 4. Product identity: the manifest names it and the lockfile pins its version.
        if let Some(product) = &cfg.product {
            let manifest = product.manifest.as_deref().unwrap_or("Cargo.toml");
            match ctx.read_text(manifest) {
                None => drafts.push(Draft {
                    rule: "product-manifest-missing",
                    severity: Severity::Blocker,
                    title: format!("the product manifest {manifest} is missing"),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("product manifest {manifest}"),
                        b"missing",
                    )],
                    tier: Tier::Static,
                    fix_hint: Some(
                        "restore the manifest or correct release.product.manifest".into(),
                    ),
                    path: String::new(),
                    line: 0,
                }),
                Some(text) => {
                    if !manifest_names(&text, &product.name) {
                        drafts.push(Draft {
                            rule: "product-name-mismatch",
                            severity: Severity::Blocker,
                            title: format!(
                                "{manifest} does not name the product {:?} in its package section",
                                product.name
                            ),
                            claim_seq: None,
                            evidence: vec![verdict_core::EvidencePointer::command(
                                &format!("product name in {manifest}"),
                                text.as_bytes(),
                            )],
                            tier: Tier::Static,
                            fix_hint: Some("set the package name the release claims".into()),
                            path: String::new(),
                            line: 0,
                        });
                    }
                }
            }
            if let (Some(lockfile), Some(version)) = (&product.lockfile, &cfg.version) {
                match ctx.read_text(lockfile) {
                    None => drafts.push(Draft {
                        rule: "lockfile-missing",
                        severity: Severity::Blocker,
                        title: format!("the lockfile {lockfile} is missing"),
                        claim_seq: None,
                        evidence: vec![verdict_core::EvidencePointer::command(
                            &format!("lockfile {lockfile}"),
                            b"missing",
                        )],
                        tier: Tier::Static,
                        fix_hint: Some(
                            "restore the lockfile or correct release.product.lockfile".into(),
                        ),
                        path: String::new(),
                        line: 0,
                    }),
                    Some(text) => {
                        if !lockfile_pins(&text, &product.name, version) {
                            drafts.push(Draft {
                                rule: "lockfile-version-mismatch",
                                severity: Severity::Blocker,
                                title: format!(
                                    "{lockfile} does not pin {:?} at version {version}",
                                    product.name
                                ),
                                claim_seq: None,
                                evidence: vec![verdict_core::EvidencePointer::command(
                                    &format!("lockfile pin for {}", product.name),
                                    text.as_bytes(),
                                )],
                                tier: Tier::Static,
                                fix_hint: Some(
                                    "refresh the lockfile for the release version".into(),
                                ),
                                path: String::new(),
                                line: 0,
                            });
                        }
                    }
                }
            }
        }

        // 5. Assets exist, and their hashes agree where the project states them.
        if !cfg.assets.is_empty() {
            for pattern in &cfg.assets {
                let files = asset_files(&ctx.root, pattern);
                ctx.note(format!(
                    "release asset {}: {}",
                    pattern,
                    if files.is_empty() {
                        "missing".to_string()
                    } else {
                        files.join(", ")
                    }
                ));
                if files.is_empty() {
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
                    continue;
                }
                for file in files {
                    verify_asset_hash(ctx, &cfg, &file, &mut drafts);
                }
            }
        }
        for path in cfg.asset_hashes.keys() {
            if !ctx.root.join(path).exists() {
                drafts.push(Draft {
                    rule: "asset-hash-missing-file",
                    severity: Severity::Blocker,
                    title: format!(
                        "release.asset_hashes names {path}, but that file is not in the candidate"
                    ),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("release.asset_hashes {path}"),
                        b"not found",
                    )],
                    tier: Tier::Asset,
                    fix_hint: Some("build the asset or correct release.asset_hashes".into()),
                    path: String::new(),
                    line: 0,
                });
            }
        }

        // 6. Published release correspondence.
        if let Some(published) = &cfg.published {
            match ctx.read_text(published) {
                None => drafts.push(Draft {
                    rule: "published-missing",
                    severity: Severity::Blocker,
                    title: format!(
                        "the saved published-release record {published} cannot be read, so the \
                         candidate cannot be compared with what was published"
                    ),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("published release {published}"),
                        b"missing",
                    )],
                    tier: Tier::Asset,
                    fix_hint: Some(
                        "save the published release's asset list (gh release view --json assets)"
                            .into(),
                    ),
                    path: String::new(),
                    line: 0,
                }),
                Some(text) => published_mismatches(ctx, &cfg, &text, &mut drafts),
            }
        }

        // 7. README links and commands.
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
                    if !ctx.tree().paths.contains(&normalized) && !ctx.tree().has_path(&normalized)
                    {
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
                for command in &cfg.readme_commands {
                    if !text.contains(command.as_str()) {
                        drafts.push(Draft {
                            rule: "readme-command-missing",
                            severity: Severity::Major,
                            title: format!("{readme} does not show the command `{command}`"),
                            claim_seq: None,
                            evidence: vec![verdict_core::EvidencePointer::command(
                                &format!("README command {command}"),
                                text.as_bytes(),
                            )],
                            tier: Tier::Static,
                            fix_hint: Some("document the command the release runs".into()),
                            path: String::new(),
                            line: 0,
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

        // 8. Suite-wide pin and version coherence.
        if !cfg.suite.is_empty() {
            check_suite(ctx, &cfg, &mut drafts);
        }

        Ok(finish(self.name(), drafts))
    }
}

/// The version a manifest states for its own package/project, and the line it is on. A bare
/// `version` key outside a package/project section is never taken (it could be a dependency).
fn stated_version(file: &str, text: &str) -> Option<(String, u64)> {
    let lower = file.to_ascii_lowercase();
    // A JSON manifest states its version at the top level; a lockfile never states the product's.
    if lower.ends_with(".json") {
        let v = serde_json::from_str::<Value>(text).ok()?;
        let s = v.get("version").and_then(Value::as_str)?;
        let line = text
            .lines()
            .position(|l| l.contains("\"version\"") && l.contains(s))
            .map(|i| i as u64 + 1)
            .unwrap_or(1);
        return Some((s.trim().to_string(), line));
    }
    // Lockfiles pin dependencies; their `version` keys are never the product version.
    if matches!(
        lower.rsplit('/').next().unwrap_or(""),
        "cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "poetry.lock"
    ) {
        return None;
    }
    let mut section = String::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t
                .trim_matches('[')
                .trim_matches(']')
                .trim()
                .to_ascii_lowercase();
            continue;
        }
        if matches!(
            section.as_str(),
            "package" | "project" | "workspace.package" | "tool.poetry"
        ) && let Some((_, value)) = t.split_once('=')
        {
            let key = t.split('=').next().unwrap_or("").trim();
            if key == "version" {
                let v = value.trim().trim_matches(['"', '\'']);
                if !v.is_empty() {
                    return Some((v.to_string(), i as u64 + 1));
                }
            }
        }
    }
    // A .json file that failed to parse, or another format: take an anchored top-level-looking
    // assignment rather than an arbitrary dependency line.
    let re = Regex::new(r#"(?m)^\s*version\s*[:=]\s*"([^"]+)""#).ok()?;
    let c = re.captures(text)?;
    let line = text[..c.get(0)?.start()].matches('\n').count() as u64 + 1;
    Some((c.get(1)?.as_str().to_string(), line))
}

fn manifest_names(text: &str, name: &str) -> bool {
    let mut section = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t
                .trim_matches('[')
                .trim_matches(']')
                .trim()
                .to_ascii_lowercase();
            continue;
        }
        if matches!(section.as_str(), "package" | "project" | "tool.poetry")
            && let Some((k, v)) = t.split_once('=')
            && k.trim() == "name"
            && v.trim().trim_matches(['"', '\'']) == name
        {
            return true;
        }
    }
    false
}

fn lockfile_pins(text: &str, name: &str, version: &str) -> bool {
    let escaped = regex::escape(name);
    let escaped_version = regex::escape(version);
    let re = Regex::new(&format!(
        r#"(?s)name\s*=\s*"{escaped}"\s*(?:\n[^\n]*)*?\n\s*version\s*=\s*"{escaped_version}""#
    ))
    .expect("escaped regex");
    if re.is_match(text) {
        return true;
    }
    // A JSON lockfile (package-lock.json shaped).
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        let mut found = false;
        if let Some(pkgs) = v.get("packages").and_then(Value::as_object) {
            for (path, body) in pkgs {
                if path.ends_with(&format!("node_modules/{name}"))
                    && body.get("version").and_then(Value::as_str)
                        == Some(version.trim_matches('"'))
                {
                    found = true;
                }
            }
        }
        if let Some(deps) = v.get("dependencies").and_then(Value::as_object)
            && let Some(body) = deps.get(name)
            && body.get("version").and_then(Value::as_str) == Some(version.trim_matches('"'))
        {
            found = true;
        }
        return found;
    }
    false
}

/// Verify one asset file against a configured hash or an adjacent `<file>.sha256`.
fn verify_asset_hash(ctx: &CheckContext, cfg: &Config, rel: &str, drafts: &mut Vec<Draft>) {
    let Some(bytes) = std::fs::read(ctx.root.join(rel)).ok() else {
        return;
    };
    let actual = verdict_core::sha256::hex(&bytes);
    let stated: Option<(String, String)> = if let Some(want) = cfg.asset_hashes.get(rel) {
        Some(("release.asset_hashes".into(), want.clone()))
    } else {
        let sidecar = format!("{rel}.sha256");
        ctx.read_text(&sidecar).and_then(|t| {
            t.split_whitespace()
                .next()
                .filter(|h| h.len() == 64)
                .map(|h| (sidecar, h.to_ascii_lowercase()))
        })
    };
    let Some((source, want)) = stated else { return };
    if want == actual {
        ctx.note(format!(
            "release asset {rel} sha256 {actual} matches {source}"
        ));
    } else {
        drafts.push(Draft {
            rule: "asset-hash-mismatch",
            severity: Severity::Blocker,
            title: format!(
                "the release asset {rel} has SHA-256 {actual}, but {source} says {want}"
            ),
            claim_seq: None,
            evidence: vec![verdict_core::EvidencePointer::command(
                &format!("sha256 {rel}"),
                bytes.as_slice(),
            )],
            tier: Tier::Asset,
            fix_hint: Some(
                "rebuild the asset and its checksum from the same bytes, then publish again".into(),
            ),
            path: rel.to_string(),
            line: 0,
        });
    }
}

/// Compare the candidate's assets with the saved published-release record.
fn published_mismatches(ctx: &CheckContext, cfg: &Config, text: &str, drafts: &mut Vec<Draft>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        drafts.push(Draft {
            rule: "published-unreadable",
            severity: Severity::Blocker,
            title: "the saved published-release record is not JSON".into(),
            claim_seq: None,
            evidence: vec![verdict_core::EvidencePointer::command(
                "published release",
                text.as_bytes(),
            )],
            tier: Tier::Asset,
            fix_hint: Some("save `gh release view --json assets` as the record".into()),
            path: String::new(),
            line: 0,
        });
        return;
    };
    let assets = v
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let published: Vec<String> = assets
        .iter()
        .filter_map(|a| a.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    for pattern in &cfg.assets {
        for file in asset_files(&ctx.root, pattern) {
            let name = file.rsplit('/').next().unwrap_or(&file).to_string();
            if !published.contains(&name) {
                drafts.push(Draft {
                    rule: "published-asset-missing",
                    severity: Severity::Blocker,
                    title: format!(
                        "the candidate asset {name} is not in the published release's asset list"
                    ),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("published assets ({name})"),
                        text.as_bytes(),
                    )],
                    tier: Tier::Asset,
                    fix_hint: Some("publish the asset or correct the saved release record".into()),
                    path: String::new(),
                    line: 0,
                });
            }
        }
    }
}

/// Suite-wide dependency pins and product versions.
fn check_suite(ctx: &CheckContext, cfg: &Config, drafts: &mut Vec<Draft>) {
    let mut pins: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for member in &cfg.suite {
        let dir = ctx.root.join(&member.path);
        ctx.note(format!("suite member {}", member.path));
        if !dir.is_dir() {
            drafts.push(Draft {
                rule: "suite-member-missing",
                severity: Severity::Major,
                title: format!(
                    "suite member {} is not a folder in this checkout",
                    member.path
                ),
                claim_seq: None,
                evidence: vec![verdict_core::EvidencePointer::command(
                    &format!("suite member {}", member.path),
                    b"not found",
                )],
                tier: Tier::Static,
                fix_hint: Some("check the member out or correct suite.members".into()),
                path: String::new(),
                line: 0,
            });
            continue;
        }
        for file in &member.dependency_files {
            let rel = format!("{}/{}", member.path.trim_end_matches('/'), file);
            let Some(text) = ctx.read_text(&rel) else {
                drafts.push(Draft {
                    rule: "suite-dependency-file-missing",
                    severity: Severity::Major,
                    title: format!("suite member {} is missing {file}", member.path),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("suite dependency file {rel}"),
                        b"missing",
                    )],
                    tier: Tier::Static,
                    fix_hint: Some("restore the file or correct suite.members".into()),
                    path: String::new(),
                    line: 0,
                });
                continue;
            };
            for (name, version) in parse_pins(&text) {
                pins.entry(name)
                    .or_default()
                    .entry(version)
                    .or_default()
                    .push(member.path.clone());
            }
        }
        if let (Some(version), Some(release)) = (&member.product_version, &cfg.version)
            && version != release
        {
            drafts.push(Draft {
                rule: "suite-version-mismatch",
                severity: Severity::Blocker,
                title: format!(
                    "suite member {} declares product version {version}, but the release is {release}",
                    member.path
                ),
                claim_seq: None,
                evidence: vec![verdict_core::EvidencePointer::command(
                    &format!("suite product version {}", member.path),
                    version.as_bytes(),
                )],
                tier: Tier::Static,
                fix_hint: Some("align the member's version with the release".into()),
                path: String::new(),
                line: 0,
            });
        }
        if let (Some(record), Some(version)) = (&member.release_record, &member.product_version) {
            match ctx.read_text(&format!(
                "{}/{}",
                member.path.trim_end_matches('/'),
                record
            )) {
                Some(text) if text.contains(version.as_str()) => {}
                Some(_) => drafts.push(Draft {
                    rule: "suite-release-record-mismatch",
                    severity: Severity::Major,
                    title: format!(
                        "suite member {}'s release record {record} does not state version {version}",
                        member.path
                    ),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("suite release record {}", member.path),
                        version.as_bytes(),
                    )],
                    tier: Tier::Static,
                    fix_hint: Some("bring the member's release record up to date".into()),
                    path: String::new(),
                    line: 0,
                }),
                None => drafts.push(Draft {
                    rule: "suite-release-record-missing",
                    severity: Severity::Major,
                    title: format!(
                        "suite member {}'s release record {record} cannot be read",
                        member.path
                    ),
                    claim_seq: None,
                    evidence: vec![verdict_core::EvidencePointer::command(
                        &format!("suite release record {}", member.path),
                        b"missing",
                    )],
                    tier: Tier::Static,
                    fix_hint: Some("restore the release record".into()),
                    path: String::new(),
                    line: 0,
                }),
            }
        }
    }
    for (name, versions) in pins {
        if versions.len() > 1 {
            let shown: Vec<String> = versions
                .iter()
                .map(|(v, members)| format!("{v} in {}", members.join(", ")))
                .collect();
            drafts.push(Draft {
                rule: "suite-pin-mismatch",
                severity: Severity::Major,
                title: format!(
                    "suite members pin {name} at different versions: {}",
                    shown.join("; ")
                ),
                claim_seq: None,
                evidence: vec![verdict_core::EvidencePointer::command(
                    &format!("suite pin {name}"),
                    shown.join("\n").as_bytes(),
                )],
                tier: Tier::Static,
                fix_hint: Some("agree one shared pin across the suite".into()),
                path: String::new(),
                line: 0,
            });
        }
    }
}

/// Dependency pins in a lockfile: `(name, version)` pairs from TOML or JSON lockfiles.
fn parse_pins(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let re = Regex::new(r#"(?m)^name = "([^"]+)"\s*$"#).expect("static regex");
    let ver = Regex::new(r#"(?m)^version = "([^"]+)"\s*$"#).expect("static regex");
    let names: Vec<_> = re.captures_iter(text).collect();
    let versions: Vec<_> = ver.captures_iter(text).collect();
    if names.len() == versions.len() {
        for (n, v) in names.iter().zip(versions) {
            out.push((n[1].to_string(), v[1].to_string()));
        }
        return out;
    }
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(pkgs) = v.get("packages").and_then(Value::as_object) {
            for (path, body) in pkgs {
                if let (Some(idx), Some(version)) = (
                    path.rfind("node_modules/"),
                    body.get("version").and_then(Value::as_str),
                ) {
                    let name = &path[idx + "node_modules/".len()..];
                    out.push((name.to_string(), version.to_string()));
                }
            }
        } else if let Some(deps) = v.get("dependencies").and_then(Value::as_object) {
            for (name, body) in deps {
                if let Some(version) = body.get("version").and_then(Value::as_str) {
                    out.push((name.clone(), version.to_string()));
                }
            }
        }
    }
    out
}

/// Every file a configured asset names (an exact path, a folder, or a simple glob).
fn asset_files(root: &Path, pattern: &str) -> Vec<String> {
    let pattern = pattern.replace('\\', "/");
    let pattern = pattern.trim_start_matches("./").to_string();
    if !pattern.contains('*') && !pattern.contains('?') {
        let path = root.join(&pattern);
        if pattern.ends_with('/') {
            if path.is_dir() {
                return list_files(root, &pattern);
            }
            return Vec::new();
        }
        return path.exists().then_some(pattern).into_iter().collect();
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
        return Vec::new();
    }
    if pattern.contains("**") {
        return walkdir::WalkDir::new(&start)
            .max_depth(8)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                e.path()
                    .strip_prefix(root)
                    .unwrap_or(e.path())
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .filter(|rel| asset_matches(&pattern, rel))
            .collect();
    }
    let Ok(entries) = std::fs::read_dir(&start) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| {
            e.path()
                .strip_prefix(root)
                .unwrap_or(&e.path())
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|rel| asset_matches(&pattern, rel))
        .collect()
}

fn list_files(root: &Path, dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| {
            e.path()
                .strip_prefix(root)
                .unwrap_or(&e.path())
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
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

    #[test]
    fn a_version_is_read_from_the_package_section_not_a_dependency() {
        let toml = "[package]\nname = \"verdict\"\nversion = \"1.0.0\"\n\n\
                    [dependencies]\nserde = { version = \"9.9.9\" }\n";
        assert_eq!(stated_version("Cargo.toml", toml).unwrap().0, "1.0.0");
        let lock = "[[package]]\nname = \"serde\"\nversion = \"9.9.9\"\n";
        assert!(stated_version("Cargo.lock", lock).is_none());
        let json = "{\"name\":\"x\",\"version\":\"2.0.0\"}";
        assert_eq!(stated_version("package.json", json).unwrap().0, "2.0.0");
        // A JSON file with only a nested version key does not state a package version.
        assert!(stated_version("weird.json", "{\"a\":{\"version\":\"3\"}}").is_none());
    }

    #[test]
    fn lockfile_pins_are_checked_for_toml_and_json_shapes() {
        let toml = "[[package]]\nname = \"verdict-gate\"\nversion = \"1.0.0\"\n";
        assert!(lockfile_pins(toml, "verdict-gate", "1.0.0"));
        assert!(!lockfile_pins(toml, "verdict-gate", "1.0.1"));
        let json = "{\"packages\":{\"node_modules/left-pad\":{\"version\":\"1.2.3\"}}}";
        assert!(lockfile_pins(json, "left-pad", "1.2.3"));
    }
}
