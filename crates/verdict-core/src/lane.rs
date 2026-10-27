//! Lane classification: how much is at stake in what a session changed.
//!
//! The lane comes from **what the diff touches**, matched against the policy's blast globs. It never
//! comes from the size of the diff or from a label the agent supplies. The human can raise a lane
//! ([`DecisionKind::RaiseLane`]); nothing can lower it below what the diff implies.
//!
//! * [`classify_lane`] maps changed paths to a lane (pure, no I/O).
//! * [`touched_paths`] finds the changed paths of a session (edit tool calls, git diff since the
//!   baseline, untracked files, the staged index).
//! * [`effective_lane`] applies the human's raises on top.
//! * [`lane_checkers`] expands a lane's checker list (`*` and `-name`) into names, in the order the
//!   core runs them.

use globset::{Glob, GlobBuilder, GlobMatcher};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::ledger::{Ledger, LedgerError};
use crate::model::{DecisionKind, Lane, Record, Writer};
use crate::policy::Policy;

/// Every checker the core knows, in the order it runs them (wiring before claims_vs_code, exec
/// before mutation). `*` in a lane's list means exactly these. `packet` is a renderer, not a
/// checker, and is not here.
pub const ALL_CHECKERS: [&str; 10] = [
    "wiring",
    "baseline_diff",
    "regressions",
    "tests_weakened",
    "claims_vs_code",
    "unfinished",
    "exec",
    "mutation",
    "firstrun",
    "release",
];

/// Lane names, lowest first.
pub const ALL_LANES: [Lane; 4] = [Lane::Low, Lane::Standard, Lane::High, Lane::Release];

/// Stable lower-case name of a lane.
pub fn lane_name(lane: Lane) -> &'static str {
    match lane {
        Lane::Low => "low",
        Lane::Standard => "standard",
        Lane::High => "high",
        Lane::Release => "release",
    }
}

/// Parse a lane name.
pub fn parse_lane(s: &str) -> Option<Lane> {
    ALL_LANES.into_iter().find(|l| lane_name(*l) == s.trim())
}

/// True if `name` is one of the core's checkers.
pub fn is_checker(name: &str) -> bool {
    ALL_CHECKERS.contains(&name)
}

fn names_of(policy: &Policy, pointer: &str) -> Vec<String> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The checkers a lane runs, with `*` and `-name` expanded. Items apply left to right: a name adds
/// itself, `*` adds every checker, `-name` removes one. The result is in the core's run order;
/// names the core does not know (a custom checker) come last, in the order the policy gave them.
pub fn expand_selection(items: &[String]) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for item in items {
        if item == "*" {
            for c in ALL_CHECKERS {
                if !set.iter().any(|s| s == c) {
                    set.push(c.to_string());
                }
            }
        } else if let Some(minus) = item.strip_prefix('-') {
            set.retain(|s| s != minus);
        } else if !set.iter().any(|s| s == item) {
            set.push(item.clone());
        }
    }
    let mut ordered: Vec<String> = ALL_CHECKERS
        .iter()
        .filter(|c| set.iter().any(|s| s == *c))
        .map(|c| c.to_string())
        .collect();
    for s in set {
        if !is_checker(&s) {
            ordered.push(s);
        }
    }
    ordered
}

/// The expanded checker list for `lane` under `policy`.
pub fn lane_checkers(policy: &Policy, lane: Lane) -> Vec<String> {
    expand_selection(&names_of(
        policy,
        &format!("/lanes/{}/checkers", lane_name(lane)),
    ))
}

/// The expanded desk list for `lane` (`*` means every desk name the policy lists for release; the
/// desks themselves arrive in a later phase, so `*` stays `*` here).
pub fn lane_desks(policy: &Policy, lane: Lane) -> Vec<String> {
    names_of(policy, &format!("/lanes/{}/desks", lane_name(lane)))
}

