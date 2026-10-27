//! `vdt generate`: emit the host files from the policy, so the discipline lives once.
//!
//! The output is a plain directory tree:
//!
//! * `skills/<desk>.md` — the packaged rubric plus the policy facts that bound it (lane sets,
//!   the five-finding cap, the cross-vendor rule). One file per desk.
//! * `hooks/<host>.json` — the exact hook registration for Claude Code and Codex, in that host's
//!   own shape.
//! * `policy.json` — the effective policy in canonical JSON, so a generated tree says which policy
//!   it came from.
//! * `GENERATED.md` — what wrote these files and how to check them.
//!
//! Everything is a pure function of the policy and the packaged assets. No timestamp, no path, no
//! machine name goes into any file, so two runs on two machines produce byte-identical trees; a
//! test checks it by generating twice and comparing every byte.
//!
//! A host file is never written over a file Verdict does not own: callers point `--out` at a
//! folder Verdict owns (the default is `<project>/.verdict/generated`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use verdict_core::lane::{ALL_LANES, lane_name};
use verdict_core::policy::Policy;

/// One generated file: its path under `out` and its exact bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    /// Relative path, `/` separators.
    pub path: String,
    /// Exact contents.
    pub bytes: Vec<u8>,
}

/// The cap the report applies per desk; generated skills state it.
pub const REPORT_CAP: usize = 5;

fn lanes_json(policy: &Policy) -> Value {
    let mut out = serde_json::Map::new();
    for lane in ALL_LANES {
        let name = lane_name(lane);
        let checkers = policy
            .value
            .pointer(&format!("/lanes/{name}/checkers"))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let desks = policy
            .value
            .pointer(&format!("/lanes/{name}/desks"))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        out.insert(
            name.to_string(),
            json!({ "checkers": checkers, "desks": desks }),
        );
    }
    Value::Object(out)
}

fn list_text(v: &Value) -> String {
    let items: Vec<&str> = v
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

/// The policy facts a generated skill must carry, rendered deterministically.
fn facts(policy: &Policy) -> String {
    let lanes = lanes_json(policy);
    let mut text = String::new();
    text.push_str("## The policy these files were generated from\n\n");
    text.push_str(&format!(
        "- Policy version {} (blake3 {}).\n",
        policy.version, policy.hash
    ));
    text.push_str(&format!(
        "- Each desk reports at most {REPORT_CAP} findings in the report body; the rest stay in the \
         ledger and are counted.\n"
    ));
    let threshold = policy
        .value
        .pointer("/vendors/cross_vendor_required_from_lane")
        .and_then(Value::as_str)
        .unwrap_or("standard");
    text.push_str(&format!(
        "- From the {threshold} lane up, a desk must be served by a vendor other than the builder's. \
         The only downgrade is the global policy's `vendors.single_vendor: \"warn\"`, and the receipt \
         then says `single_vendor`.\n"
    ));
    text.push_str("\n### Lanes\n\n");
    for lane in ALL_LANES {
        let name = lane_name(lane);
        let (checkers, desks) = match lanes.get(name) {
            Some(v) => (list_text(&v["checkers"]), list_text(&v["desks"])),
            None => ("none".to_string(), "none".to_string()),
        };
        text.push_str(&format!(
            "- **{name}** — checkers: {checkers}; desks: {desks}.\n"
        ));
    }
    text
}

/// The hook registration files, in each host's own shape.
fn hook_files() -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for host in [
        verdict_adapters::Host::Claude,
        verdict_adapters::Host::Codex,
    ] {
        let block = verdict_adapters::install::hook_block("vdt", host);
        let text = format!(
            "{}\n",
            serde_json::to_string_pretty(&block).unwrap_or_default()
        );
        out.insert(format!("hooks/{}.json", host.as_str()), text.into_bytes());
    }
    out
}

