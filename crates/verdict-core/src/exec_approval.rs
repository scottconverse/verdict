//! What the human approves when they approve a repo's commands, and how a later run checks that
//! the commands are still those.
//!
//! A `run_project_commands` decision carries a hash of the effective exec configuration: the
//! commands, their tiers, what each needs, the shell each runs in, and the timeout. The page and
//! `vdt` write it with [`approval_choice`]; the exec checker compares it with [`commands_hash`]
//! computed from the policy it is about to run. Change a command and the hash changes, so the old
//! approval no longer covers it. An approval with no hash (an older ledger) does not count.

use std::time::Duration;

use serde_json::Value;

use crate::model::Tier;
use crate::policy::Policy;

/// Browser evidence for a command registered at the `browsed` tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserProof {
    /// A screenshot the command wrote (path relative to the project).
    pub screenshot: String,
    /// The page that was loaded, when the project names it.
    pub url: Option<String>,
}

/// One command from the policy's `exec.commands`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecCommand {
    /// The command line.
    pub run: String,
    /// The tier it earns.
    pub tier: Tier,
    /// What it needs, for example `server`.
    pub needs: Option<String>,
    /// The browser evidence a `browsed` command must produce.
    pub browser: Option<BrowserProof>,
}

/// Read the policy's `exec.commands`.
pub fn policy_commands(policy: &Policy) -> Vec<ExecCommand> {
    policy
        .value
        .pointer("/exec/commands")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let run = c.get("run")?.as_str()?.trim().to_string();
                    if run.is_empty() {
                        return None;
                    }
                    let tier = match c.get("tier").and_then(Value::as_str).unwrap_or("ran") {
                        "static" => Tier::Static,
                        "browsed" => Tier::Browsed,
                        "asset" => Tier::Asset,
                        _ => Tier::Ran,
                    };
                    Some(ExecCommand {
                        run,
                        tier,
                        needs: c.get("needs").and_then(Value::as_str).map(str::to_string),
                        browser: c.get("browser").and_then(|b| {
                            let screenshot = b.get("screenshot")?.as_str()?.trim();
                            if screenshot.is_empty() {
                                return None;
                            }
                            Some(BrowserProof {
                                screenshot: screenshot.to_string(),
                                url: b.get("url").and_then(Value::as_str).map(str::to_string),
                            })
                        }),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `3m`, `20s`, `500ms`, `1h`.
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let n: u64 = s[..split].parse().ok()?;
    match &s[split..] {
        "ms" => Some(Duration::from_millis(n)),
        "s" => Some(Duration::from_secs(n)),
        "m" => Some(Duration::from_secs(n.checked_mul(60)?)),
        "h" => Some(Duration::from_secs(n.checked_mul(3600)?)),
        _ => None,
    }
}

/// The policy's `exec.timeout`, as written (`None` when the policy sets none).
pub fn policy_timeout_text(policy: &Policy) -> Option<String> {
    policy
        .value
        .pointer("/exec/timeout")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// True when the command line is a PowerShell script (`.ps1`), which runs in `pwsh` or
/// `powershell` rather than the platform shell.
pub fn is_powershell_script(cmd: &str) -> bool {
    let t = cmd.trim_start();
    let first = if let Some(rest) = t.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        t.split_whitespace().next().unwrap_or("")
    };
    first.to_ascii_lowercase().ends_with(".ps1")
}

/// The shell a command runs in when the project does not declare one.
pub fn shell_for(cmd: &str) -> &'static str {
    if is_powershell_script(cmd) {
        "pwsh-or-powershell -File"
    } else if cfg!(windows) {
        "cmd /D /S /C"
    } else {
        "sh -c"
    }
}

/// The shell a project declares for its approved commands (`exec.shell`). The design requires the
/// declared **native** shell; a project that names one gets exactly that, and a named shell that is
/// missing is an honest failure, never a silent fallback to a different one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shell {
    /// The platform's native shell (`cmd` on Windows, `sh` elsewhere; `.ps1` through PowerShell).
    #[default]
    Native,
    /// `cmd /D /S /C`.
    Cmd,
    /// `pwsh -NoProfile -Command`.
    Pwsh,
    /// `powershell -NoProfile -Command`.
    Powershell,
    /// `sh -c`.
    Sh,
    /// `bash -c`.
    Bash,
    /// `zsh -c`.
    Zsh,
}

