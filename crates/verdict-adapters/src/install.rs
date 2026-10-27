//! `vdt hooks install|uninstall <host>`: the exact hook registration, merged into what is there.
//!
//! * Claude Code: `<project>/.claude/settings.json`, the `hooks` block.
//! * Codex: `$CODEX_HOME/hooks.json` (normally `~/.codex/hooks.json`), the `hooks` block.
//!
//! Merging keeps every key and every hook that is not ours, in its original order, and replaces
//! only the entries whose command is `vdt hook <this host> <event>`. Installing twice changes
//! nothing and makes no second backup. A file that is not valid JSON is never overwritten.
//!
//! Installing is a human action. Without `--i-am-the-human` the command prints the block and
//! writes nothing. With it, the files are changed and a `decision` record (`install_hooks`,
//! written by `human`) goes into the project's ledger.

use serde_json::{Map, Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use verdict_core::timefmt::now_rfc3339;
use verdict_core::{DecisionKind, Ledger, Record, Writer, find_project_root};

use crate::payload::{EventKind, Host};

/// Seconds the host waits for a hook: the design's 20 for Stop, 10 for the rest.
fn timeout_for(event: EventKind) -> u64 {
    if event == EventKind::Stop { 20 } else { 10 }
}

/// What `vdt hooks ...` was asked to do.
#[derive(Debug, Clone)]
pub struct HooksRequest {
    /// Install (true) or uninstall (false).
    pub install: bool,
    /// Which host.
    pub host: Host,
    /// The project whose `.claude/settings.json` is used and whose ledger records the decision.
    pub project: Option<PathBuf>,
    /// The Codex home folder; `None` means `$CODEX_HOME`, else `~/.codex`.
    pub codex_home: Option<PathBuf>,
    /// The human typed `--i-am-the-human`.
    pub i_am_the_human: bool,
    /// Path to `vdt` to write into the commands; `None` writes plain `vdt` (found on PATH).
    pub vdt_path: Option<String>,
}

/// The command written into a hook entry.
pub fn hook_command(vdt: &str, host: Host, event: EventKind) -> String {
    format!("{vdt} hook {} {}", host.as_str(), event.as_str())
}

/// The program word to use. Plain `vdt` works from PowerShell, cmd and POSIX shells alike; a
/// custom path is accepted only when it needs no quoting (quotes behave differently per shell).
pub fn vdt_program(custom: Option<&str>) -> Result<String, String> {
    match custom {
        None => Ok("vdt".to_string()),
        Some(p) => {
            let p = p.replace('\\', "/");
            if p.chars()
                .any(|c| c.is_whitespace() || "\"'`$&|;<>()".contains(c))
            {
                Err(format!(
                    "the path {p:?} needs quoting, and quotes mean different things in PowerShell, cmd and bash. Put vdt on your PATH (or copy it to a folder without spaces) and leave out --vdt-path"
                ))
            } else {
                Ok(p)
            }
        }
    }
}

fn entry(vdt: &str, host: Host, event: EventKind) -> Value {
    json!({
        "type": "command",
        "command": hook_command(vdt, host, event),
        "timeout": timeout_for(event),
    })
}

/// The exact `hooks` block for `host`: `{ "hooks": { "<Event>": [ { "hooks": [ {...} ] } ] } }`.
pub fn hook_block(vdt: &str, host: Host) -> Value {
    let mut hooks = Map::new();
    for ev in EventKind::ALL {
        hooks.insert(
            ev.as_str().to_string(),
            json!([{ "hooks": [entry(vdt, host, ev)] }]),
        );
    }
    json!({ "hooks": Value::Object(hooks) })
}

/// Is this command one of ours for `host`? The shape is `<...>vdt[.exe] hook <host> <Event>`.
pub fn is_ours(command: &str, host: Host) -> bool {
    let mut words = command.split_whitespace();
    let (Some(prog), Some(hook), Some(h), Some(ev)) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return false;
    };
    let prog = prog.trim_matches(['"', '\'']);
    let name = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
    let stem = name
        .strip_suffix(".exe")
        .or(name.strip_suffix(".EXE"))
        .unwrap_or(name);
    stem == "vdt"
        && hook == "hook"
        && h.eq_ignore_ascii_case(host.as_str())
        && EventKind::parse(ev).is_some()
}

fn owned_entries(hooks: &Map<String, Value>, host: Host) -> Vec<(String, Value)> {
    let mut v = Vec::new();
    for (event, groups) in hooks {
        for g in groups.as_array().into_iter().flatten() {
            for h in g
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|c| is_ours(c, host))
                {
                    v.push((event.clone(), h.clone()));
                }
            }
        }
    }
    v
}

