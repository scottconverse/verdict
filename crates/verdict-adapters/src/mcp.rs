//! `vdt mcp-install <host>`: register the Verdict MCP server with a host.
//!
//! The server itself is `vdt mcp` on stdio. Registering it is a human action, so without
//! `--i-am-the-human` this command prints exactly what it would write and changes nothing.
//!
//! * Claude Code: `<project>/.mcp.json`, `mcpServers.verdict`, merged with any other servers.
//! * Cursor: `<project>/.cursor/mcp.json`, the same shape.
//! * Codex: delegated to `codex mcp add verdict -- vdt mcp --project <root>`, so Codex writes its
//!   own config in the format that version understands (`CODEX_HOME` is honored).
//! * DeepSeek Harness: prints the exact `cordis.patch.yml` entry; the harness keeps its own
//!   configuration, so this command never edits it (`docs/DSH.md`).
//! * Antigravity: prints where to register it; its MCP configuration format is not verified here.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value, json};
use verdict_core::{DecisionKind, Ledger, Record, Writer, find_project_root};

use crate::install::{backup, vdt_program, write_json};

/// A host that can run the Verdict MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpHost {
    /// Claude Code.
    Claude,
    /// Cursor.
    Cursor,
    /// OpenAI Codex.
    Codex,
    /// DeepSeek Harness.
    Dsh,
    /// Google Antigravity.
    Antigravity,
}

impl McpHost {
    /// Parse the command-line name.
    pub fn parse(s: &str) -> Option<McpHost> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(McpHost::Claude),
            "cursor" => Some(McpHost::Cursor),
            "codex" => Some(McpHost::Codex),
            "dsh" | "deepseek-harness" => Some(McpHost::Dsh),
            "antigravity" => Some(McpHost::Antigravity),
            _ => None,
        }
    }

    /// Stable name used in the decision's choice.
    pub fn key(self) -> &'static str {
        match self {
            McpHost::Claude => "claude_code",
            McpHost::Cursor => "cursor",
            McpHost::Codex => "codex",
            McpHost::Dsh => "dsh",
            McpHost::Antigravity => "antigravity",
        }
    }

    /// The name shown to a person.
    pub fn title(self) -> &'static str {
        match self {
            McpHost::Claude => "Claude Code",
            McpHost::Cursor => "Cursor",
            McpHost::Codex => "Codex",
            McpHost::Dsh => "DeepSeek Harness",
            McpHost::Antigravity => "Antigravity",
        }
    }
}

/// What `vdt mcp-install` was asked for.
#[derive(Debug, Clone)]
pub struct McpRequest {
    /// Which host.
    pub host: McpHost,
    /// The project folder (Claude and Cursor write a project-level file).
    pub project: Option<PathBuf>,
    /// The path to `vdt` to register (default: plain `vdt` on PATH).
    pub vdt_path: Option<String>,
    /// The Codex home folder (`None`: `$CODEX_HOME`, else `~/.codex`).
    pub codex_home: Option<PathBuf>,
    /// The human typed `--i-am-the-human`.
    pub i_am_the_human: bool,
}

fn server_entry(vdt: &str, root: &Path) -> Value {
    json!({
        "command": vdt,
        "args": ["mcp", "--project", root.to_string_lossy().replace('\\', "/")],
    })
}

/// The exact `mcpServers` document the project-level hosts read.
pub fn mcp_block(vdt: &str, root: &Path) -> Value {
    json!({ "mcpServers": { "verdict": server_entry(vdt, root) } })
}

fn target_file(host: McpHost, root: &Path) -> Option<PathBuf> {
    match host {
        McpHost::Claude => Some(root.join(".mcp.json")),
        McpHost::Cursor => Some(root.join(".cursor").join("mcp.json")),
        _ => None,
    }
}