impl Shell {
    /// Stable name, as the policy writes it.
    pub fn name(self) -> &'static str {
        match self {
            Shell::Native => "native",
            Shell::Cmd => "cmd",
            Shell::Pwsh => "pwsh",
            Shell::Powershell => "powershell",
            Shell::Sh => "sh",
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
        }
    }

    /// Parse a declared shell name.
    pub fn parse(s: &str) -> Option<Shell> {
        match s.trim().to_ascii_lowercase().as_str() {
            "native" | "" => Some(Shell::Native),
            "cmd" | "cmd.exe" => Some(Shell::Cmd),
            "pwsh" => Some(Shell::Pwsh),
            "powershell" | "powershell.exe" => Some(Shell::Powershell),
            "sh" => Some(Shell::Sh),
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            _ => None,
        }
    }
}

/// The shell the policy declares, or why the declaration cannot be used. A project that declares a
/// shell this build does not know must not silently run its commands somewhere else.
pub fn policy_shell(policy: &Policy) -> Result<Shell, String> {
    match policy.value.pointer("/exec/shell") {
        None | Some(Value::Null) => Ok(Shell::Native),
        Some(Value::String(s)) => Shell::parse(s)
            .ok_or_else(|| format!("exec.shell is {s:?}, which is not a shell this build runs")),
        Some(other) => Err(format!("exec.shell must be a string, not {other}")),
    }
}

/// The program and leading arguments that run `cmd` in `shell` (before `cmd` itself is added).
/// The first element is the program name. Used by the exec runner and by the first-run server
/// launcher, so both speak the same shell.
pub fn shell_invocation(cmd: &str, shell: Shell) -> (String, Vec<String>) {
    match shell {
        Shell::Native => {
            if is_powershell_script(cmd) {
                let mut args = vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-File".to_string(),
                ];
                // A script path followed by arguments: split on whitespace, keep quotes simple.
                args.extend(cmd.split_whitespace().map(str::to_string));
                return ("pwsh".to_string(), args);
            }
            if cfg!(windows) {
                (
                    "cmd".to_string(),
                    vec!["/D".into(), "/S".into(), "/C".into(), format!("\"{cmd}\"")],
                )
            } else {
                ("sh".to_string(), vec!["-c".into(), cmd.to_string()])
            }
        }
        Shell::Cmd => (
            "cmd".to_string(),
            vec!["/D".into(), "/S".into(), "/C".into(), format!("\"{cmd}\"")],
        ),
        Shell::Pwsh => (
            "pwsh".to_string(),
            vec!["-NoProfile".into(), "-Command".into(), cmd.to_string()],
        ),
        Shell::Powershell => (
            "powershell".to_string(),
            vec!["-NoProfile".into(), "-Command".into(), cmd.to_string()],
        ),
        Shell::Sh => ("sh".to_string(), vec!["-c".into(), cmd.to_string()]),
        Shell::Bash => ("bash".to_string(), vec!["-c".into(), cmd.to_string()]),
        Shell::Zsh => ("zsh".to_string(), vec!["-c".into(), cmd.to_string()]),
    }
}

/// Commands the other configured tools would run: the mutation runner, the first-run workflow and
/// the release checks. They are covered by the same human approval as `exec.commands`, so changing
/// any of them asks the human again.
pub fn tool_commands(policy: &Policy) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(c) = policy
        .value
        .pointer("/mutation/command")
        .and_then(Value::as_str)
    {
        let c = c.trim();
        if !c.is_empty() {
            out.push(("mutation", c.to_string()));
        }
    }
    for (kind, pointer) in [
        ("firstrun-server", "/firstrun/server/command"),
        ("firstrun-browser", "/firstrun/browser/command"),
    ] {
        if let Some(c) = policy.value.pointer(pointer).and_then(Value::as_str) {
            let c = c.trim();
            if !c.is_empty() {
                out.push((kind, c.to_string()));
            }
        }
    }
    for key in ["/firstrun/commands", "/release/checks"] {
        if let Some(list) = policy.value.pointer(key).and_then(Value::as_array) {
            for item in list {
                let cmd = match item {
                    Value::String(s) => Some(s.trim()),
                    Value::Object(o) => o.get("run").and_then(Value::as_str).map(str::trim),
                    _ => None,
                };
                if let Some(c) = cmd.filter(|c| !c.is_empty()) {
                    let kind = if key.contains("firstrun") {
                        "firstrun"
                    } else {
                        "release"
                    };
                    out.push((kind, c.to_string()));
                }
            }
        }
    }
    out
}

