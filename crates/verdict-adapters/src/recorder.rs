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
    ClaimSource, Ledger, Record, SessionParams, Writer, find_project_root, start_session,
};

use crate::payload::{
    EventKind, HookEvent, Host, canonical_tool, is_shell, shell_output_text, tool_target,
};
use crate::redact::redact;
use crate::state::{self, Lock, SessionState};

/// Knobs for tests. `Options::default()` is what the real hook uses.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Folder holding `.verdict/policy.json`; `None` means the user's home folder.
    pub home: Option<PathBuf>,
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
pub fn record_event(ev: &HookEvent, opts: &Options) -> Result<(), String> {
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
        }
    }
    if dirty {
        state::save(&state_file, &st).map_err(|e| format!("cannot save session state: {e}"))?;
    }
    Ok(())
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
    let mut ledger = Ledger::open(&root).map_err(|e| format!("cannot open the ledger: {e}"))?;
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
