//! Reading a host's hook payload (JSON on stdin) into one host-neutral [`HookEvent`].
//!
//! Both hosts send a JSON object with `session_id`, `cwd` and `hook_event_name`, plus fields that
//! depend on the event. Codex adds `turn_id` and `model` to every turn-scoped event; Claude Code
//! sends `prompt_id` (used here as the turn id) and `model` only on SessionStart. Nothing here
//! decides anything: it only reads.

use serde_json::Value;
use std::path::PathBuf;

/// The agent host that sent the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Claude Code.
    Claude,
    /// OpenAI Codex.
    Codex,
    /// Cursor (agent chat).
    Cursor,
    /// Google Antigravity.
    Antigravity,
    /// DeepSeek Harness (optional host: the MCP connection is its primary path; the hook command
    /// exists for a harness that can call one).
    Dsh,
}

impl Host {
    /// Parse the command-line name (`claude`, `codex`, `cursor`, `antigravity`, `dsh`),
    /// case-insensitively.
    pub fn parse(s: &str) -> Option<Host> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Host::Claude),
            "codex" => Some(Host::Codex),
            "cursor" => Some(Host::Cursor),
            "antigravity" | "antigravity-cli" => Some(Host::Antigravity),
            "dsh" | "deepseek-harness" => Some(Host::Dsh),
            _ => None,
        }
    }

    /// Stable lower-case name; also the `agent` written to the ledger.
    pub fn as_str(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Codex => "codex",
            Host::Cursor => "cursor",
            Host::Antigravity => "antigravity",
            Host::Dsh => "dsh",
        }
    }

    /// The model vendor recorded in the session record. Cursor and DSH serve models from several
    /// vendors, so their vendor is derived from the model; `unknown` when the payload does not say.
    pub fn vendor(self) -> &'static str {
        match self {
            Host::Claude => "anthropic",
            Host::Codex => "openai",
            Host::Cursor | Host::Dsh => "unknown",
            Host::Antigravity => "google",
        }
    }

    /// The host's own event name (Cursor and Antigravity spell some of them differently).
    pub fn event_name(self, kind: EventKind) -> &'static str {
        match self {
            Host::Cursor => match kind {
                EventKind::SessionStart => "sessionStart",
                EventKind::UserPromptSubmit => "beforeSubmitPrompt",
                EventKind::PreToolUse => "preToolUse",
                EventKind::PostToolUse => "postToolUse",
                EventKind::Stop => "stop",
            },
            // Antigravity has no prompt-submit event. Its PreInvocation fires before the model is
            // called and is where the prompt reaches a hook; the field names it sends are not
            // verified yet (docs/HOSTS.md).
            Host::Antigravity => match kind {
                EventKind::UserPromptSubmit => "PreInvocation",
                _ => kind.as_str(),
            },
            _ => kind.as_str(),
        }
    }

    /// Where a project's hook registration lives, as a path relative to the project.
    pub fn project_hooks_file(self) -> &'static str {
        match self {
            Host::Claude => ".claude/settings.json",
            Host::Codex => ".codex/hooks.json",
            Host::Cursor => ".cursor/hooks.json",
            Host::Antigravity => ".agents/hooks.json",
            // DSH's own configuration is host-managed; docs/DSH.md explains the options.
            Host::Dsh => "the harness's own configuration",
        }
    }

    /// The hosts the page offers to install hooks for (DSH is host-managed; see docs/DSH.md).
    pub const ALL: [Host; 4] = [Host::Claude, Host::Codex, Host::Cursor, Host::Antigravity];
}

/// The vendor of a model name, for a host that serves more than one vendor.
pub fn vendor_of_model(model: &str) -> Option<&'static str> {
    let m = model.to_ascii_lowercase();
    if m.contains("claude") || m.contains("anthropic") {
        Some("anthropic")
    } else if m.contains("gpt")
        || (m.starts_with('o') && m.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
    {
        Some("openai")
    } else if m.contains("gemini") || m.contains("google") {
        Some("google")
    } else if m.contains("deepseek") {
        Some("deepseek")
    } else if m.contains("llama") || m.contains("qwen") || m.contains("mistral") {
        Some("local")
    } else {
        None
    }
}

/// The vendor to record for a session: the host's own, refined by the model when the host can
/// serve several vendors.
pub fn session_vendor(host: Host, model: Option<&str>) -> String {
    match host {
        Host::Cursor | Host::Dsh => model
            .and_then(vendor_of_model)
            .unwrap_or("unknown")
            .to_string(),
        _ => host.vendor().to_string(),
    }
}

/// The five events the adapters record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// A session began.
    SessionStart,
    /// The human submitted a prompt.
    UserPromptSubmit,
    /// A tool is about to run.
    PreToolUse,
    /// A tool ran.
    PostToolUse,
    /// The agent says it is done with the turn.
    Stop,
}