/// One desk provider from the policy's `vendors.providers`, as the approval sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskProgram {
    /// The provider's name.
    pub name: String,
    /// Its vendor.
    pub vendor: String,
    /// The model it asks for.
    pub model: String,
    /// The program it starts.
    pub command: String,
    /// The program's arguments.
    pub args: Vec<String>,
    /// Its own time limit.
    pub timeout_secs: Option<u64>,
}

/// The desk provider programs the policy configures. A desk provider starts a program on this
/// machine, so it is a project command like any other: the same human approval covers it, and a
/// repo's `verdict.json` cannot approve its own providers (design.md: detection "may propose them
/// but cannot approve them"). Entries that cannot be read are kept as text so they still change
/// the approval.
pub fn desk_programs(policy: &Policy) -> Result<Vec<DeskProgram>, String> {
    let Some(list) = policy.value.pointer("/vendors/providers") else {
        return Ok(Vec::new());
    };
    let Some(list) = list.as_array() else {
        return Err(format!("vendors.providers is not a list: {list}"));
    };
    Ok(list
        .iter()
        .map(|item| {
            let s = |k: &str| {
                item.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            DeskProgram {
                name: s("name"),
                vendor: s("vendor"),
                model: item
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("unspecified")
                    .to_string(),
                command: s("command"),
                args: item
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                timeout_secs: item.get("timeout_secs").and_then(Value::as_u64),
            }
        })
        .collect())
}

/// True when the human's latest `run_project_commands` decision approves exactly the
/// configuration `policy` describes now (commands, tools and desk providers). Decisions are
/// project-wide; only records the human wrote count.
pub fn approved_now(ledger: &crate::Ledger, policy: &Policy) -> Result<bool, String> {
    let mut latest: Option<ApprovalChoice> = None;
    for s in ledger.records().map_err(|e| e.to_string())? {
        if let crate::Record::Decision {
            decision: crate::DecisionKind::RunProjectCommands,
            choice,
            who: crate::Writer::Human,
            ..
        } = &s.record
        {
            latest = Some(parse_choice(choice));
        }
    }
    Ok(matches!(latest, Some(ApprovalChoice::Approved(h)) if h == commands_hash(policy)))
}

/// One line per desk provider for [`config_text`], with every field that changes what runs.
fn push_desk_lines(policy: &Policy, out: &mut String) {
    let clean = |s: &str| s.replace(['\n', '\r', '\t'], " ");
    match desk_programs(policy) {
        Ok(list) => {
            for d in list {
                out.push_str(&format!(
                    "desk\tname={}\tvendor={}\tmodel={}\tcommand={}\targs={}\ttimeout={}\n",
                    clean(&d.name),
                    clean(&d.vendor),
                    clean(&d.model),
                    clean(&d.command),
                    serde_json::to_string(&d.args).unwrap_or_default(),
                    d.timeout_secs
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "-".into())
                ));
            }
        }
        Err(e) => out.push_str(&format!("desk\tunreadable\t{}\n", clean(&e))),
    }
}

