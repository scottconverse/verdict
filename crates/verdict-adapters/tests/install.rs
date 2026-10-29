mod common;

use common::*;
use serde_json::{Value, json};
use std::path::Path;
use verdict_adapters::{HooksRequest, Host, hooks_command};
use verdict_core::{DecisionKind, Record, Writer};

fn req(
    host: Host,
    project: &Path,
    codex_home: Option<&Path>,
    human: bool,
    install: bool,
) -> HooksRequest {
    HooksRequest {
        install,
        host,
        project: Some(project.to_path_buf()),
        codex_home: codex_home.map(Path::to_path_buf),
        i_am_the_human: human,
        vdt_path: None,
    }
}

fn run(r: &HooksRequest, cwd: &Path) -> String {
    let mut out = Vec::new();
    let code = hooks_command(r, cwd, &mut out).unwrap();
    assert_eq!(code, 0);
    String::from_utf8(out).unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn backups(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".bak-vdt-"))
        .collect()
}

fn existing_settings() -> Value {
    json!({
        "model": "keep-me",
        "permissions": { "allow": ["Bash(cargo test)"] },
        "hooks": {
            "PostToolUse": [
                { "matcher": "Edit", "hooks": [ { "type": "command", "command": "node other.mjs", "timeout": 5 } ] }
            ],
            "Stop": [
                { "hooks": [ { "type": "command", "command": "node stop-other.mjs" } ] }
            ],
            "Notification": [
                { "hooks": [ { "type": "command", "command": "notify" } ] }
            ]
        }
    })
}

#[test]
fn a_dsh_install_is_refused_with_the_host_managed_pointer() {
    let p = Project::new();
    let r = req(Host::Dsh, p.dir.path(), None, true, true);
    let mut out = Vec::new();
    let err = hooks_command(&r, p.dir.path(), &mut out).unwrap_err();
    assert!(err.contains("docs/DSH.md"), "{err}");
    assert!(!p.dir.path().join(".dsh").exists());
}

#[test]
fn cursor_install_writes_its_flat_schema_and_uninstall_restores_the_file() {
    let p = Project::new();
    let file = p.dir.path().join(".cursor/hooks.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = json!({
        "version": 1,
        "hooks": {
            "afterFileEdit": [ { "command": "prettier --write $FILE", "timeout": 5 } ]
        }
    });
    std::fs::write(&file, serde_json::to_string_pretty(&original).unwrap()).unwrap();

    let text = run(
        &req(Host::Cursor, p.dir.path(), None, true, true),
        p.dir.path(),
    );
    assert!(text.contains("installed 5 cursor hooks"), "{text}");
    let v = read(&file);
    assert_eq!(v["version"], 1);
    let hooks = v["hooks"].as_object().unwrap();
    assert!(hooks["afterFileEdit"][0]["command"] == "prettier --write $FILE");
    for ev in [
        "sessionStart",
        "beforeSubmitPrompt",
        "preToolUse",
        "postToolUse",
        "stop",
    ] {
        let list = hooks[ev]
            .as_array()
            .unwrap_or_else(|| panic!("{ev} missing: {v}"));
        assert_eq!(list.len(), 1, "{ev}: {v}");
        assert_eq!(list[0]["command"], format!("vdt hook cursor {ev}"));
        assert!(list[0]["timeout"].is_number());
    }
    // Idempotent, one backup.
    let text = run(
        &req(Host::Cursor, p.dir.path(), None, true, true),
        p.dir.path(),
    );
    assert!(text.contains("already installed"), "{text}");
    assert_eq!(backups(file.parent().unwrap()).len(), 1);
    // Uninstall removes only ours and puts the original back.
    run(
        &req(Host::Cursor, p.dir.path(), None, true, false),
        p.dir.path(),
    );
    assert_eq!(read(&file), original);
}

