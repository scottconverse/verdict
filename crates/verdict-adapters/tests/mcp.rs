//! `vdt mcp-install`: the server registration files, merged and human-gated.

mod common;

use common::*;
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use verdict_adapters::{McpHost, McpRequest, mcp_command};

fn req(host: McpHost, project: &Path, codex_home: Option<&Path>, human: bool) -> McpRequest {
    McpRequest {
        host,
        project: Some(project.to_path_buf()),
        vdt_path: Some("vdt".into()),
        codex_home: codex_home.map(Path::to_path_buf),
        i_am_the_human: human,
    }
}

fn run(r: &McpRequest, cwd: &Path) -> String {
    let mut out = Vec::new();
    let code = mcp_command(r, cwd, &mut out).unwrap();
    assert_eq!(code, 0);
    String::from_utf8(out).unwrap()
}

fn backups(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".bak-vdt-"))
        .collect()
}

#[test]
fn without_the_human_flag_it_prints_the_entry_and_writes_nothing() {
    let p = Project::new();
    let text = run(
        &req(McpHost::Claude, p.dir.path(), None, false),
        p.dir.path(),
    );
    assert!(text.contains("Nothing was changed"), "{text}");
    assert!(text.contains("mcpServers"), "{text}");
    assert!(!p.dir.path().join(".mcp.json").exists());
    assert!(!p.dir.path().join(".verdict").exists());
}

#[test]
fn claude_registration_merges_keeps_other_servers_backs_up_and_records_the_decision() {
    let p = Project::new();
    let file = p.dir.path().join(".mcp.json");
    std::fs::write(
        &file,
        r#"{"mcpServers":{"other":{"command":"other.exe","args":[]}}}"#,
    )
    .unwrap();
    let text = run(
        &req(McpHost::Claude, p.dir.path(), None, true),
        p.dir.path(),
    );
    assert!(text.contains("registered the Verdict MCP server"), "{text}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["verdict"]["command"], "vdt");
    assert_eq!(v["mcpServers"]["verdict"]["args"][0], "mcp");
    assert_eq!(v["mcpServers"]["other"]["command"], "other.exe");
    assert_eq!(backups(p.dir.path()).len(), 1);
    let recs = p.records();
    assert!(recs.iter().any(|s| matches!(
        &s.record,
        verdict_core::Record::Decision {
            decision: verdict_core::DecisionKind::McpInstall,
            choice,
            who: verdict_core::Writer::Human,
            ..
        } if choice == "claude_code"
    )));
    // Idempotent: no second backup, no second decision.
    let text = run(
        &req(McpHost::Claude, p.dir.path(), None, true),
        p.dir.path(),
    );
    assert!(text.contains("already registered"), "{text}");
    assert_eq!(backups(p.dir.path()).len(), 1);
    assert_eq!(
        p.records()
            .iter()
            .filter(|s| matches!(
                s.record,
                verdict_core::Record::Decision {
                    decision: verdict_core::DecisionKind::McpInstall,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[test]
fn cursor_registration_goes_to_its_project_file() {
    let p = Project::new();
    run(
        &req(McpHost::Cursor, p.dir.path(), None, true),
        p.dir.path(),
    );
    let file = p.dir.path().join(".cursor/mcp.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(v["mcpServers"]["verdict"]["args"][0], "mcp");
}

#[test]
fn dsh_prints_the_entry_and_never_edits_the_harness_configuration() {
    let p = Project::new();
    let text = run(&req(McpHost::Dsh, p.dir.path(), None, true), p.dir.path());
    assert!(text.contains("dsh-mcp-client"), "{text}");
    assert!(text.contains("serverName: verdict"), "{text}");
    assert!(!p.dir.path().join("cordis.patch.yml").exists());
}

#[test]
fn codex_registration_uses_the_codex_cli_when_it_is_installed() {
    let installed = Command::new("codex")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !installed {
        println!("codex is not installed; the Codex registration check is skipped");
        return;
    }
    let p = Project::new();
    let home = p.home.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let text = run(
        &req(McpHost::Codex, p.dir.path(), Some(&home), true),
        p.dir.path(),
    );
    assert!(
        text.contains("registered the Verdict MCP server for Codex"),
        "{text}"
    );
    let list = Command::new("codex")
        .args(["mcp", "list"])
        .env("CODEX_HOME", &home)
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&list.stdout).to_string();
    assert!(listed.contains("verdict"), "codex mcp list: {listed}");
}

#[test]
fn an_unknown_mcp_file_is_never_overwritten() {
    let p = Project::new();
    let file = p.dir.path().join(".mcp.json");
    std::fs::write(&file, "not json").unwrap();
    let mut out = Vec::new();
    let err = mcp_command(
        &req(McpHost::Claude, p.dir.path(), None, true),
        p.dir.path(),
        &mut out,
    )
    .unwrap_err();
    assert!(err.contains("not valid JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
}