/// The effective configuration in a fixed text form: one line per command, then the timeout.
/// This is what gets hashed, and it is also a readable summary.
pub fn config_text(policy: &Policy) -> String {
    let mut out = String::from("verdict-exec-approval-v1\n");
    // The declared shell changes what a command means, so it is part of what the human approves.
    out.push_str(&format!(
        "shell\t{}\n",
        policy_shell(policy)
            .map(|s| s.name().to_string())
            .unwrap_or_else(|_| "unusable".to_string())
    ));
    for c in policy_commands(policy) {
        out.push_str(&format!(
            "command\t{}\ttier={}\tneeds={}\tshell={}\n",
            c.run.replace(['\n', '\r', '\t'], " "),
            c.tier.as_str(),
            c.needs.as_deref().unwrap_or("-"),
            shell_for(&c.run)
        ));
    }
    for (kind, command) in tool_commands(policy) {
        out.push_str(&format!(
            "tool\t{kind}\t{}\tshell={}\n",
            command.replace(['\n', '\r', '\t'], " "),
            shell_for(&command)
        ));
    }
    push_desk_lines(policy, &mut out);
    let timeout = match policy_timeout_text(policy) {
        None => "default".to_string(),
        Some(t) => match parse_duration(&t) {
            Some(d) => format!("{}ms", d.as_millis()),
            None => format!("unreadable:{}", t.trim()),
        },
    };
    out.push_str(&format!("timeout\t{timeout}\n"));
    out
}

/// Hash of the effective exec configuration (blake3, hex).
pub fn commands_hash(policy: &Policy) -> String {
    blake3::hash(config_text(policy).as_bytes())
        .to_hex()
        .to_string()
}

/// The `choice` text of an approval of exactly these commands.
pub fn approval_choice(policy: &Policy) -> String {
    format!("approved:{}", commands_hash(policy))
}

/// What a recorded `run_project_commands` choice says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalChoice {
    /// The human said no.
    Refused,
    /// An approval that names the configuration it covers.
    Approved(String),
    /// An approval with no hash (older ledgers): it covers nothing.
    Unbound,
}

/// Words that mean "no".
const REFUSALS: &[&str] = &[
    "deny", "denied", "no", "never", "decline", "declined", "false",
];

/// Read a recorded choice.
pub fn parse_choice(choice: &str) -> ApprovalChoice {
    let c = choice.trim();
    let lower = c.to_ascii_lowercase();
    if REFUSALS.contains(&lower.as_str()) {
        return ApprovalChoice::Refused;
    }
    match lower.strip_prefix("approved:") {
        Some(h) if !h.trim().is_empty() => ApprovalChoice::Approved(h.trim().to_string()),
        _ => ApprovalChoice::Unbound,
    }
}

