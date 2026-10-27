//! The hook registration the page offers to install, for each host.
//!
//! The block shown and the file the button writes are the adapters' own `hook_block`, so the page
//! cannot drift from `vdt hooks install`.

/// A host whose hooks the page can offer to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Claude Code.
    ClaudeCode,
    /// Codex.
    Codex,
    /// Cursor.
    Cursor,
    /// Google Antigravity.
    Antigravity,
}

impl Host {
    /// Every host, in page order.
    pub const ALL: [Host; 4] = [
        Host::ClaudeCode,
        Host::Codex,
        Host::Cursor,
        Host::Antigravity,
    ];

    /// The stable key stored as the decision's choice.
    pub fn key(self) -> &'static str {
        match self {
            Host::ClaudeCode => "claude_code",
            Host::Codex => "codex",
            Host::Cursor => "cursor",
            Host::Antigravity => "antigravity",
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
            Host::Cursor => "Cursor",
            Host::Antigravity => "Antigravity",
        }
    }

    /// Where the block goes.
    pub fn target_file(self) -> &'static str {
        match self {
            Host::ClaudeCode => ".claude/settings.json in this project",
            Host::Codex => "hooks.json in your Codex home folder (normally ~/.codex/hooks.json)",
            Host::Cursor => ".cursor/hooks.json in this project",
            Host::Antigravity => ".agents/hooks.json in this project",
        }
    }

    /// The same host, as the adapter crate names it.
    pub fn adapter_host(self) -> verdict_adapters::payload::Host {
        match self {
            Host::ClaudeCode => verdict_adapters::payload::Host::Claude,
            Host::Codex => verdict_adapters::payload::Host::Codex,
            Host::Cursor => verdict_adapters::payload::Host::Cursor,
            Host::Antigravity => verdict_adapters::payload::Host::Antigravity,
        }
    }

    /// The exact registration, in this host's own shape.
    pub fn block(self) -> String {
        let value = verdict_adapters::install::hook_block("vdt", self.adapter_host());
        serde_json::to_string_pretty(&value).unwrap_or_default()
    }
}