#[test]
fn antigravity_install_writes_its_named_group_and_uninstall_restores_the_file() {
    let p = Project::new();
    let file = p.dir.path().join(".agents/hooks.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = json!({
        "my-linter": {
            "PostToolUse": [
                { "matcher": "run_command", "hooks": [ { "type": "command", "command": "./lint.sh" } ] }
            ]
        }
    });
    std::fs::write(&file, serde_json::to_string_pretty(&original).unwrap()).unwrap();

    let text = run(
        &req(Host::Antigravity, p.dir.path(), None, true, true),
        p.dir.path(),
    );
    assert!(text.contains("installed 5 antigravity hooks"), "{text}");
    let v = read(&file);
    assert!(v["my-linter"].is_object(), "another tool's hook is kept");
    let group = v["verdict"].as_object().unwrap();
    for ev in [
        "SessionStart",
        "PreInvocation",
        "PreToolUse",
        "PostToolUse",
        "Stop",
    ] {
        let list = group[ev]
            .as_array()
            .unwrap_or_else(|| panic!("{ev} missing: {v}"));
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0]["hooks"][0]["command"],
            format!("vdt hook antigravity {ev}")
        );
    }
    let text = run(
        &req(Host::Antigravity, p.dir.path(), None, true, true),
        p.dir.path(),
    );
    assert!(text.contains("already installed"), "{text}");
    assert_eq!(backups(file.parent().unwrap()).len(), 1);
    run(
        &req(Host::Antigravity, p.dir.path(), None, true, false),
        p.dir.path(),
    );
    assert_eq!(read(&file), original);
}

#[test]
fn without_the_human_flag_it_prints_the_exact_block_and_writes_nothing() {
    let proj = TempDir::new("inst");
    let text = run(
        &req(Host::Claude, proj.path(), None, false, true),
        proj.path(),
    );
    assert!(text.contains("Nothing was changed"), "{text}");
    assert!(text.contains("vdt hook claude PostToolUse"), "{text}");
    assert!(text.contains("--i-am-the-human"), "{text}");
    assert!(
        !proj.path().join(".claude").exists(),
        "no settings file was created"
    );
    assert!(
        !proj.path().join(".verdict").exists(),
        "no ledger and no decision was written"
    );
}

#[test]
fn claude_install_registers_all_five_events_in_project_settings() {
    let proj = TempDir::new("inst");
    let text = run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    assert!(text.contains("installed 5 claude hooks"), "{text}");
    let doc = read(&proj.path().join(".claude").join("settings.json"));
    let hooks = doc["hooks"].as_object().unwrap();
    assert_eq!(
        hooks.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop"
        ]
    );
    for (event, groups) in hooks {
        let h = &groups[0]["hooks"][0];
        assert_eq!(h["type"], "command");
        assert_eq!(h["command"], format!("vdt hook claude {event}"));
        // Plain words only: no quotes, no backslashes, so PowerShell, cmd and bash read it alike.
        let cmd = h["command"].as_str().unwrap();
        assert!(!cmd.contains(['"', '\'', '\\', '$', '&', '|']), "{cmd}");
        assert_eq!(h["timeout"], if event == "Stop" { 20 } else { 10 });
    }
}