/// The commands as plain lines for a person to read before approving. Every command the approval
/// covers is listed, including the mutation runner, the first-run workflow and the release checks:
/// a person can only approve what they can see.
pub fn describe_commands(policy: &Policy) -> Vec<String> {
    let mut out: Vec<String> = policy_commands(policy)
        .into_iter()
        .map(|c| match &c.needs {
            Some(n) => format!("{}   (needs a {n}; not run by Verdict)", c.run),
            None => c.run,
        })
        .collect();
    for (kind, command) in tool_commands(policy) {
        let label = match kind {
            "mutation" => "mutation runner",
            "firstrun" => "first-run command",
            "firstrun-server" => "first-run server",
            "firstrun-browser" => "first-run browser",
            _ => "release check",
        };
        out.push(format!("{command}   ({label})"));
    }
    match desk_programs(policy) {
        Ok(list) => {
            for d in list {
                let mut line = d.command.clone();
                for a in &d.args {
                    line.push(' ');
                    line.push_str(a);
                }
                out.push(format!(
                    "{line}   (desk provider {}, vendor {}, model {})",
                    d.name, d.vendor, d.model
                ));
            }
        }
        Err(e) => out.push(format!("unreadable desk providers: {e}")),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(commands: Value, timeout: Option<&str>) -> Policy {
        let mut p = Policy::builtin().unwrap();
        let mut exec = json!({ "commands": commands });
        if let Some(t) = timeout {
            exec["timeout"] = json!(t);
        }
        p.value["exec"] = exec;
        p
    }

    #[test]
    fn the_hash_follows_every_part_of_the_configuration() {
        let base = policy(json!([{"run":"npm test","tier":"ran"}]), Some("3m"));
        let same = policy(json!([{"run":"npm test","tier":"ran"}]), Some("3m"));
        assert_eq!(commands_hash(&base), commands_hash(&same));
        for changed in [
            policy(json!([{"run":"npm test && evil","tier":"ran"}]), Some("3m")),
            policy(
                json!([{"run":"npm test","tier":"ran"},{"run":"x","tier":"ran"}]),
                Some("3m"),
            ),
            policy(json!([{"run":"npm test","tier":"browsed"}]), Some("3m")),
            policy(json!([{"run":"npm test","tier":"ran"}]), Some("4m")),
            policy(json!([{"run":"npm test","tier":"ran"}]), None),
            policy(
                json!([{"run":"npm test","tier":"ran","needs":"server"}]),
                Some("3m"),
            ),
        ] {
            assert_ne!(commands_hash(&base), commands_hash(&changed));
        }
    }

    #[test]
    fn a_choice_reads_back() {
        let p = policy(json!([{"run":"a"}]), None);
        assert_eq!(
            parse_choice(&approval_choice(&p)),
            ApprovalChoice::Approved(commands_hash(&p))
        );
        assert_eq!(parse_choice("approved"), ApprovalChoice::Unbound);
        assert_eq!(parse_choice("approved:"), ApprovalChoice::Unbound);
        assert_eq!(parse_choice("Deny"), ApprovalChoice::Refused);
    }

    #[test]
    fn powershell_scripts_are_spotted() {
        assert!(is_powershell_script(r#".\scripts\check.ps1 -Fast"#));
        assert!(is_powershell_script(r#""a b\check.PS1" -x"#));
        assert!(!is_powershell_script("npm test"));
    }

    #[test]
    fn the_approval_card_lists_every_command_the_approval_covers() {
        let mut p = Policy::builtin().unwrap();
        p.value["exec"]["commands"] = json!([{"run":"npm test","tier":"ran"}]);
        p.value["mutation"] = json!({"command":"mutate --json"});
        p.value["firstrun"] = json!({"commands":["first-run.sh"]});
        p.value["release"] = json!({"checks":["release-truth.sh"]});
        let listed = describe_commands(&p);
        for expected in [
            "npm test",
            "mutate --json",
            "first-run.sh",
            "release-truth.sh",
        ] {
            assert!(
                listed.iter().any(|l| l.contains(expected)),
                "{expected} is approved but not shown: {listed:?}"
            );
        }
    }

    #[test]
    fn desk_providers_are_part_of_the_approval_and_shown_on_the_card() {
        let mut p = Policy::builtin().unwrap();
        let without = commands_hash(&p);
        p.value["vendors"]["providers"] = json!([
            {"name":"x","vendor":"openai","model":"m","command":"evil.exe","args":["--a"]}
        ]);
        let with = commands_hash(&p);
        assert_ne!(without, with, "adding a provider asks the human again");
        p.value["vendors"]["providers"][0]["args"] = json!(["--b"]);
        assert_ne!(with, commands_hash(&p), "changing its arguments asks again");
        p.value["vendors"]["providers"][0]["args"] = json!(["--a"]);
        p.value["vendors"]["providers"][0]["command"] = json!("other.exe");
        assert_ne!(with, commands_hash(&p), "changing its program asks again");
        let listed = describe_commands(&p);
        assert!(
            listed
                .iter()
                .any(|l| l.contains("other.exe --a") && l.contains("desk provider x")),
            "{listed:?}"
        );
    }

    #[test]
    fn a_declared_shell_is_part_of_the_approval_and_an_unknown_one_is_refused() {
        let mut p = Policy::builtin().unwrap();
        assert_eq!(policy_shell(&p).unwrap(), Shell::Native);
        p.value["exec"]["shell"] = json!("bash");
        assert_eq!(policy_shell(&p).unwrap(), Shell::Bash);
        let with_bash = commands_hash(&p);
        p.value["exec"]["shell"] = json!("native");
        assert_ne!(
            with_bash,
            commands_hash(&p),
            "the shell is part of the hash"
        );
        p.value["exec"]["shell"] = json!("fish");
        assert!(
            policy_shell(&p).is_err(),
            "a shell this build cannot start is refused"
        );
    }
}
