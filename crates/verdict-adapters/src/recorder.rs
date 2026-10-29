//! Turning one [`HookEvent`] into ledger records, always as writer `adapter`.
//!
//! Shadow mode only: this module records what happened. It never decides, blocks or denies.
//!
//! | Event | Records |
//! | --- | --- |
//! | any event of a new host session | a `session` record with the baseline |
//! | UserPromptSubmit | a `prompt` record (ordinal counts up per session) |
//! | PreToolUse | none; remembers the start time so Codex tool calls get a duration |
//! | PostToolUse | a `tool_call` record: tool, target, output hash (shell only), duration |
//! | Stop | the agent's final message as a `claim` (source `final_message`), tied to the prompt of the turn |

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use verdict_core::{
    ClaimSource, Ledger, Policy, Record, SessionParams, Writer, find_project_root, start_session,
};

use crate::payload::{
    EventKind, HookEvent, Host, canonical_tool, is_shell, shell_output_text, tool_target,
};
use crate::redact::redact;
use crate::state::{self, Lock, SessionState};

/// What the hook tells the host about this event. Shadow mode always answers [`HookReply::Continue`],
/// so the agent is never held until a human has turned enforce on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookReply {
    /// Nothing to say: print nothing, let the agent continue.
    Continue,
    /// Hold this tool call (enforce mode, pre-edit check).
    Deny(String),
    /// Send the agent back with the finding list (enforce mode, Stop hook).
    SendBack(String),
}

fn bounded(mut text: String, max: usize) -> String {
    if text.len() > max {
        let cut = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|i| *i <= max)
            .last()
            .unwrap_or(0);
        text.truncate(cut);
        text.push_str("\n... (the rest is in the GUI and `vdt report`)");
    }
    text
}

/// The mode in force for this project: the human's switch in the ledger wins, then the policy.
fn enforce_mode(ledger: &Ledger, root: &Path, home: Option<&Path>) -> bool {
    let mode = verdict_core::human::chosen_mode(ledger)
        .ok()
        .flatten()
        .or_else(|| {
            verdict_core::verdict::load_policy(root, home)
                .map(|(p, _)| p.gate_mode().to_string())
                .ok()
        });
    mode.as_deref() == Some("enforce")
}

fn policy_for(root: &Path, home: Option<&Path>) -> Policy {
    verdict_core::verdict::load_policy(root, home)
        .map(|(p, _)| p)
        .or_else(|_| Policy::builtin())
        .unwrap_or_else(|_| Policy::builtin().expect("the built-in policy is valid"))
}

/// Is `target` covered by a claim of this session (its own words or the names it recorded)?
fn claims_cover(ledger: &Ledger, session: &str, target: &str) -> bool {
    let target = target.replace('\\', "/");
    let base = target.rsplit('/').next().unwrap_or(&target);
    let Ok(records) = ledger.records_for_session(session) else {
        return false;
    };
    records.iter().any(|s| match &s.record {
        Record::Claim { text, names, .. } => {
            names.iter().any(|n| {
                let n = n.replace('\\', "/");
                n == target || n.ends_with(&format!("/{base}")) || n == base
            }) || text.contains(&target)
                || (!base.is_empty() && text.contains(base))
        }
        _ => false,
    })
}

fn latest_prompt_names(ledger: &Ledger, session: &str, target: &str) -> bool {
    let target = target.replace('\\', "/");
    let base = target.rsplit('/').next().unwrap_or(&target);
    let Ok(records) = ledger.records_for_session(session) else {
        return false;
    };
    records
        .iter()
        .rev()
        .find_map(|s| match &s.record {
            Record::Prompt { text, .. } => Some(text.clone()),
            _ => None,
        })
        .is_some_and(|p| {
            let p = p.replace('\\', "/");
            p.contains(&target) || (!base.is_empty() && p.contains(base))
        })
}

/// Knobs for tests. `Options::default()` is what the real hook uses.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Folder holding `.verdict/policy.json`; `None` means the user's home folder.
    pub home: Option<PathBuf>,
    /// When this event's work must be done (the hook's own budget, minus a reserve). Fresh
    /// verification is bounded by it so the hook can still record why it stopped.
    pub deadline: Option<std::time::Instant>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn project_root(cwd: Option<&Path>) -> Result<PathBuf, String> {
    let cwd = match cwd {
        Some(c) => c.to_path_buf(),
        None => std::env::current_dir().map_err(|e| format!("no working folder: {e}"))?,
    };
    Ok(find_project_root(&cwd))
}