#[test]
fn install_keeps_other_hooks_and_keys_is_idempotent_and_backs_up_once() {
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let original = serde_json::to_string_pretty(&existing_settings()).unwrap();
    std::fs::write(&file, &original).unwrap();

    let r = req(Host::Claude, proj.path(), None, true, true);
    run(&r, proj.path());
    let after_first = std::fs::read_to_string(&file).unwrap();
    let doc: Value = serde_json::from_str(&after_first).unwrap();
    assert_eq!(doc["model"], "keep-me");
    assert_eq!(doc["permissions"]["allow"][0], "Bash(cargo test)");
    assert_eq!(
        doc["hooks"]["Notification"][0]["hooks"][0]["command"],
        "notify"
    );
    // The matcher group for Edit is untouched; ours went in its own plain group.
    assert_eq!(doc["hooks"]["PostToolUse"][0]["matcher"], "Edit");
    assert_eq!(
        doc["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "node other.mjs"
    );
    assert_eq!(
        doc["hooks"]["PostToolUse"][1]["hooks"][0]["command"],
        "vdt hook claude PostToolUse"
    );
    // Stop: ours joined the plain group after the existing hook.
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"][0]["command"],
        "node stop-other.mjs"
    );
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"][1]["command"],
        "vdt hook claude Stop"
    );
    // Key order of the user's file is kept.
    let keys: Vec<_> = doc.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["model", "permissions", "hooks"]);

    let bak = backups(file.parent().unwrap());
    assert_eq!(bak.len(), 1);
    assert_eq!(
        std::fs::read_to_string(file.parent().unwrap().join(&bak[0])).unwrap(),
        original,
        "the backup is the file as it was"
    );

    let text = run(&r, proj.path());
    assert!(text.contains("already installed"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        after_first,
        "second run changes nothing"
    );
    assert_eq!(
        backups(file.parent().unwrap()).len(),
        1,
        "and makes no second backup"
    );
}

#[test]
fn reinstall_with_a_different_program_path_replaces_ours_without_duplicating() {
    let proj = TempDir::new("inst");
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    let mut r = req(Host::Claude, proj.path(), None, true, true);
    r.vdt_path = Some("C:\\tools\\vdt.exe".into());
    run(&r, proj.path());
    let doc = read(&proj.path().join(".claude").join("settings.json"));
    let stop = doc["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["hooks"].as_array().unwrap().len(), 1);
    assert_eq!(
        stop[0]["hooks"][0]["command"],
        r#""C:\tools\vdt.exe" hook claude Stop"#
    );
}

#[test]
fn a_spaced_core_path_is_quoted_and_removed_and_a_metacharacter_path_is_refused() {
    // A path with spaces is written quoted, so it runs from cmd/PowerShell/sh, and uninstalling
    // with the same path removes exactly that entry.
    let proj = TempDir::new("inst");
    let mut r = req(Host::Claude, proj.path(), None, true, true);
    r.vdt_path = Some("C:\\Program Files\\Verdict\\vdt.exe".into());
    run(&r, proj.path());
    let doc = read(&proj.path().join(".claude").join("settings.json"));
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"][0]["command"],
        "\"C:\\Program Files\\Verdict\\vdt.exe\" hook claude Stop"
    );
    r.install = false;
    let out = run(&r, proj.path());
    assert!(out.contains("removed 5"), "{out}");

    // A path the host shells would expand inside quotes is refused, naming the character.
    let mut bad = req(Host::Claude, proj.path(), None, true, true);
    bad.vdt_path = Some("C:\\tmp\\$vdt\\vdt.exe".into());
    let err = hooks_command(&bad, proj.path(), &mut Vec::new()).unwrap_err();
    assert!(err.contains("'$'"), "{err}");
    assert!(err.contains("expand"), "{err}");
}

#[test]
fn uninstall_removes_only_ours_and_leaves_everything_else() {
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&existing_settings()).unwrap(),
    )
    .unwrap();
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    let text = run(
        &req(Host::Claude, proj.path(), None, true, false),
        proj.path(),
    );
    assert!(text.contains("removed 5 claude hooks"), "{text}");
    assert_eq!(
        read(&file),
        existing_settings(),
        "back to exactly what the user had"
    );
    // Without anything of ours left, uninstall is a no-op.
    let text = run(
        &req(Host::Claude, proj.path(), None, true, false),
        proj.path(),
    );
    assert!(text.contains("nothing changed"), "{text}");
}

#[test]
fn uninstall_alone_in_a_file_drops_the_empty_hooks_block() {
    let proj = TempDir::new("inst");
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    run(
        &req(Host::Claude, proj.path(), None, true, false),
        proj.path(),
    );
    let doc = read(&proj.path().join(".claude").join("settings.json"));
    assert_eq!(doc, json!({}));
}

