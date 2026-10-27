//! The hook block a human adds to a host's settings to install Verdict's hooks.
//!
//! Part 1b owns the real installer (`vdt hooks install`). Until it is merged this page records
//! the decision and shows the block; it never edits the host's files itself.

/// A host whose hooks the page can offer to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Claude Code.
    ClaudeCode,
    /// Codex.
    Codex,
}

impl Host {
    /// Both hosts, in page order.
    pub const ALL: [Host; 2] = [Host::ClaudeCode, Host::Codex];

    /// The stable key stored as the decision's choice.
    pub fn key(self) -> &'static str {
        match self {
            Host::ClaudeCode => "claude_code",
            Host::Codex => "codex",
        }
    }

    /// Parse the stable key.
    pub fn parse(s: &str) -> Option<Host> {
        Host::ALL.into_iter().find(|h| h.key() == s)
    }

    /// The name shown to the human.
    pub fn title(self) -> &'static str {
        match self {
            Host::ClaudeCode => "Claude Code",
            Host::Codex => "Codex",
        }
    }

    /// Where the block goes.
    pub fn target_file(self) -> &'static str {
        match self {
            Host::ClaudeCode => ".claude/settings.json in this project",
            Host::Codex => "hooks.json in your Codex home folder (normally ~/.codex/hooks.json)",
        }
    }

    /// The exact block to add: three hooks, each calling `vdt hook <host> <event>`.
    /// The Stop hook has a 20 second timeout, as the design says.
    pub fn block(self) -> String {
        let (name, tools) = match self {
            Host::ClaudeCode => ("claude", "Edit|Write|MultiEdit|NotebookEdit|Bash"),
            Host::Codex => ("codex", "apply_patch|Bash"),
        };
        format!(
            r#"{{
  "hooks": {{
    "UserPromptSubmit": [
      {{ "hooks": [ {{ "type": "command", "command": "vdt hook {name} UserPromptSubmit" }} ] }}
    ],
    "PostToolUse": [
      {{ "matcher": "{tools}",
        "hooks": [ {{ "type": "command", "command": "vdt hook {name} PostToolUse" }} ] }}
    ],
    "Stop": [
      {{ "hooks": [ {{ "type": "command", "command": "vdt hook {name} Stop", "timeout": 20 }} ] }}
    ]
  }}
}}"#
        )
    }
}