fn merge_server(doc: &mut Value, vdt: &str, root: &Path) -> Result<bool, String> {
    let obj = doc
        .as_object_mut()
        .ok_or("the MCP file is JSON but not an object")?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("the \"mcpServers\" key is not an object")?;
    let want = server_entry(vdt, root);
    if servers.get("verdict") == Some(&want) {
        return Ok(false);
    }
    servers.insert("verdict".into(), want);
    Ok(true)
}

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(json!({})),
        Ok(s) => serde_json::from_str(&s).map_err(|e| {
            format!(
                "{} is not valid JSON, so it was left alone: {e}",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn record_decision(root: &Path, host: McpHost) -> Result<u64, String> {
    let mut ledger = Ledger::open(root).map_err(|e| e.to_string())?;
    ledger
        .append(
            Writer::Human,
            None,
            Record::Decision {
                decision: DecisionKind::McpInstall,
                choice: host.key().into(),
                reason: None,
                who: Writer::Human,
            },
        )
        .map(|a| a.seq)
        .map_err(|e| e.to_string())
}

/// Run `vdt mcp-install`. Returns the exit code (0 fine, 1 error).
pub fn mcp_command(req: &McpRequest, cwd: &Path, out: &mut dyn Write) -> Result<u8, String> {
    let io = |e: std::io::Error| e.to_string();
    let root = find_project_root(req.project.as_deref().unwrap_or(cwd));
    let vdt = vdt_program(req.vdt_path.as_deref())?;
    if let Some(path) = target_file(req.host, &root) {
        let block =
            serde_json::to_string_pretty(&mcp_block(&vdt, &root)).map_err(|e| e.to_string())?;
        if !req.i_am_the_human {
            writeln!(
                out,
                "Nothing was changed. Registering the MCP server is a decision only you make, so vdt will not do it unless you add --i-am-the-human."
            )
            .map_err(io)?;
            writeln!(out, "File that would change: {}", path.display()).map_err(io)?;
            writeln!(
                out,
                "This entry would be merged in (your other servers stay):\n{block}"
            )
            .map_err(io)?;
            return Ok(0);
        }
        let mut doc = read_json(&path)?;
        let changed = merge_server(&mut doc, &vdt, &root)?;
        let backup = if changed && path.exists() {
            Some(backup(&path)?)
        } else {
            None
        };
        if changed {
            write_json(&path, &doc)?;
        }
        if changed {
            let seq = record_decision(&root, req.host)?;
            writeln!(
                out,
                "registered the Verdict MCP server for {} in {}",
                req.host.title(),
                path.display()
            )
            .map_err(io)?;
            if let Some(b) = backup {
                writeln!(out, "backup of the old file: {}", b.display()).map_err(io)?;
            }
            writeln!(out, "decision recorded as ledger record {seq}").map_err(io)?;
        } else {
            writeln!(out, "the Verdict MCP server is already registered for {} in {}; nothing changed, no decision recorded", req.host.title(), path.display())
                .map_err(io)?;
        }
        return Ok(0);
    }
    match req.host {
        McpHost::Codex => {
            let mut cmd = Command::new("codex");
            cmd.args(["mcp", "add", "verdict", "--"])
                .arg(&vdt)
                .args(["mcp", "--project"])
                .arg(&root);
            if let Some(home) = &req.codex_home {
                cmd.env("CODEX_HOME", home);
            }
            if !req.i_am_the_human {
                writeln!(
                    out,
                    "Nothing was changed. This would run:\n  codex mcp add verdict -- {vdt} mcp --project {}",
                    root.display()
                )
                .map_err(io)?;
                return Ok(0);
            }
            let result = cmd.output().map_err(|e| {
                format!("cannot run `codex mcp add` (is the Codex CLI installed?): {e}")
            })?;
            if !result.status.success() {
                let err = String::from_utf8_lossy(&result.stderr);
                return Err(format!(
                    "`codex mcp add` failed: {}",
                    err.lines().next().unwrap_or("no message")
                ));
            }
            let seq = record_decision(&root, req.host)?;
            writeln!(out, "registered the Verdict MCP server for Codex").map_err(io)?;
            writeln!(out, "decision recorded as ledger record {seq}").map_err(io)?;
            Ok(0)
        }
        McpHost::Dsh => {
            writeln!(
                out,
                "DeepSeek Harness keeps its own configuration, so vdt does not edit it. Add this entry to cordis.patch.yml under `insert:` (adjust the path to vdt):\n\n\
                 - id: mcp-verdict\n  name: '@deepseek-ai/dsh-mcp-client'\n  config:\n    serverName: verdict\n    transport: stdio\n    command: {vdt}\n    args:\n    - mcp\n    - --project\n    - {root}\n    failOnStartupError: false\n\nThe optional native adapter is in integrations/dsh (docs/DSH.md).",
                root = root.to_string_lossy().replace('\\', "/")
            )
            .map_err(io)?;
            Ok(0)
        }
        McpHost::Antigravity => {
            writeln!(
                out,
                "Antigravity's MCP configuration format is not verified for this build. Register `{vdt} mcp --project {}` with it and see docs/HOSTS.md.",
                root.display()
            )
            .map_err(io)?;
            Ok(0)
        }
        McpHost::Claude | McpHost::Cursor => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_names_the_server_the_binary_and_the_project() {
        let block = mcp_block("vdt", Path::new("C:/work/app"));
        assert_eq!(block["mcpServers"]["verdict"]["command"], "vdt");
        assert_eq!(block["mcpServers"]["verdict"]["args"][0], "mcp");
        assert_eq!(block["mcpServers"]["verdict"]["args"][2], "C:/work/app");
    }

    #[test]
    fn merging_keeps_other_servers_and_is_idempotent() {
        let mut doc = json!({"mcpServers": {"other": {"command": "x"}}});
        assert!(merge_server(&mut doc, "vdt", Path::new("/p")).unwrap());
        assert!(doc["mcpServers"]["other"].is_object());
        assert!(!merge_server(&mut doc, "vdt", Path::new("/p")).unwrap());
    }
}