#[test]
fn a_file_that_is_not_json_is_never_overwritten() {
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "{ not json").unwrap();
    let err = hooks_command(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
        &mut Vec::new(),
    )
    .unwrap_err();
    assert!(err.contains("not valid JSON"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
    assert!(backups(file.parent().unwrap()).is_empty());
}

#[test]
fn codex_install_goes_to_codex_home_hooks_json_and_never_touches_config_toml() {
    let proj = TempDir::new("inst");
    let home = TempDir::new("codexhome");
    std::fs::write(home.path().join("config.toml"), "model = \"keep-me\"\n").unwrap();
    std::fs::write(
        home.path().join("hooks.json"),
        serde_json::to_string(&json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"node C:/other/wake.mjs"}]}]}})).unwrap(),
    )
    .unwrap();
    let text = run(
        &req(Host::Codex, proj.path(), Some(home.path()), true, true),
        proj.path(),
    );
    assert!(
        text.contains("/hooks"),
        "tells the human Codex needs a trust review: {text}"
    );
    let doc = read(&home.path().join("hooks.json"));
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"][0]["command"],
        "node C:/other/wake.mjs"
    );
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"][1]["command"],
        "vdt hook codex Stop"
    );
    assert_eq!(
        doc["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "vdt hook codex SessionStart"
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml")).unwrap(),
        "model = \"keep-me\"\n"
    );
    assert!(!proj.path().join(".claude").exists());
    // A Claude entry in the same file would not be removed by a Codex uninstall.
    run(
        &req(Host::Codex, proj.path(), Some(home.path()), true, false),
        proj.path(),
    );
    let doc = read(&home.path().join("hooks.json"));
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn install_records_a_decision_written_by_the_human_and_only_with_the_flag() {
    let proj = TempDir::new("inst");
    run(
        &req(Host::Claude, proj.path(), None, false, true),
        proj.path(),
    );
    assert!(
        verdict_core::Ledger::open_existing(proj.path())
            .unwrap()
            .is_none()
    );

    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    let l = verdict_core::Ledger::open_existing(proj.path())
        .unwrap()
        .unwrap();
    let recs = l.records().unwrap();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].writer, Writer::Human);
    let Record::Decision {
        decision,
        who,
        choice,
        ..
    } = &recs[0].record
    else {
        panic!("{:?}", recs[0].record)
    };
    assert_eq!(*decision, DecisionKind::InstallHooks);
    assert_eq!(*who, Writer::Human);
    assert!(choice.contains("install claude"), "{choice}");

    // Idempotent: nothing changed, so no second decision.
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    assert_eq!(l.records().unwrap().len(), 1);
}

#[test]
fn an_installed_claude_block_is_what_the_hook_command_understands() {
    // The registered command words must be accepted by `hook`: host and event parse.
    let proj = TempDir::new("inst");
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    let doc = read(&proj.path().join(".claude").join("settings.json"));
    for (event, groups) in doc["hooks"].as_object().unwrap() {
        let cmd = groups[0]["hooks"][0]["command"].as_str().unwrap();
        let words: Vec<&str> = cmd.split_whitespace().collect();
        assert_eq!(words[1], "hook");
        assert_eq!(Host::parse(words[2]), Some(Host::Claude));
        assert_eq!(
            verdict_adapters::EventKind::parse(words[3])
                .unwrap()
                .as_str(),
            event
        );
    }
}

