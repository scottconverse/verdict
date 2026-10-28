//! mutation: the project's configured mutation runner, run only where the policy requires it.
//!
//! A mutation run is the strongest evidence a test suite has teeth, and it is expensive, so it
//! runs only when the policy names a reason (design: `mutation.require_when`, never a lane label
//! or a file count):
//!
//! * `critical_logic_changed` — a changed **logic** file (not a test) touches a path the project
//!   classifies as critical (`mutation.critical_paths`, else `blast.high_if_touches` and
//!   `blast.release_if_touches`). With no classification configured, any changed logic file counts:
//!   missing classification is treated conservatively, never as "nothing critical changed".
//! * `test_sensitivity_uncertain` — the project declares `mutation.test_sensitivity_uncertain:
//!   true`. The tool never guesses this from a file count or a lane.
//!
//! The runner is a command or a report file the project's tooling writes:
//!
//! * `mutation.command`: run in the project under the declared shell and the same human approval
//!   as `exec.commands`. It must print one JSON object with
//!   `{"survivors": [...], "killed": N, "total": N}`. Exit codes other than 0 count only when the
//!   policy lists them in `mutation.success_exit_codes` (a documented tool convention); any other
//!   non-zero ending is a failure, and its output is never read as a clean report.
//! * `mutation.report`: read that JSON from this path instead (a runner in CI may write it).
//!
//! The report is parsed strictly: `survivors` must be a JSON array of objects, `killed` and
//! `total` must be numbers when present, and a total that does not equal killed plus survivors is
//! refused. A survivor's `seed` is kept in its finding. A required mutation run with no runner
//! configured, no approval, a failed command or malformed output is `Unavailable`, never a pass.
//! Tooling that is not installed is recorded as unavailable, not manufactured.

use std::path::Path;

use serde_json::Value;
use verdict_core::exec_approval::policy_shell;
use verdict_core::lane::{path_matches, policy_names};
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::exec::{parse_duration, run_command_in};

/// The mutation checker.
pub struct Mutation;