fn hooks_object(root: &mut Value) -> Result<Option<&mut Map<String, Value>>, String> {
    let Some(obj) = root.as_object_mut() else {
        return Err("the file is JSON but not an object".into());
    };
    match obj.get_mut("hooks") {
        None => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m)),
        Some(_) => Err("the file has a \"hooks\" key that is not an object".into()),
    }
}

/// The result of a merge or removal on a parsed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Our entries added.
    pub added: usize,
    /// Our entries removed (replaced entries count here and in `added`).
    pub removed: usize,
    /// False when the file already held exactly the right entries.
    pub changed: bool,
}

/// Merge our entries for `host` into `root` (the parsed file), in place.
pub fn merge_into(root: &mut Value, vdt: &str, host: Host) -> Result<Change, String> {
    let want: Vec<(String, Value)> = EventKind::ALL
        .iter()
        .map(|e| (e.as_str().to_string(), entry(vdt, host, *e)))
        .collect();
    let sorted = |mut v: Vec<(String, Value)>| {
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    if let Some(h) = hooks_object(root)?
        && sorted(owned_entries(h, host)) == sorted(want.clone())
    {
        return Ok(Change {
            added: 0,
            removed: 0,
            changed: false,
        });
    }
    let removed = remove_from(root, host)?.removed;
    let obj = root
        .as_object_mut()
        .ok_or("the file is not a JSON object")?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or("the \"hooks\" key is not an object")?;
    for (event, e) in &want {
        let groups = hooks
            .entry(event.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let groups = groups
            .as_array_mut()
            .ok_or_else(|| format!("hooks.{event} is not a list"))?;
        let plain = groups
            .iter_mut()
            .find(|g| g.get("matcher").is_none() && g.get("hooks").is_some_and(Value::is_array));
        match plain {
            Some(g) => {
                if let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    list.push(e.clone());
                }
            }
            None => groups.push(json!({ "hooks": [e.clone()] })),
        }
    }
    Ok(Change {
        added: want.len(),
        removed,
        changed: true,
    })
}

/// Remove our entries for `host` from `root`, in place. Emptied groups and events are dropped;
/// an emptied `hooks` key is dropped; nothing else is touched.
pub fn remove_from(root: &mut Value, host: Host) -> Result<Change, String> {
    let mut removed = 0;
    if let Some(hooks) = hooks_object(root)? {
        let events: Vec<String> = hooks.keys().cloned().collect();
        for event in events {
            let Some(groups) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
                continue;
            };
            let mut kept = Vec::new();
            for mut g in std::mem::take(groups) {
                if let Some(list) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    let before = list.len();
                    list.retain(|h| {
                        !h.get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|c| is_ours(c, host))
                    });
                    removed += before - list.len();
                    if list.is_empty() && before > 0 {
                        continue;
                    }
                }
                kept.push(g);
            }
            if kept.is_empty() {
                hooks.shift_remove(&event);
            } else {
                *groups = kept;
            }
        }
        if hooks.is_empty()
            && let Some(o) = root.as_object_mut()
        {
            o.shift_remove("hooks");
        }
    }
    Ok(Change {
        added: 0,
        removed,
        changed: removed > 0,
    })
}