/// A glob that matches at any depth when the pattern has no `/` (like a `.gitignore` line), and
/// matches case-insensitively (a lane that errs high is safe; one that errs low is not).
fn compile(pattern: &str) -> Result<GlobMatcher, String> {
    let p = pattern.replace('\\', "/");
    let p = p.trim_start_matches("./");
    let full = if p.contains('/') {
        p.to_string()
    } else {
        format!("**/{p}")
    };
    GlobBuilder::new(&full)
        .literal_separator(true)
        .case_insensitive(true)
        .build()
        .map(|g: Glob| g.compile_matcher())
        .map_err(|e| format!("blast glob {pattern:?} is not a valid glob: {e}"))
}

/// The paths a session changed, as the lane sees them: forward slashes, relative to the project,
/// with the policy's excluded folders and the tool's own `.verdict` folder left out.
pub fn normalize_paths(policy: &Policy, root: &Path, paths: &[String]) -> Vec<String> {
    let exclude: Vec<GlobMatcher> = names_of(policy, "/exclude")
        .iter()
        .flat_map(|p| {
            let p = p.trim_matches('/').to_string();
            [p.clone(), format!("{p}/**"), format!("**/{p}/**")]
        })
        .filter_map(|p| compile(&p).ok())
        .collect();
    let root_s = root.to_string_lossy().replace('\\', "/");
    let root_s = root_s.trim_end_matches('/').to_string();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for raw in paths {
        let mut p = raw.trim().replace('\\', "/");
        if p.is_empty() {
            continue;
        }
        if let Some(rest) = p.strip_prefix(&format!("{root_s}/")) {
            p = rest.to_string();
        }
        while let Some(rest) = p.strip_prefix("./") {
            p = rest.to_string();
        }
        if p == ".verdict" || p.starts_with(".verdict/") {
            continue;
        }
        if exclude.iter().any(|m| m.is_match(&p)) {
            continue;
        }
        out.insert(p);
    }
    out.into_iter().collect()
}

/// Extensions and names that change no behavior: a diff of only these is the low lane.
fn is_documentation(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if matches!(
        name.as_str(),
        ".gitignore" | ".gitattributes" | "license" | "notice" | "authors" | "contributors"
    ) || name.starts_with("license.")
        || name.starts_with("notice.")
    {
        return true;
    }
    match name.rsplit_once('.') {
        Some((_, ext)) => matches!(
            ext,
            "md" | "markdown"
                | "txt"
                | "rst"
                | "adoc"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "svg"
                | "ico"
                | "webp"
        ),
        None => false,
    }
}

/// What [`classify_lane`] decided and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The lane the diff implies.
    pub lane: Lane,
    /// One line per reason, for the report.
    pub reasons: Vec<String>,
    /// Policy problems met on the way (a bad glob). A bad glob is skipped, never silently trusted.
    pub warnings: Vec<String>,
}

/// Classify changed paths into a lane under `policy`.
///
/// * any path matching `blast.release_if_touches` is **release**;
/// * else any matching `blast.high_if_touches` is **high**;
/// * else any path that is not documentation or an image is **standard**;
/// * else (nothing changed, or only documentation) **low**.
///
/// The number of files and lines never matters.
pub fn classify_lane(policy: &Policy, paths: &[String]) -> Classification {
    let mut warnings = Vec::new();
    let mut compile_all = |pointer: &str| -> Vec<(String, GlobMatcher)> {
        names_of(policy, pointer)
            .into_iter()
            .filter_map(|p| match compile(&p) {
                Ok(m) => Some((p, m)),
                Err(w) => {
                    warnings.push(w);
                    None
                }
            })
            .collect()
    };
    let release = compile_all("/blast/release_if_touches");
    let high = compile_all("/blast/high_if_touches");
    let mut lane = Lane::Low;
    let mut reasons = Vec::new();
    for path in paths {
        let hit = |globs: &[(String, GlobMatcher)]| {
            globs
                .iter()
                .find(|(_, m)| m.is_match(path.as_str()))
                .map(|(p, _)| p.clone())
        };
        if let Some(g) = hit(&release) {
            if lane < Lane::Release {
                lane = Lane::Release;
            }
            reasons.push(format!("{path} matches release_if_touches {g}"));
        } else if let Some(g) = hit(&high) {
            if lane < Lane::High {
                lane = Lane::High;
            }
            reasons.push(format!("{path} matches high_if_touches {g}"));
        } else if !is_documentation(path) {
            if lane < Lane::Standard {
                lane = Lane::Standard;
            }
            reasons.push(format!("{path} changes code or configuration"));
        }
    }
    if reasons.is_empty() {
        reasons.push(if paths.is_empty() {
            "no changed files found".to_string()
        } else {
            "only documentation or images changed".to_string()
        });
    }
    Classification {
        lane,
        reasons,
        warnings,
    }
}