impl Checker for Mutation {
    fn name(&self) -> &'static str {
        "mutation"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Ran, Tier::Asset]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let command = policy_str(&ctx.policy, "/mutation/command");
        let report = policy_str(&ctx.policy, "/mutation/report");
        if command.is_none() && report.is_none() {
            return Err(Unavailable::new(
                "no mutation runner is configured (set mutation.command or mutation.report)",
            ));
        }
        let reasons = policy_list(&ctx.policy, "/mutation/require_when");
        if reasons.is_empty() {
            ctx.skipped(
                "mutation is configured but mutation.require_when names no reason, so the policy \
                 does not require a mutation run here",
            );
            return Ok(Vec::new());
        }
        let changes = ctx.changes().map_err(|u| Unavailable::new(u.reason))?;
        let changed_logic: Vec<String> = changes
            .files
            .iter()
            .filter(|f| {
                crate::paths::is_code(&f.path)
                    && !crate::paths::is_test_path(&f.path)
                    && !matches!(f.status, crate::changes::Status::Deleted)
            })
            .map(|f| f.path.clone())
            .collect();
        let critical = critical_change(&ctx.policy, &changed_logic);
        let sensitive = ctx
            .policy
            .value
            .pointer("/mutation/test_sensitivity_uncertain")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut why: Vec<String> = Vec::new();
        if reasons.iter().any(|r| r == "critical_logic_changed") && critical {
            why.push(format!(
                "critical logic changed ({})",
                changed_logic
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if reasons.iter().any(|r| r == "test_sensitivity_uncertain") && sensitive {
            why.push("the project declares test_sensitivity_uncertain".into());
        }
        if why.is_empty() {
            ctx.skipped(format!(
                "mutation was not required by mutation.require_when ({}) for this diff: {}",
                reasons.join(", "),
                if changed_logic.is_empty() {
                    "no logic file changed".to_string()
                } else {
                    "no named reason applies".to_string()
                }
            ));
            return Ok(Vec::new());
        }
        ctx.note(format!("mutation required: {}", why.join("; ")));

        let mut drafts = Vec::new();
        let (text, tier, source) = if let Some(report) = &report {
            let path = ctx.root.join(report);
            let text = std::fs::read_to_string(&path).map_err(|e| {
                Unavailable::new(format!("the mutation report {report} cannot be read: {e}"))
            })?;
            ctx.reached(Tier::Asset);
            (text, Tier::Asset, format!("report {report}"))
        } else {
            let command = command.expect("checked above");
            if !ctx.exec_approved {
                return Err(Unavailable::new(
                    "not approved: the human has not decided to run this repo's configured commands",
                ));
            }
            let timeout = policy_str(&ctx.policy, "/mutation/timeout")
                .as_deref()
                .and_then(parse_duration)
                .unwrap_or(ctx.timeout);
            let shell = policy_shell(&ctx.policy).map_err(|e| {
                Unavailable::new(format!(
                    "the project's declared shell cannot be used: {e}; nothing was run"
                ))
            })?;
            let run = run_command_in(&command, &ctx.root, timeout, shell);
            if let Some(e) = &run.spawn_error {
                return Err(Unavailable::new(format!(
                    "the mutation runner could not start: {e}"
                )));
            }
            if run.timed_out {
                return Err(Unavailable::new(format!(
                    "the mutation runner did not finish within {} seconds",
                    timeout.as_secs()
                )));
            }
            let allowed = exit_codes(&ctx.policy);
            let ended_ok = run.clean()
                && run
                    .exit
                    .is_some_and(|code| allowed.contains(&i64::from(code)));
            if !ended_ok {
                return Err(Unavailable::new(format!(
                    "the mutation runner did not end successfully ({}), so its output was not read \
                     as a mutation report",
                    match run.exit {
                        Some(c) => format!(
                            "exit {c}{}",
                            run.problem()
                                .as_ref()
                                .map(|p| format!("; {p}"))
                                .unwrap_or_default()
                        ),
                        None => "it ended without an exit code".to_string(),
                    }
                )));
            }
            ctx.reached(Tier::Ran);
            (
                run.stdout_text.clone(),
                Tier::Ran,
                format!("command `{command}`"),
            )
        };

        let parsed = parse_report(&text).map_err(|e| {
            Unavailable::new(format!(
                "the mutation runner ({source}) produced no usable report: {e}"
            ))
        })?;
        let seed_list: Vec<String> = parsed
            .survivors
            .iter()
            .map(|s| survivor_seed(s).unwrap_or_else(|| "-".into()))
            .collect();
        ctx.note(format!(
            "mutation: {} survivor(s){}{}; seeds: {}",
            parsed.survivors.len(),
            parsed
                .killed
                .map(|k| format!(", {k} killed"))
                .unwrap_or_default(),
            parsed
                .total
                .map(|t| format!(" of {t} mutant(s)"))
                .unwrap_or_default(),
            seed_list.join(", ")
        ));
        for (i, s) in parsed.survivors.iter().enumerate() {
            let file = s.get("file").and_then(Value::as_str);
            let line = s.get("line").and_then(Value::as_u64);
            let description = s
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("a mutant survived the tests");
            let seed = survivor_seed(s);
            let (path, line_no, pointer) = match (file, line) {
                (Some(f), Some(l)) => {
                    let p = Path::new(f);
                    if p.is_absolute() || ctx.read_text(f).is_none() {
                        ("".to_string(), 0, None)
                    } else {
                        (f.to_string(), l, Some(ctx.line_pointer(f, l)))
                    }
                }
                _ => ("".to_string(), 0, None),
            };
            let evidence = pointer
                .or_else(|| {
                    Some(verdict_core::EvidencePointer::command(
                        &format!("mutation {source}"),
                        text.as_bytes(),
                    ))
                })
                .into_iter()
                .collect();
            drafts.push(Draft {
                rule: "survivor",
                severity: Severity::Major,
                title: format!(
                    "mutation survivor {} of {}{}: {}",
                    i + 1,
                    parsed.survivors.len(),
                    seed.as_deref()
                        .map(|s| format!(" (seed {s})"))
                        .unwrap_or_default(),
                    description
                ),
                claim_seq: None,
                evidence,
                tier,
                fix_hint: Some(
                    "add or tighten the test that should have caught this change".into(),
                ),
                path,
                line: line_no,
            });
        }
        Ok(finish(self.name(), drafts))
    }
}

fn survivor_seed(s: &Value) -> Option<String> {
    s.get("seed").and_then(|v| match v {
        Value::String(x) => Some(x.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Whether a changed logic file counts as critical for the mutation rule.
fn critical_change(policy: &Policy, files: &[String]) -> bool {
    if files.is_empty() {
        return false;
    }
    let mut globs = policy_names(policy, "/mutation/critical_paths");
    globs.extend(policy_names(policy, "/blast/high_if_touches"));
    globs.extend(policy_names(policy, "/blast/release_if_touches"));
    if globs.is_empty() {
        // The project has not said what is critical. That is not "nothing is", so a changed logic
        // file is treated as critical: the conservative direction for requiring more evidence.
        return true;
    }
    files.iter().any(|f| path_matches(&globs, f))
}

/// The exit codes the policy names as a successful mutation-runner convention (default `[0]`).
fn exit_codes(policy: &Policy) -> Vec<i64> {
    let listed: Vec<i64> = policy
        .value
        .pointer("/mutation/success_exit_codes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    if listed.is_empty() { vec![0] } else { listed }
}

struct Report {
    survivors: Vec<Value>,
    killed: Option<u64>,
    total: Option<u64>,
}

/// Parse the mutation report strictly. A non-array `survivors`, a non-object survivor, a
/// non-numeric count or an inconsistent total is an error, never a clean empty report.
fn parse_report(text: &str) -> Result<Report, String> {
    let mut starts: Vec<usize> = text.match_indices('{').map(|(i, _)| i).collect();
    starts.reverse();

    for i in starts {
        let mut it = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>();
        let Some(Ok(v)) = it.next() else { continue };
        let Some(survivors) = v.get("survivors") else {
            continue;
        };
        let Some(array) = survivors.as_array() else {
            return Err("`survivors` is not a list".into());
        };
        let mut out = Vec::new();
        for (n, s) in array.iter().enumerate() {
            if !s.is_object() {
                return Err(format!(
                    "survivor {} is not an object with a file, line or description",
                    n + 1
                ));
            }
            out.push(s.clone());
        }
        let count = |key: &str| -> Result<Option<u64>, String> {
            match v.get(key) {
                None => Ok(None),
                Some(Value::Number(n)) => n
                    .as_u64()
                    .map(Some)
                    .ok_or_else(|| format!("`{key}` is not a whole number")),
                Some(_) => Err(format!("`{key}` is not a number")),
            }
        };
        let killed = count("killed")?;
        let total = count("total")?;
        if let (Some(k), Some(t)) = (killed, total)
            && k.saturating_add(out.len() as u64) != t
        {
            return Err(format!(
                "the report is inconsistent: {k} killed plus {} survivor(s) is not the stated total of {t}",
                out.len()
            ));
        }
        return Ok(Report {
            survivors: out,
            killed,
            total,
        });
    }
    Err("it printed no {\"survivors\": [...]} report".into())
}

fn policy_str(policy: &Policy, pointer: &str) -> Option<String> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn policy_list(policy: &Policy, pointer: &str) -> Vec<String> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_is_the_last_object_that_has_survivors() {
        let text = "noise {\"other\": 1}\n{\"survivors\": [], \"killed\": 3, \"total\": 3}\n";
        let r = parse_report(text).unwrap();
        assert_eq!(r.killed, Some(3));
        assert_eq!(r.total, Some(3));
        assert!(parse_report("nothing here").is_err());
        assert!(parse_report("{\"killed\": 1}").is_err());
    }

    #[test]
    fn malformed_reports_are_refused_never_read_as_clean() {
        assert!(parse_report("{\"survivors\": null}").is_err());
        assert!(parse_report("{\"survivors\": [\"a mutant\"]}").is_err());
        assert!(parse_report("{\"survivors\": [], \"killed\": \"3\"}").is_err());
        // A total that does not add up is malformed output, not a clean run.
        assert!(parse_report("{\"survivors\": [], \"killed\": 3, \"total\": 4}").is_err());
        assert!(parse_report("{\"survivors\": [{\"seed\": 7}]}").is_ok());
    }
}