impl EventKind {
    /// Parse `SessionStart`, `session-start`, `session_start` and so on.
    pub fn parse(s: &str) -> Option<EventKind> {
        let key: String = s
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .collect::<String>()
            .to_ascii_lowercase();
        match key.as_str() {
            "sessionstart" => Some(EventKind::SessionStart),
            "userpromptsubmit" | "beforesubmitprompt" | "preinvocation" => {
                Some(EventKind::UserPromptSubmit)
            }
            "pretooluse" => Some(EventKind::PreToolUse),
            "posttooluse" => Some(EventKind::PostToolUse),
            "stop" => Some(EventKind::Stop),
            _ => None,
        }
    }

    /// The host's own spelling (the same for both hosts).
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::SessionStart => "SessionStart",
            EventKind::UserPromptSubmit => "UserPromptSubmit",
            EventKind::PreToolUse => "PreToolUse",
            EventKind::PostToolUse => "PostToolUse",
            EventKind::Stop => "Stop",
        }
    }

    /// Every event, in registration order.
    pub const ALL: [EventKind; 5] = [
        EventKind::SessionStart,
        EventKind::UserPromptSubmit,
        EventKind::PreToolUse,
        EventKind::PostToolUse,
        EventKind::Stop,
    ];
}

/// One hook payload, host-neutral. Every field the host did not send is `None`.
#[derive(Debug, Clone)]
pub struct HookEvent {
    /// Who sent it.
    pub host: Host,
    /// Which event.
    pub kind: EventKind,
    /// The host's raw session id.
    pub session_id: Option<String>,
    /// The host's raw turn id (`turn_id` on Codex, `prompt_id` on Claude Code).
    pub turn_id: Option<String>,
    /// The session's working folder.
    pub cwd: Option<PathBuf>,
    /// The model name, when the host says it.
    pub model: Option<String>,
    /// The prompt, word for word (UserPromptSubmit).
    pub prompt: Option<String>,
    /// The tool's name as the host spells it.
    pub tool_name: Option<String>,
    /// The tool's input, as sent.
    pub tool_input: Value,
    /// The tool's output, as sent (PostToolUse).
    pub tool_response: Value,
    /// The host's id for this tool call.
    pub tool_use_id: Option<String>,
    /// Milliseconds the tool took, when the host says (Claude Code PostToolUse).
    pub duration_ms: Option<u64>,
    /// The agent's final message (Stop).
    pub last_message: Option<String>,
    /// True when the host is already continuing because of a Stop hook.
    pub stop_hook_active: bool,
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// The first of `keys` that holds a non-empty string. Cursor and Antigravity spell some fields
/// differently from Claude Code and Codex, and neither spelling is verified against a live host
/// yet (see `docs/HOSTS.md`), so both are read and a missing one stays `None`.
fn first_text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| text(v, k))
}

fn first_value(v: &Value, keys: &[&str]) -> Value {
    keys.iter()
        .find_map(|k| v.get(*k).cloned())
        .unwrap_or(Value::Null)
}

fn model_name(v: &Value) -> Option<String> {
    match v.get("model") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Object(o)) => ["id", "name", "slug", "display_name"]
            .iter()
            .find_map(|k| o.get(*k).and_then(Value::as_str))
            .map(str::to_string),
        _ => None,
    }
}