/// Every file `vdt generate` emits for `policy`, sorted by path.
pub fn generate(policy: &Policy) -> Vec<Generated> {
    let mut files: Vec<Generated> = Vec::new();
    let facts = facts(policy);
    for desk in verdict_desks::DESK_NAMES {
        let Some(rubric) = verdict_desks::rubric(desk) else {
            continue;
        };
        // The rubric is compiled in from a file, and a Windows checkout may have turned its line
        // endings into CRLF. The generated tree has to be byte-identical on every machine, so it
        // is normalised here rather than trusted from the build host.
        let rubric = rubric.trim_end().replace("\r\n", "\n");
        let text = format!(
            "<!-- Generated by `vdt generate` from policy v{} ({}). Do not edit by hand: change the \
             policy or the packaged rubric, then generate again. -->\n\n{}\n\n{}\n",
            policy.version, policy.hash, rubric, facts
        );
        files.push(Generated {
            path: format!("skills/{desk}.md"),
            bytes: text.into_bytes(),
        });
    }
    for (path, bytes) in hook_files() {
        files.push(Generated { path, bytes });
    }
    // The effective policy, canonical: serde_json's map is key-ordered.
    files.push(Generated {
        path: "policy.json".into(),
        bytes: format!(
            "{}\n",
            serde_json::to_string_pretty(&policy.value).unwrap_or_default()
        )
        .into_bytes(),
    });
    files.push(Generated {
        path: "GENERATED.md".into(),
        bytes: format!(
            "# Generated by Verdict\n\nThese files come from `vdt generate` and the policy they name \
             (version {}, blake3 {}). Regenerate rather than edit: the policy and the packaged \
             rubrics are the source. `vdt generate --out <dir>` writes this tree again.\n\nThis tool \
             owns this folder only; it never writes a host's own settings file. Install a host's \
             hooks with `vdt hooks install <host> --i-am-the-human`.\n",
            policy.version, policy.hash
        )
        .into_bytes(),
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Write the generated tree under `out`, creating folders as needed. Returns the paths written.
pub fn write_tree(policy: &Policy, out: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for f in generate(policy) {
        let path = out.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, f.bytes)?;
        written.push(path);
    }
    Ok(written)
}

/// The lane names, in order, for a caller that wants to print them.
pub fn lane_names() -> Vec<&'static str> {
    ALL_LANES.iter().map(|l| lane_name(*l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy::builtin().unwrap()
    }

    #[test]
    fn every_desk_and_every_host_is_generated() {
        let files = generate(&policy());
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        for desk in verdict_desks::DESK_NAMES {
            assert!(
                paths.contains(&format!("skills/{desk}.md").as_str()),
                "{paths:?}"
            );
        }
        assert!(paths.contains(&"hooks/claude.json"));
        assert!(paths.contains(&"hooks/codex.json"));
        assert!(paths.contains(&"policy.json"));
        assert!(paths.contains(&"GENERATED.md"));
        // Sorted, so two runs traverse in the same order.
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn the_output_is_a_pure_function_of_the_policy() {
        let a = generate(&policy());
        let b = generate(&policy());
        assert_eq!(a, b);
        // Nothing absolute, machine-shaped or time-like leaks in.
        for f in &a {
            let text = String::from_utf8_lossy(&f.bytes);
            assert!(
                !text.contains('\\'),
                "{} has a Windows path separator",
                f.path
            );
            assert!(!text.contains("C:"), "{} has an absolute path", f.path);
            for bad in ["2026-", "T12:"] {
                assert!(!text.contains(bad), "{} has {bad:?}", f.path);
            }
        }
    }

    #[test]
    fn a_different_policy_generates_different_skills() {
        let mut p = policy();
        if let Some(v) = p.value.pointer_mut("/lanes/standard/desks") {
            *v = json!(["docs"]);
        }
        let a = generate(&p);
        let b = generate(&policy());
        assert_ne!(a, b);
    }

    #[test]
    fn the_skills_carry_their_rubric_and_the_lane_facts() {
        let files = generate(&policy());
        let engineer = files
            .iter()
            .find(|f| f.path == "skills/engineer.md")
            .unwrap();
        let text = String::from_utf8_lossy(&engineer.bytes);
        assert!(text.contains("Engineer desk rubric"));
        assert!(text.contains("**standard**"));
        assert!(text.contains("at most 5 findings"));
        assert!(text.contains("vendor other than the builder's"));
    }
}