/// Where the registration for `host` lives.
pub fn target_path(
    host: Host,
    project: &Path,
    codex_home: Option<&Path>,
) -> Result<PathBuf, String> {
    match host {
        Host::Claude => Ok(project.join(".claude").join("settings.json")),
        Host::Codex => {
            let home = match codex_home {
                Some(h) => h.to_path_buf(),
                None => std::env::var_os("CODEX_HOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| verdict_core::policy::home_dir().map(|h| h.join(".codex")))
                    .ok_or("cannot find the Codex home folder (set CODEX_HOME)")?,
            };
            Ok(home.join("hooks.json"))
        }
    }
}

fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match fs::read_to_string(path) {
        Ok(s) if s.trim().is_empty() => Ok(Some(Value::Object(Map::new()))),
        Ok(s) => serde_json::from_str(&s).map(Some).map_err(|e| {
            format!(
                "{} is not valid JSON, so it was left alone: {e}",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

fn backup(path: &Path) -> Result<PathBuf, String> {
    let digits: String = now_rfc3339().chars().filter(char::is_ascii_digit).collect();
    let stamp = format!("{}-{}", &digits[..8], &digits[8..14]);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let base = path.with_file_name(format!("{name}.bak-vdt-{stamp}"));
    let mut bak = base.clone();
    let mut n = 2;
    while bak.exists() {
        bak = PathBuf::from(format!("{}-{n}", base.display()));
        n += 1;
    }
    fs::copy(path, &bak).map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
    Ok(bak)
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("vdt-tmp{}", std::process::id()));
    let mut text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(&tmp, text).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", path.display())
    })
}

/// What installing or uninstalling did to a file.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// The file.
    pub path: PathBuf,
    /// The backup made, if the file existed and changed.
    pub backup: Option<PathBuf>,
    /// The change.
    pub change: Change,
}

/// Merge our hooks into `path` (creating it if needed), with a backup.
pub fn install_file(path: &Path, vdt: &str, host: Host) -> Result<Outcome, String> {
    let existed = read_json(path)?;
    let mut doc = existed.clone().unwrap_or_else(|| Value::Object(Map::new()));
    let change = merge_into(&mut doc, vdt, host)?;
    let mut bak = None;
    if change.changed {
        if existed.is_some() {
            bak = Some(backup(path)?);
        }
        write_json(path, &doc)?;
    }
    Ok(Outcome {
        path: path.to_path_buf(),
        backup: bak,
        change,
    })
}

/// Remove our hooks from `path`, with a backup. A missing file is nothing to do.
pub fn uninstall_file(path: &Path, host: Host) -> Result<Outcome, String> {
    let Some(mut doc) = read_json(path)? else {
        return Ok(Outcome {
            path: path.to_path_buf(),
            backup: None,
            change: Change {
                added: 0,
                removed: 0,
                changed: false,
            },
        });
    };
    let change = remove_from(&mut doc, host)?;
    let mut bak = None;
    if change.changed {
        bak = Some(backup(path)?);
        write_json(path, &doc)?;
    }
    Ok(Outcome {
        path: path.to_path_buf(),
        backup: bak,
        change,
    })
}

/// Run `vdt hooks install|uninstall`. Returns the exit code (0 fine, 1 error).
pub fn hooks_command(req: &HooksRequest, cwd: &Path, out: &mut dyn Write) -> Result<u8, String> {
    let io = |e: std::io::Error| e.to_string();
    let project = find_project_root(req.project.as_deref().unwrap_or(cwd));
    let vdt = vdt_program(req.vdt_path.as_deref())?;
    let path = target_path(req.host, &project, req.codex_home.as_deref())?;
    let verb = if req.install { "install" } else { "uninstall" };
    if !req.i_am_the_human {
        writeln!(
            out,
            "Nothing was changed. Installing or removing hooks is a decision only you make, so vdt will not do it unless you add --i-am-the-human."
        )
        .map_err(io)?;
        writeln!(out, "File that would change: {}", path.display()).map_err(io)?;
        if req.install {
            writeln!(
                out,
                "This block would be merged in (your other hooks stay):\n"
            )
            .map_err(io)?;
            let block = serde_json::to_string_pretty(&hook_block(&vdt, req.host))
                .map_err(|e| e.to_string())?;
            writeln!(out, "{block}").map_err(io)?;
        } else {
            writeln!(
                out,
                "Every entry of the form `{vdt} hook {} <Event>` would be removed; nothing else.",
                req.host.as_str()
            )
            .map_err(io)?;
        }
        writeln!(
            out,
            "\nTo {verb} it, run the same command again with --i-am-the-human."
        )
        .map_err(io)?;
        return Ok(0);
    }
    let outcome = if req.install {
        install_file(&path, &vdt, req.host)?
    } else {
        uninstall_file(&path, req.host)?
    };
    let summary = match (req.install, outcome.change.changed) {
        (true, true) => format!(
            "installed {} {} hooks in {}",
            outcome.change.added,
            req.host.as_str(),
            path.display()
        ),
        (true, false) => format!("already installed in {}; nothing changed", path.display()),
        (false, true) => format!(
            "removed {} {} hooks from {}",
            outcome.change.removed,
            req.host.as_str(),
            path.display()
        ),
        (false, false) => format!(
            "no {} hooks found in {}; nothing changed",
            req.host.as_str(),
            path.display()
        ),
    };
    writeln!(out, "{summary}").map_err(io)?;
    if let Some(b) = &outcome.backup {
        writeln!(out, "backup of the old file: {}", b.display()).map_err(io)?;
    }
    if outcome.change.changed {
        let mut ledger = Ledger::open(&project).map_err(|e| {
            format!("the hooks file was changed but the decision could not be recorded: {e}")
        })?;
        let appended = ledger
            .append(
                Writer::Human,
                None,
                Record::Decision {
                    decision: DecisionKind::InstallHooks,
                    choice: format!("{verb} {} hooks in {}", req.host.as_str(), path.display()),
                    reason: None,
                    who: Writer::Human,
                },
            )
            .map_err(|e| {
                format!("the hooks file was changed but the decision could not be recorded: {e}")
            })?;
        writeln!(out, "decision recorded as ledger record {}", appended.seq).map_err(io)?;
    }
    if req.install && req.host == Host::Codex && outcome.change.changed {
        writeln!(
            out,
            "Codex will not run these hooks until you review and trust them: open /hooks in Codex and trust the new entries."
        )
        .map_err(io)?;
    }
    Ok(0)
}