/// Parse a payload. Fails only when the bytes are not a JSON object.
pub fn parse_payload(host: Host, kind: EventKind, bytes: &[u8]) -> Result<HookEvent, String> {
    let v: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("payload is not JSON: {e}"))?;
    if !v.is_object() {
        return Err("payload is JSON but not an object".into());
    }
    Ok(HookEvent {
        host,
        kind,
        session_id: first_text(
            &v,
            &[
                "session_id",
                "sessionId",
                "conversation_id",
                "conversationId",
                "thread_id",
            ],
        ),
        turn_id: first_text(
            &v,
            &[
                "turn_id",
                "turnId",
                "prompt_id",
                "promptId",
                "generation_id",
                "generationId",
            ],
        ),
        cwd: first_text(&v, &["cwd", "workspace", "workspace_path", "workspaceRoot"])
            .map(PathBuf::from),
        model: model_name(&v)
            .or_else(|| v.get("model").and_then(Value::as_str).map(str::to_string)),
        prompt: first_text(&v, &["prompt", "user_prompt", "userPrompt", "text"]),
        tool_name: first_text(&v, &["tool_name", "toolName", "tool"]),
        tool_input: first_value(&v, &["tool_input", "toolInput", "arguments", "input"]),
        tool_response: first_value(
            &v,
            &[
                "tool_response",
                "toolResponse",
                "tool_output",
                "toolOutput",
                "result",
                "output",
            ],
        ),
        tool_use_id: first_text(&v, &["tool_use_id", "toolUseId", "tool_call_id", "call_id"]),
        duration_ms: v
            .get("duration_ms")
            .or_else(|| v.get("durationMs"))
            .and_then(Value::as_u64),
        last_message: first_text(
            &v,
            &[
                "last_assistant_message",
                "lastAssistantMessage",
                "message",
                "response",
                "text",
            ],
        ),
        stop_hook_active: v
            .get("stop_hook_active")
            .or_else(|| v.get("stopHookActive"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// How the adapters name a tool in the ledger, the same on both hosts.
///
/// Shell tools become `shell`, file-edit tools `edit` (`Write` stays `write`), so one logical
/// session gives the same records on both hosts. Everything else keeps the host's name.
pub fn canonical_tool(name: &str) -> String {
    match name {
        "Bash" | "PowerShell" | "exec_command" | "shell_command" | "shell" | "local_shell"
        | "container.exec" | "run_command" | "runCommand" | "Shell" => "shell".into(),
        "Edit"
        | "MultiEdit"
        | "NotebookEdit"
        | "apply_patch"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "edit_file"
        | "ApplyPatch" => "edit".into(),
        "Write" | "write_to_file" => "write".into(),
        other => other.into(),
    }
}

/// True for the shell tools.
pub fn is_shell(canonical: &str) -> bool {
    canonical == "shell"
}

const TARGET_LIMIT: usize = 400;

fn clip(s: &str) -> String {
    if s.chars().count() <= TARGET_LIMIT {
        return s.to_string();
    }
    let head: String = s.chars().take(TARGET_LIMIT).collect();
    format!("{head}... [{} characters in all]", s.chars().count())
}

/// `path` relative to `base` with forward slashes, when it is inside `base`; else `path` with
/// forward slashes. (Claude Code sends absolute paths with backslashes on Windows; Codex patches
/// are relative. This makes the same edit read the same on both.)
pub fn relative_to(path: &str, base: Option<&std::path::Path>) -> String {
    let norm = |s: &str| s.replace('\\', "/");
    let p = norm(path);
    if let Some(b) = base {
        let b = norm(&b.to_string_lossy());
        let b = b.trim_end_matches('/');
        if let Some(rest) = p.strip_prefix(b)
            && let Some(rest) = rest.strip_prefix('/')
        {
            return rest.to_string();
        }
    }
    p
}

/// File paths named by a Codex `apply_patch` body.
pub fn patch_paths(patch: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in patch.lines() {
        let line = line.trim_end_matches('\r');
        for prefix in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(p) = line.strip_prefix(prefix) {
                let p = p.trim().to_string();
                if !p.is_empty() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// The target of a tool call: the command for a shell, the file path(s) for an edit, else the
/// first of the usual fields (path, url, pattern, query). Clipped to 400 characters.
pub fn tool_target(canonical: &str, input: &Value, base: Option<&std::path::Path>) -> String {
    let str_of = |k: &str| input.get(k).and_then(Value::as_str).map(str::to_string);
    let raw = match input {
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    if is_shell(canonical) {
        let cmd = str_of("command")
            .or_else(|| str_of("cmd"))
            .or_else(|| str_of("CommandLine"))
            .or_else(|| match input.get("command").or_else(|| input.get("cmd")) {
                Some(Value::Array(a)) => Some(
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                _ => None,
            })
            .or(raw);
        return clip(&cmd.unwrap_or_default());
    }
    if canonical == "edit" || canonical == "write" {
        if let Some(p) = str_of("file_path")
            .or_else(|| str_of("notebook_path"))
            .or_else(|| str_of("path"))
            .or_else(|| str_of("TargetFile"))
        {
            return clip(&relative_to(&p, base));
        }
        let body = str_of("command")
            .or_else(|| str_of("patch"))
            .or_else(|| str_of("input"))
            .or(raw);
        if let Some(body) = body {
            let paths = patch_paths(&body);
            if !paths.is_empty() {
                return clip(&paths.join(", "));
            }
        }
        return String::new();
    }
    for k in ["file_path", "path", "url", "pattern", "query", "command"] {
        if let Some(v) = str_of(k) {
            return clip(&v);
        }
    }
    String::new()
}

/// The text of a shell tool's output, when the host gave one.
///
/// Claude Code sends an object with `stdout` and `stderr`; Codex sends the model-facing output,
/// usually a string. The text hashed is stdout followed by stderr.
pub fn shell_output_text(response: &Value) -> Option<String> {
    match response {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            let out = o.get("stdout").and_then(Value::as_str);
            let err = o.get("stderr").and_then(Value::as_str);
            if out.is_some() || err.is_some() {
                return Some(format!("{}{}", out.unwrap_or(""), err.unwrap_or("")));
            }
            ["output", "content", "text"]
                .iter()
                .find_map(|k| o.get(*k).and_then(Value::as_str))
                .map(str::to_string)
        }
        _ => None,
    }
}

/// Just the working folder and session id of a payload, for the case where the event or host is
/// not one the adapters know but the failure should still be recorded against the right session.
pub fn peek_context(bytes: &[u8]) -> (Option<PathBuf>, Option<String>) {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => (text(&v, "cwd").map(PathBuf::from), text(&v, "session_id")),
        Err(_) => (None, None),
    }
}
