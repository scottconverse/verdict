//! mutation: the project's configured mutation runner, run only where the policy requires it.
//!
//! A mutation run is the strongest evidence a test suite has teeth, and it is expensive, so it
//! runs only when the policy names a reason (design: `mutation.require_when`, never a lane label
//! or a file count). The runner is a command or a report file the project's tooling writes:
//!
//! * `mutation.command`: run in the project, under the same human approval as `exec.commands`.
//!   It must print one JSON object with `{"survivors": [...], "killed": N, "total": N}`.
//! * `mutation.report`: read that JSON from this path instead (a runner in CI may write it).
//!
//! A survivor is a `Major` finding with a `file:line` pointer when it names one. A required
//! mutation run with no runner configured, no approval, or unreadable output is `Unavailable`,
//! never a pass. Tooling that is not installed is recorded as unavailable, not manufactured.

use std::path::Path;

use serde_json::Value;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::exec::{parse_duration, run_command};

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
        // The policy's proportionality rule: mutation runs only when a reason it names is present.
        let reasons = policy_list(&ctx.policy, "/mutation/require_when");
        if reasons.is_empty() {
            ctx.note("mutation is configured but mutation.require_when names no reason, so it was not required");
            return Ok(Vec::new());
        }
        let changes = ctx.changes().map_err(|u| Unavailable::new(u.reason))?;
        let logic_changed = changes.files.iter().any(|f| {
            crate::paths::is_code(&f.path)
                && !crate::paths::is_test_path(&f.path)
                && !matches!(f.status, crate::changes::Status::Deleted)
        });
        // A configured report is the project's own runner's output: reading it is evidence that a
        // run happened, and a survivor in it proves logic was mutated. Only the command mode needs
        // the changed-logic heuristic before it spends time running anything.
        if !logic_changed && report.is_none() {
            ctx.note("mutation was not required: no logic file changed");
            return Ok(Vec::new());
        }

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
            let run = run_command(&command, &ctx.root, timeout);
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
            if let Some(problem) = run.problem() {
                ctx.note(format!("mutation runner: {problem}"));
            }
            ctx.reached(Tier::Ran);
            (
                run.stdout_text.clone(),
                Tier::Ran,
                format!("command `{command}`"),
            )
        };

        let parsed = last_report(&text).ok_or_else(|| {
            Unavailable::new(format!(
                "the mutation runner ({source}) printed no {{\"survivors\": [...]}} report"
            ))
        })?;
        let survivors = parsed
            .get("survivors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let killed = parsed.get("killed").and_then(Value::as_u64);
        let total = parsed.get("total").and_then(Value::as_u64);
        ctx.note(format!(
            "mutation: {} survivor(s){}{}",
            survivors.len(),
            killed.map(|k| format!(", {k} killed")).unwrap_or_default(),
            total
                .map(|t| format!(" of {t} mutant(s)"))
                .unwrap_or_default()
        ));
        for (i, s) in survivors.iter().enumerate() {
            let file = s.get("file").and_then(Value::as_str);
            let line = s.get("line").and_then(Value::as_u64);
            let description = s
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("a mutant survived the tests");
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
                    "mutation survivor {} of {}: {}",
                    i + 1,
                    survivors.len(),
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

/// The last JSON object in `text` that carries a `survivors` key.
fn last_report(text: &str) -> Option<Value> {
    let mut starts: Vec<usize> = text.match_indices('{').map(|(i, _)| i).collect();
    starts.reverse();
    for i in starts {
        let mut it = serde_json::Deserializer::from_str(&text[i..]).into_iter::<Value>();
        if let Some(Ok(v)) = it.next()
            && v.get("survivors").is_some()
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_is_the_last_object_that_has_survivors() {
        let text = "noise {\"other\": 1}\n{\"survivors\": [], \"killed\": 3}\n";
        assert_eq!(last_report(text).unwrap()["killed"], serde_json::json!(3));
        assert!(last_report("nothing here").is_none());
        assert!(last_report("{\"killed\": 1}").is_none());
    }
}