/// The lane after the human's raises: the highest of the diff's lane and every
/// [`DecisionKind::RaiseLane`] decision of the session (or project-wide). A raise below what the
/// diff implies changes nothing; nothing here can lower a lane.
pub fn effective_lane(ledger: &Ledger, session: &str, implied: Lane) -> Result<Lane, LedgerError> {
    let mut lane = implied;
    for s in ledger.records()? {
        if !matches!(&s.session_id, Some(id) if id == session) && s.session_id.is_some() {
            continue;
        }
        if let Record::Decision {
            decision: DecisionKind::RaiseLane,
            choice,
            who: Writer::Human,
            ..
        } = &s.record
            && let Some(l) = parse_lane(choice)
            && l > lane
        {
            lane = l;
        }
    }
    Ok(lane)
}

/// Run git in `root`; `None` when git is missing or the command fails.
pub(crate) fn git_out(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

fn nul_list(bytes: Vec<u8>) -> Vec<String> {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).to_string())
        .collect()
}

/// Paths in the staged index (`git diff --cached`), relative to `root`. Empty when `root` is not a
/// git repository.
pub fn staged_paths(root: &Path) -> Vec<String> {
    git_out(
        root,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "--relative",
            "-z",
        ],
    )
    .map(nul_list)
    .unwrap_or_default()
}

/// The tree hash of the staged index (`git write-tree`), the identity of "this exact staged tree".
pub fn staged_tree(root: &Path) -> Option<String> {
    git_out(root, &["write-tree"])
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Tools whose target is a file the agent changed (the adapters name them `edit` and `write`).
fn is_edit_tool(tool: &str) -> bool {
    matches!(
        tool.to_ascii_lowercase().as_str(),
        "edit" | "write" | "multiedit" | "notebookedit" | "apply_patch"
    )
}

/// The component name a `check_unavailable` record uses when the session's diff could not be read
/// for lane classification.
pub const LANE_COMPONENT: &str = "lane_classification";

/// What [`touched_paths_checked`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touched {
    /// The changed paths it could read.
    pub paths: Vec<String>,
    /// Set (why) when git could not produce the session's diff: a missing baseline commit or a
    /// git error. The paths are then incomplete and the lane must not be trusted to be low.
    pub unreadable: Option<String>,
}

/// Every path the session changed: files named by edit tool calls, plus (in a git repository) the
/// diff of the working tree against the session's baseline commit, the untracked files and the
/// staged index. Not normalized; pass the result through [`normalize_paths`]. A diff git could not
/// produce adds nothing here; use [`touched_paths_checked`] to learn about it.
pub fn touched_paths(
    root: &Path,
    ledger: &Ledger,
    session: &str,
) -> Result<Vec<String>, LedgerError> {
    Ok(touched_paths_checked(root, ledger, session)?.paths)
}

/// Run git and return its output, or why it failed (its own words, first line).
fn git_or_why(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|e| format!("git could not be started: {e}"))?;
    if out.status.success() {
        return Ok(out.stdout);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    Err(format!(
        "`git {}` failed: {}",
        args.first().copied().unwrap_or(""),
        if first.is_empty() {
            format!("exit status {}", out.status)
        } else {
            first.trim().to_string()
        }
    ))
}

