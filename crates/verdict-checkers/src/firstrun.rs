//! firstrun: the project's configured first-run workflow, run for real.
//!
//! The design's first-run proof is a walk from an empty state on a machine, with the evidence
//! recorded. This checker runs the commands the project lists under `firstrun.commands` (each a
//! string, or an object with `run` and an optional `needs`), in order, under the same human
//! approval as every other project command. A command that fails is a `Blocker` when the release
//! lane is judging and a `Major` otherwise; a command that needs a server Verdict does not start is
//! recorded as not established rather than skipped silently.
//!
//! The clean-container walk itself is a release activity (`docs/RELEASE.md`); this checker records
//! the configured commands' evidence, never a manufactured pass.

use serde_json::Value;
use verdict_core::{Finding, Lane, Policy, Severity, Tier};

use crate::Checker;
use crate::context::{CheckContext, Draft, Unavailable, finish};
use crate::exec::{parse_duration, run_command};

/// The first-run checker.
pub struct FirstRun;

impl Checker for FirstRun {
    fn name(&self) -> &'static str {
        "firstrun"
    }

    fn tiers(&self) -> &'static [Tier] {
        &[Tier::Ran]
    }

    fn applies(&self, policy: &Policy, lane: Lane) -> bool {
        crate::lane_selects(policy, lane, self.name())
    }

    fn run(&self, ctx: &CheckContext) -> Result<Vec<Finding>, Unavailable> {
        let commands = commands(&ctx.policy);
        if commands.is_empty() {
            return Err(Unavailable::new(
                "no first-run workflow is configured (set firstrun.commands in verdict.json)",
            ));
        }
        if !ctx.exec_approved {
            return Err(Unavailable::new(
                "not approved: the human has not decided to run this repo's configured commands",
            ));
        }
        let timeout = policy_duration(&ctx.policy, "/firstrun/timeout").unwrap_or(ctx.timeout);
        let mut drafts: Vec<Draft> = Vec::new();
        let mut ran = 0usize;
        for (i, (command, needs)) in commands.iter().enumerate() {
            if let Some(needs) = needs {
                ctx.unestablished(format!(
                    "first-run command {} needs a {needs}, which this build does not start",
                    i + 1
                ));
                continue;
            }
            let run = run_command(command, &ctx.root, timeout);
            ran += 1;
            let problem = if let Some(e) = &run.spawn_error {
                Some(format!("it could not start: {e}"))
            } else if run.timed_out {
                Some(format!(
                    "it did not finish within {} seconds",
                    timeout.as_secs()
                ))
            } else if run.exit != Some(0) {
                Some(match run.exit {
                    Some(code) => format!("it exited {code}"),
                    None => "it ended without an exit code".to_string(),
                })
            } else {
                run.problem()
            };
            match problem {
                None => ctx.note(format!(
                    "first-run command {} ran and passed: {command}",
                    i + 1
                )),
                Some(why) => {
                    ctx.note(format!("first-run command {} failed: {command}", i + 1));
                    let severity = if ctx.lane == Lane::Release {
                        Severity::Blocker
                    } else {
                        Severity::Major
                    };
                    drafts.push(Draft {
                        rule: "command-failed",
                        severity,
                        title: format!(
                            "first-run command {} `{command}` did not pass: {why}",
                            i + 1
                        ),
                        claim_seq: None,
                        evidence: vec![verdict_core::EvidencePointer::command(
                            command,
                            run.tail.as_bytes(),
                        )],
                        tier: Tier::Ran,
                        fix_hint: Some(
                            "make the documented first-run path work from an empty state".into(),
                        ),
                        path: String::new(),
                        line: 0,
                    });
                }
            }
        }
        if ran > 0 {
            ctx.reached(Tier::Ran);
        }
        Ok(finish(self.name(), drafts))
    }
}

fn commands(policy: &Policy) -> Vec<(String, Option<String>)> {
    policy
        .value
        .pointer("/firstrun/commands")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| match v {
                    Value::String(s) if !s.trim().is_empty() => Some((s.trim().to_string(), None)),
                    Value::Object(o) => {
                        let run = o.get("run").and_then(Value::as_str)?.trim().to_string();
                        if run.is_empty() {
                            return None;
                        }
                        let needs = o.get("needs").and_then(Value::as_str).map(str::to_string);
                        Some((run, needs))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn policy_duration(policy: &Policy, pointer: &str) -> Option<std::time::Duration> {
    policy
        .value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(parse_duration)
}