/// Record one event. On error nothing is half-written that matters: the caller turns the error
/// into a `check_unavailable` record via [`record_unavailable`].
pub fn record_event(ev: &HookEvent, opts: &Options) -> Result<HookReply, String> {
    let sid = ev.session_id.as_deref().ok_or_else(|| {
        "the payload had no session_id, so it cannot be tied to a session".to_string()
    })?;
    let root = project_root(ev.cwd.as_deref())?;
    let state_file = state::state_path(&root, ev.host, sid);
    let _lock = Lock::acquire(&state_file, Duration::from_secs(3))?;
    let mut ledger = Ledger::open(&root).map_err(|e| format!("cannot open the ledger: {e}"))?;

    let mut st = match state::load(&state_file) {
        Some(s) => s,
        None => {
            let model = ev.model.clone().unwrap_or_else(|| "unknown".to_string());
            let started = start_session(
                &mut ledger,
                &root,
                &SessionParams {
                    agent: ev.host.as_str().into(),
                    model: model.clone(),
                    vendor: crate::payload::session_vendor(ev.host, Some(&model)),
                    home: opts.home.clone(),
                },
            )
            .map_err(|e| format!("cannot start the session record: {e}"))?;
            let st = SessionState {
                ledger_session: started.id,
                model,
                ..SessionState::default()
            };
            state::save(&state_file, &st).map_err(|e| format!("cannot save session state: {e}"))?;
            st
        }
    };
    let ledger_sid = st.ledger_session.clone();
    let mut dirty = false;
    let append = |ledger: &mut Ledger, rec: Record| -> Result<(), String> {
        ledger
            .append(Writer::Adapter, Some(&ledger_sid), rec)
            .map(|_| ())
            .map_err(|e| format!("cannot write to the ledger: {e}"))
    };

    let mut reply = HookReply::Continue;
    match ev.kind {
        EventKind::SessionStart => {}
        EventKind::UserPromptSubmit => {
            let Some(prompt) = ev.prompt.clone() else {
                return Err("UserPromptSubmit had no prompt text".into());
            };
            st.ordinal += 1;
            if let Some(t) = &ev.turn_id {
                st.note_turn(t, st.ordinal);
            }
            dirty = true;
            append(
                &mut ledger,
                Record::Prompt {
                    ordinal: st.ordinal,
                    text: redact(&prompt),
                    agent: ev.host.as_str().into(),
                    model: st.model.clone(),
                },
            )?;
        }
        EventKind::PreToolUse => {
            // In enforce mode an edit is held until a claim covers the file the prompt named. The
            // check is a speed bump, not a lock: a claim through `verdict_claim` releases it.
            if enforce_mode(&ledger, &root, opts.home.as_deref())
                && let Some(name) = &ev.tool_name
            {
                let tool = canonical_tool(name);
                if tool == "edit" || tool == "write" {
                    let target = tool_target(&tool, &ev.tool_input, ev.cwd.as_deref());
                    if !target.is_empty()
                        && latest_prompt_names(&ledger, &ledger_sid, &target)
                        && !claims_cover(&ledger, &ledger_sid, &target)
                    {
                        reply = HookReply::Deny(format!(
                            "Verdict is holding this edit: no claim covers {target}. Record one \
                             checkable line naming it with verdict_claim, then try the edit again."
                        ));
                    }
                }
            }
            if let Some(id) = &ev.tool_use_id {
                st.note_pending(id, now_ms());
                dirty = true;
            }
        }
        EventKind::PostToolUse => {
            let Some(name) = &ev.tool_name else {
                return Err("PostToolUse had no tool_name".into());
            };
            let tool = canonical_tool(name);
            let started = ev.tool_use_id.as_ref().and_then(|id| st.pending.remove(id));
            if started.is_some() {
                dirty = true;
            }
            let duration_ms = ev
                .duration_ms
                .or_else(|| started.map(|s| now_ms().saturating_sub(s)))
                .unwrap_or(0);
            let output_hash = if is_shell(&tool) {
                shell_output_text(&ev.tool_response)
                    .map(|t| blake3::hash(t.as_bytes()).to_hex().to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            append(
                &mut ledger,
                Record::ToolCall {
                    target: redact(&tool_target(&tool, &ev.tool_input, ev.cwd.as_deref())),
                    tool,
                    output_hash,
                    duration_ms,
                },
            )?;
        }
        EventKind::Stop => {
            let ordinal = ev
                .turn_id
                .as_ref()
                .and_then(|t| st.turns.get(t).copied())
                .or((st.ordinal > 0).then_some(st.ordinal));
            match &ev.last_message {
                Some(text) => append(
                    &mut ledger,
                    Record::Claim {
                        source: ClaimSource::FinalMessage,
                        text: redact(text),
                        names: Vec::new(),
                        prompt_ordinal: ordinal,
                    },
                )?,
                None => {
                    return Err(
                        "the Stop payload did not carry the agent's final message (last_assistant_message), so it was not recorded"
                            .into(),
                    );
                }
            }
            // The ordinary completion path starts fresh verification: this is the design's Stop
            // sequence (final claim, then verdict_check, then the gate reply). A verification that
            // cannot run leaves a `check_unavailable` record, never a clean result.
            let policy = policy_for(&root, opts.home.as_deref());
            // Leave a little of the hook's own budget for writing the records.
            let mut budget = verdict_engine::stop_timeout(&policy, Duration::from_secs(18))
                .saturating_sub(Duration::from_secs(2))
                .max(Duration::from_secs(1));
            if let Some(deadline) = opts.deadline {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                budget = budget.min(left.max(Duration::from_millis(200)));
            }
            let verified = verdict_engine::verify(
                &root,
                &mut ledger,
                &ledger_sid,
                opts.home.as_deref(),
                &verdict_engine::VerifyOptions {
                    budget: Some(budget),
                    reason: Some("stop hook".into()),
                },
            );
            match verified {
                Err(reason) => {
                    ledger
                        .append(
                            Writer::Core,
                            Some(&ledger_sid),
                            Record::CheckUnavailable {
                                component: "verification".into(),
                                reason: format!(
                                    "fresh verification could not run at Stop: {reason}"
                                ),
                            },
                        )
                        .map_err(|e| format!("cannot record the failed verification: {e}"))?;
                }
                Ok(v) => {
                    // Enforce mode: send the agent back with the findings, at most the policy's
                    // number of times. After that the agent is released and the human is told
                    // through the ledger.
                    if enforce_mode(&ledger, &root, opts.home.as_deref())
                        && v.computed.word == verdict_core::VerdictWord::Failed
                    {
                        let max = policy
                            .value
                            .pointer("/gate/max_sendbacks")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(2) as u32;
                        if st.sendbacks < max {
                            st.sendbacks += 1;
                            dirty = true;
                            let report = verdict_core::report::render_report_with(
                                &ledger,
                                Some(&ledger_sid),
                                Some(&v.computed),
                            )
                            .unwrap_or_else(|_| {
                                "VERDICT: FAILED (the report could not be rendered)".into()
                            });
                            reply = HookReply::SendBack(bounded(
                                format!(
                                    "Verdict found a blocking finding (send-back {} of {max}). Fix it, then say done again.\n\n{report}",
                                    st.sendbacks
                                ),
                                12_000,
                            ));
                        } else {
                            let detail = format!(
                                "send-back limit reached ({max}); the agent was released and the human was told. The verdict stays FAILED (gate record {}).",
                                v.gate_seq
                            );
                            ledger
                                .append(
                                    Writer::Core,
                                    Some(&ledger_sid),
                                    Record::Check {
                                        name: "sendbacks".into(),
                                        tier: verdict_core::Tier::Judgment,
                                        inputs_hash: String::new(),
                                        started_at: verdict_core::timefmt::now_rfc3339(),
                                        finished_at: verdict_core::timefmt::now_rfc3339(),
                                        outcome: verdict_core::CheckOutcome::Ran,
                                        detail: Some(detail),
                                        tree_state: verdict_core::baseline::tree_state_of_ledger(
                                            &ledger,
                                        ),
                                    },
                                )
                                .map_err(|e| format!("cannot record the send-back limit: {e}"))?;
                        }
                    }
                }
            }
        }
    }
    if dirty {
        state::save(&state_file, &st).map_err(|e| format!("cannot save session state: {e}"))?;
    }
    Ok(reply)
}

/// Write a `check_unavailable` record saying why the adapter could not record. Best effort:
/// returns what went wrong if even this failed (the caller then stays silent and fails open).
pub fn record_unavailable(
    host_label: &str,
    cwd: Option<&Path>,
    host_session_id: Option<&str>,
    reason: &str,
) -> Result<(), String> {
    let root = project_root(cwd)?;
    let ledger_sid = Host::parse(host_label)
        .zip(host_session_id)
        .and_then(|(host, sid)| state::load(&state::state_path(&root, host, sid)))
        .map(|s| s.ledger_session);
    // Fail-open: a locked ledger fails immediately instead of waiting, so the hook returns fast;
    // a writable one is written synchronously before the hook exits (no detached writer to be
    // killed mid-write on Windows).
    let mut ledger = Ledger::open_with_busy_timeout(&root, std::time::Duration::ZERO)
        .map_err(|e| format!("cannot open the ledger: {e}"))?;
    ledger
        .append(
            Writer::Adapter,
            ledger_sid.as_deref(),
            Record::CheckUnavailable {
                component: format!(
                    "adapter:{}",
                    host_label.chars().take(40).collect::<String>()
                ),
                reason: reason.to_string(),
            },
        )
        .map(|_| ())
        .map_err(|e| format!("cannot write to the ledger: {e}"))
}