#[test]
fn a_hook_with_extra_commands_after_ours_is_the_users_and_is_kept() {
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mine = "vdt hook claude Stop && node policy-check.js";
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": mine } ] } ] }
        }))
        .unwrap(),
    )
    .unwrap();
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    assert!(std::fs::read_to_string(&file).unwrap().contains(mine));
    run(
        &req(Host::Claude, proj.path(), None, true, false),
        proj.path(),
    );
    let doc = read(&file);
    assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], mine);
    assert_eq!(
        doc["hooks"]["Stop"][0]["hooks"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn hooks_installed_with_a_custom_program_name_can_be_uninstalled() {
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    let mut r = req(Host::Claude, proj.path(), None, true, true);
    r.vdt_path = Some(r"C:\tools\verdict-debug.exe".into());
    run(&r, proj.path());
    let out = run(&r, proj.path());
    assert!(
        out.contains("nothing changed"),
        "a second install changes nothing: {out}"
    );
    r.install = false;
    let out = run(&r, proj.path());
    assert!(out.contains("removed 5"), "{out}");
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains("verdict-debug")
    );
}

#[test]
fn no_temporary_file_is_left_beside_the_settings() {
    let proj = TempDir::new("inst");
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    let dir = proj.path().join(".claude");
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("vdt-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn another_programs_hook_survives_install_and_uninstall() {
    // Same four-word shape, different program: never Verdict's, never removed.
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mine = "node policy-check.js hook claude Stop";
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&json!({
            "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": mine } ] } ] }
        }))
        .unwrap(),
    )
    .unwrap();
    run(
        &req(Host::Claude, proj.path(), None, true, true),
        proj.path(),
    );
    let doc = read(&file);
    let commands: Vec<&str> = doc["hooks"]["Stop"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["hooks"].as_array())
        .flatten()
        .filter_map(|h| h["command"].as_str())
        .collect();
    assert!(commands.contains(&mine), "survives install: {commands:?}");
    assert!(
        commands.iter().any(|c| c.starts_with("vdt hook claude")),
        "{commands:?}"
    );
    run(
        &req(Host::Claude, proj.path(), None, true, false),
        proj.path(),
    );
    let doc = read(&file);
    let commands: Vec<&str> = doc["hooks"]["Stop"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["hooks"].as_array())
        .flatten()
        .filter_map(|h| h["command"].as_str())
        .collect();
    assert_eq!(
        commands,
        vec![mine],
        "uninstall removes only Verdict's entry"
    );
}

#[test]
fn uninstall_with_a_new_path_removes_entries_written_with_an_older_path() {
    let proj = TempDir::new("inst");
    let file = proj.path().join(".claude").join("settings.json");
    let mut old = req(Host::Claude, proj.path(), None, true, true);
    old.vdt_path = Some(r"C:\old\vdt.exe".into());
    run(&old, proj.path());
    let mut new = req(Host::Claude, proj.path(), None, true, false);
    new.vdt_path = Some(r"C:\new\vdt.exe".into());
    let out = run(&new, proj.path());
    assert!(out.contains("removed 5"), "{out}");
    assert!(
        !std::fs::read_to_string(&file).unwrap().contains("vdt"),
        "all older-path entries removed"
    );
}

#[test]
fn cursor_and_antigravity_custom_path_entries_are_recognised_and_removed() {
    for host in [Host::Cursor, Host::Antigravity] {
        let proj = TempDir::new("inst");
        let mut r = req(host, proj.path(), None, true, true);
        r.vdt_path = Some(r"C:\Program Files\Verdict\vdt.exe".into());
        run(&r, proj.path());
        r.install = false;
        let out = run(&r, proj.path());
        assert!(out.contains("removed 5"), "{host:?}: {out}");
    }
}

#[test]
fn a_verbatim_windows_path_is_written_without_the_prefix() {
    // cmd cannot run a \\?\ path even quoted; the hook command carries the plain absolute path.
    let got = verdict_adapters::install::vdt_program(Some(r"\\?\C:\Verdict\bin\vdt.exe")).unwrap();
    assert_eq!(got, r#""C:\Verdict\bin\vdt.exe""#);
    let unc =
        verdict_adapters::install::vdt_program(Some(r"\\?\UNC\server\share\vdt.exe")).unwrap();
    assert_eq!(unc, r#""\\server\share\vdt.exe""#);
}