/// Like [`touched_paths`], and also says when the session's diff could not be read.
pub fn touched_paths_checked(
    root: &Path,
    ledger: &Ledger,
    session: &str,
) -> Result<Touched, LedgerError> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut unreadable: Option<String> = None;
    let mut base: Option<String> = None;
    for s in ledger.records_for_session(session)? {
        match &s.record {
            Record::Session { baseline, .. } => {
                base = baseline.git.as_ref().and_then(|g| g.head_commit.clone());
            }
            Record::ToolCall { tool, target, .. } if is_edit_tool(tool) => {
                for p in target.split(", ") {
                    let p = p.trim();
                    if !p.is_empty() {
                        out.insert(p.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    if root.join(".git").exists() || git_out(root, &["rev-parse", "--git-dir"]).is_some() {
        let mut why: Vec<String> = Vec::new();
        // The diff against the baseline commit. A repository that had no commit when the session
        // started (and still has none) has nothing to diff against: every file in it is untracked
        // or staged, and both are listed below.
        let base = match base {
            Some(b) => {
                let spec = format!("{b}^{{commit}}");
                match git_or_why(root, &["rev-parse", "--verify", "-q", &spec]) {
                    Ok(_) => Some(b),
                    Err(_) => {
                        why.push(format!(
                            "the session's baseline commit {} is not in this repository",
                            b.get(..12).unwrap_or(&b)
                        ));
                        None
                    }
                }
            }
            None => git_out(root, &["rev-parse", "--verify", "-q", "HEAD^{commit}"])
                .map(|_| "HEAD".to_string()),
        };
        if let Some(base) = base {
            match git_or_why(
                root,
                &[
                    "diff",
                    "--name-only",
                    "--no-renames",
                    "--relative",
                    "-z",
                    &base,
                    "--",
                ],
            ) {
                Ok(b) => out.extend(nul_list(b)),
                Err(e) => why.push(e),
            }
        }
        match git_or_why(root, &["ls-files", "--others", "--exclude-standard", "-z"]) {
            Ok(b) => out.extend(nul_list(b)),
            Err(e) => why.push(e),
        }
        match git_or_why(
            root,
            &[
                "diff",
                "--cached",
                "--name-only",
                "--no-renames",
                "--relative",
                "-z",
            ],
        ) {
            Ok(b) => out.extend(nul_list(b)),
            // Staged paths need a HEAD to diff against; with no commit yet, list the index.
            Err(e) => match git_or_why(root, &["ls-files", "-z"]) {
                Ok(b) => out.extend(nul_list(b)),
                Err(_) => why.push(e),
            },
        }
        if !why.is_empty() {
            unreadable = Some(why.join("; "));
        }
    }
    Ok(Touched {
        paths: out.into_iter().collect(),
        unreadable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn star_and_minus_expand_in_run_order() {
        let high = expand_selection(&strs(&["*", "-firstrun", "-release"]));
        assert_eq!(
            high,
            strs(&[
                "wiring",
                "baseline_diff",
                "regressions",
                "tests_weakened",
                "claims_vs_code",
                "unfinished",
                "exec",
                "mutation"
            ])
        );
        assert_eq!(expand_selection(&strs(&["*"])).len(), ALL_CHECKERS.len());
        // Order is the core's, not the policy's.
        assert_eq!(
            expand_selection(&strs(&["claims_vs_code", "wiring"])),
            strs(&["wiring", "claims_vs_code"])
        );
        // Left to right: a removal before the star is undone by it.
        assert!(expand_selection(&strs(&["-exec", "*"])).contains(&"exec".to_string()));
        assert_eq!(
            expand_selection(&strs(&["custom", "wiring"])),
            strs(&["wiring", "custom"])
        );
    }
}
