//! Verdict's own bounded mutation runner.
//!
//! This is **project tooling**, not part of the shipped product. The project's policy configures it
//! as `mutation.command`; the mutation checker runs it and reads the JSON it prints.
//!
//! Safety and honesty rules:
//! * It works in a throwaway `git worktree` of the current commit. A dirty working tree is refused,
//!   so the run always describes the candidate that was committed.
//! * It first runs every targeted test unmutated and refuses to continue if a baseline fails.
//! * Each mutation has a stable id/seed, one exact replacement, and a targeted test. A mutation is
//!   `killed` only when the test fails for a reason other than a compile/build error. A build
//!   failure or a timeout is an `error`, and the runner aborts with a nonzero exit.
//! * Every file is restored from git after each mutation and the worktree is verified clean. The
//!   worktree is removed on success, failure and panic.
//!
//! The mutation set is deliberately small and documented here. It is bounded coverage, not a claim
//! of full mutation testing.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MUTATION_TIMEOUT: Duration = Duration::from_secs(300);

struct Mutation {
    seed: &'static str,
    file: &'static str,
    old: &'static str,
    new: &'static str,
    /// `cargo test` arguments before the `--` filter.
    package: &'static str,
    target: Target,
    filter: &'static str,
    description: &'static str,
}

enum Target {
    Integration(&'static str),
    Lib,
}

const MUTATIONS: &[Mutation] = &[
    Mutation {
        seed: "m1-blocking-threshold",
        file: "crates/verdict-core/src/verdict.rs",
        old: "let serious = severity_rank(f.severity) <= severity_rank(Severity::Major);",
        new: "let serious = severity_rank(f.severity) <= severity_rank(Severity::Blocker);",
        package: "verdict-core",
        target: Target::Integration("verdict_word"),
        filter: "a_blocking_finding_is_failed_even_when_other_checkers_are_clean",
        description: "a Major finding stops counting as blocking",
    },
    Mutation {
        seed: "m2-release-bar",
        file: "crates/verdict-core/src/lane.rs",
        old: "Some(\"blockers_only\") => (Bar::BlockersOnly, None),",
        new: "Some(\"blockers_only\") => (Bar::ZeroAboveNit, None),",
        package: "verdict-core",
        target: Target::Integration("lanes"),
        filter: "a_policy_declared_blockers_only_bar_is_read_as_blockers_only",
        description: "the blockers_only release bar silently becomes zero_above_nit",
    },
    Mutation {
        seed: "m3-sha-constant",
        file: "crates/verdict-core/src/sha256.rs",
        old: "0x6a09e667, 0xbb67ae85,",
        new: "0x6a09e667, 0xbb67ae86,",
        package: "verdict-core",
        target: Target::Lib,
        filter: "sha256::tests::known_vectors",
        description: "one SHA-256 initial constant is changed",
    },
];

struct Worktree {
    path: PathBuf,
    repo: PathBuf,
}

impl Worktree {
    fn create(repo: &Path) -> Result<Worktree, String> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("vdt-mutation-{}-{stamp}", std::process::id()));
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg("HEAD")
            .output()
            .map_err(|e| format!("could not start git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(Worktree {
            path,
            repo: repo.to_path_buf(),
        })
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .output();
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("could not start git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn target_dir(repo: &Path) -> PathBuf {
    repo.join("target")
}

fn test_args(m: &Mutation) -> Vec<String> {
    let mut args = vec![
        "test".to_string(),
        "--locked".to_string(),
        "--quiet".to_string(),
        "-p".to_string(),
        m.package.to_string(),
    ];
    if let Target::Integration(name) = m.target {
        args.push("--test".into());
        args.push(name.into());
    } else {
        args.push("--lib".into());
    }
    args.push("--target-dir".into());
    args
}

fn run_test(worktree: &Path, m: &Mutation, target: &Path) -> Result<(bool, String), String> {
    let mut args = test_args(m);
    args.push(target.to_string_lossy().to_string());
    args.push("--".into());
    args.push(m.filter.into());
    let mut child = Command::new("cargo")
        .args(&args)
        .current_dir(worktree)
        .env("CARGO_TERM_COLOR", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start cargo {}: {e}", args.join(" ")))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = child
                    .wait_with_output()
                    .map_err(|e| format!("could not read cargo output: {e}"))?;
                let mut text = String::from_utf8_lossy(&out.stdout).to_string();
                text.push_str(&String::from_utf8_lossy(&out.stderr));
                return Ok((status.success(), text));
            }
            Ok(None) => {
                if started.elapsed() > MUTATION_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "the targeted test did not finish within {} s",
                        MUTATION_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("could not wait for cargo: {e}")),
        }
    }
}

