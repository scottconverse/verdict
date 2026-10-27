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

/// One command from the policy's `exec.commands`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecCommand {
    /// The command line.
    pub run: String,
    /// The tier it earns.
    pub tier: Tier,
    /// What it needs, for example `server`.
    pub needs: Option<String>,
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

/// The shell a command runs in on this platform.
pub fn shell_for(cmd: &str) -> &'static str {
    if is_powershell_script(cmd) {
        "pwsh-or-powershell -File"
    } else if cfg!(windows) {
        "cmd /D /S /C"
    } else {
        "sh -c"
    }
}

/// The effective configuration in a fixed text form: one line per command, then the timeout.
/// This is what gets hashed, and it is also a readable summary.
pub fn config_text(policy: &Policy) -> String {
    let mut out = String::from("verdict-exec-approval-v1\n");
    for c in policy_commands(policy) {
        out.push_str(&format!(
            "command\t{}\ttier={}\tneeds={}\tshell={}\n",
            c.run.replace(['\n', '\r', '\t'], " "),
            c.tier.as_str(),
            c.needs.as_deref().unwrap_or("-"),
            shell_for(&c.run)
        ));
    }
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

/// The commands as plain lines for a person to read before approving.
pub fn describe_commands(policy: &Policy) -> Vec<String> {
    policy_commands(policy)
        .into_iter()
        .map(|c| match &c.needs {
            Some(n) => format!("{}   (needs a {n}; not run by Verdict)", c.run),
            None => c.run,
        })
        .collect()
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
}