fn compile_error(text: &str) -> bool {
    text.contains("error[E")
        || text.contains("could not compile")
        || text.contains("error: could not compile")
}

fn replace_once(text: &str, old: &str, new: &str) -> Result<String, String> {
    let count = text.matches(old).count();
    if count != 1 {
        return Err(format!(
            "the mutation target appears {count} times, expected exactly once"
        ));
    }
    Ok(text.replacen(old, new, 1))
}

fn line_of(text: &str, needle: &str) -> u64 {
    let at = text.find(needle).unwrap_or(0);
    text[..at].matches('\n').count() as u64 + 1
}

fn clean(worktree: &Path) -> Result<(), String> {
    let status = git(worktree, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(format!(
            "the worktree is not clean after restoring the mutation:\n{status}"
        ));
    }
    Ok(())
}

fn main() {
    match run() {
        Ok(report) => println!("{report}"),
        Err(e) => {
            eprintln!("vdt-mutation-runner: {e}");
            std::process::exit(1);
        }
    }
}

fn run() -> Result<String, String> {
    let repo = std::env::args()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
    let repo = PathBuf::from(git(&repo, &["rev-parse", "--show-toplevel"])?.trim());
    if !git(&repo, &["status", "--porcelain"])?.trim().is_empty() {
        return Err(
            "the working tree has uncommitted changes; commit the candidate before running the \
             mutation runner"
                .into(),
        );
    }
    let worktree = Worktree::create(&repo)?;
    let target = target_dir(&repo);

    // Baseline: every targeted test must pass before any mutation is applied.
    for m in MUTATIONS {
        let (ok, text) = run_test(&worktree.path, m, &target)?;
        if !ok {
            return Err(format!(
                "baseline test `{}` failed; the runner will not treat anything as killed against a \
                 failing baseline:\n{}",
                m.filter,
                tail(&text)
            ));
        }
    }

    let mut killed = 0u64;
    let mut survivors = Vec::new();
    for m in MUTATIONS {
        let path = worktree.path.join(m.file);
        let original =
            std::fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", m.file))?;
        let line = line_of(&original, m.old);
        let mutated = replace_once(&original, m.old, m.new)?;
        std::fs::write(&path, &mutated).map_err(|e| format!("cannot write {}: {e}", m.file))?;
        let result = run_test(&worktree.path, m, &target);
        // Restore even when the test run itself failed to start.
        let restore =
            git(&worktree.path, &["checkout", "--", m.file]).and_then(|_| clean(&worktree.path));
        if let Err(e) = restore {
            return Err(format!("restoration failed after {}: {e}", m.seed));
        }
        let (ok, text) = result?;
        if ok {
            survivors.push(serde_json::json!({
                "seed": m.seed,
                "file": m.file,
                "line": line,
                "description": m.description,
            }));
        } else if compile_error(&text) {
            return Err(format!(
                "mutation {} produced a compile/build failure, which is an error, not a killed \
                 mutation:\n{}",
                m.seed,
                tail(&text)
            ));
        } else {
            killed += 1;
        }
    }
    let total = MUTATIONS.len() as u64;
    let report = serde_json::json!({
        "mutations": MUTATIONS.iter().map(|m| serde_json::json!({
            "seed": m.seed, "file": m.file, "description": m.description,
        })).collect::<Vec<_>>(),
        "killed": killed,
        "total": total,
        "survivors": survivors,
    });
    Ok(report.to_string())
}

fn tail(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().rev().take(25).collect();
    lines.reverse();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replacement_must_be_unique() {
        assert_eq!(replace_once("a b", "a", "c").unwrap(), "c b");
        assert!(replace_once("a a", "a", "c").is_err());
        assert!(replace_once("a", "z", "c").is_err());
    }

    #[test]
    fn a_compile_failure_is_not_a_kill() {
        assert!(compile_error("error[E0425]: cannot find value"));
        assert!(compile_error("error: could not compile `x`"));
        assert!(!compile_error("assertion failed: left == right"));
    }
}
