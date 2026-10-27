//! `vdt gate install` and `vdt gate uninstall`: the git pre-commit hook.
//!
//! The hook is a small `sh` script. Git runs hook scripts with its own bundled `sh` on Windows, so
//! the same file works whether the commit is made from cmd, PowerShell, Git Bash or an editor.
//!
//! * An existing pre-commit hook of another tool is **kept**: it is copied to
//!   `pre-commit.verdict-chained` (which the new hook runs first, so its refusals still hold) and a
//!   second copy is kept as `pre-commit.verdict-backup`. Uninstalling puts the original back.
//! * A hook that is not Verdict's is never deleted or overwritten on uninstall.
//! * The hook fails open: if `vdt` is missing or crashes, the commit goes through. Only the exit
//!   code 1 of `vdt gate pre-commit` (enforce mode, FAILED, no commit-anyway) holds a commit.
//! * Installing or removing is a human decision: without `--i-am-the-human` nothing is written.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::lane::git_out;
use crate::ledger::Ledger;
use crate::model::{DecisionKind, Record, Writer};

/// Marker line that says a hook file is Verdict's.
pub const MARKER: &str = "# verdict-gate: managed by `vdt gate install`";
/// The copy of another tool's hook that Verdict's hook runs first.
pub const CHAINED: &str = "pre-commit.verdict-chained";
/// The untouched backup of another tool's hook.
pub const BACKUP: &str = "pre-commit.verdict-backup";

/// What the user asked for.
#[derive(Debug, Clone)]
pub struct HookRequest {
    /// Install (true) or uninstall (false).
    pub install: bool,
    /// The project folder.
    pub project: PathBuf,
    /// How the hook calls vdt (default: plain `vdt`, found on PATH).
    pub vdt_path: Option<String>,
    /// The human is doing this. Without it nothing is written.
    pub i_am_the_human: bool,
}

/// Where git keeps hooks for `project` (honors `core.hooksPath` and worktrees).
pub fn hooks_dir(project: &Path) -> Result<PathBuf, String> {
    let out = git_out(project, &["rev-parse", "--git-path", "hooks"]).ok_or_else(|| {
        format!(
            "{} is not a git repository (or git is not installed), so there is no pre-commit hook to manage",
            project.display()
        )
    })?;
    let text = String::from_utf8_lossy(&out).trim().to_string();
    let p = PathBuf::from(text);
    Ok(if p.is_absolute() { p } else { project.join(p) })
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The text of the hook script.
pub fn hook_script(vdt: &str) -> String {
    let vdt = sh_quote(&vdt.replace('\\', "/"));
    format!(
        "#!/bin/sh\n\
{MARKER}\n\
# Remove it with `vdt gate uninstall`. It fails open: if vdt is missing or crashes, the commit goes\n\
# through. Only exit code 1 from `vdt gate pre-commit` (enforce mode, FAILED, no commit-anyway)\n\
# holds a commit.\n\
hook_dir=$(dirname \"$0\")\n\
if [ -x \"$hook_dir/{CHAINED}\" ]; then\n\
  \"$hook_dir/{CHAINED}\" \"$@\" || exit $?\n\
fi\n\
VDT={vdt}\n\
[ -x \"$VDT\" ] || VDT=vdt\n\
if command -v \"$VDT\" >/dev/null 2>&1; then\n\
  \"$VDT\" gate pre-commit\n\
  code=$?\n\
  if [ \"$code\" -eq 1 ]; then\n\
    exit 1\n\
  fi\n\
else\n\
  echo 'verdict: vdt was not found, so the pre-commit gate was skipped' >&2\n\
fi\n\
exit 0\n"
    )
}

fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains(MARKER))
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn free_backup_name(dir: &Path) -> PathBuf {
    let first = dir.join(BACKUP);
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{BACKUP}-{n}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Run `vdt gate install|uninstall`. Returns the exit code (0 fine). Errors are plain sentences.
pub fn hook_command(req: &HookRequest, out: &mut dyn Write) -> Result<u8, String> {
    let io = |e: std::io::Error| e.to_string();
    let dir = hooks_dir(&req.project)?;
    let hook = dir.join("pre-commit");
    let chained = dir.join(CHAINED);
    let vdt = req.vdt_path.clone().unwrap_or_else(|| "vdt".to_string());
    let verb = if req.install { "install" } else { "uninstall" };
    let existing_other = hook.is_file() && !is_ours(&hook);

    if !req.i_am_the_human {
        writeln!(
            out,
            "Nothing was changed. Installing or removing the pre-commit gate is a decision only you make, so vdt will not do it unless you add --i-am-the-human."
        )
        .map_err(io)?;
        writeln!(out, "Hook file that would change: {}", hook.display()).map_err(io)?;
        if req.install {
            if existing_other {
                writeln!(
                    out,
                    "A pre-commit hook of another tool is already there. It would be kept: copied to {} (run first, so its refusals still hold) and backed up as {}.",
                    chained.display(),
                    dir.join(BACKUP).display()
                )
                .map_err(io)?;
            }
            writeln!(
                out,
                "This script would be written:\n\n{}",
                hook_script(&vdt)
            )
            .map_err(io)?;
        } else if is_ours(&hook) {
            writeln!(
                out,
                "Verdict's hook would be removed{}.",
                if chained.is_file() {
                    " and the hook that was there before put back"
                } else {
                    ""
                }
            )
            .map_err(io)?;
        } else {
            writeln!(out, "There is no Verdict hook there; nothing would change.").map_err(io)?;
        }
        writeln!(
            out,
            "\nTo {verb} it, run the same command again with --i-am-the-human."
        )
        .map_err(io)?;
        return Ok(0);
    }

    let summary: String;
    if req.install {
        std::fs::create_dir_all(&dir).map_err(io)?;
        let mut note = String::new();
        if existing_other {
            if chained.exists() {
                return Err(format!(
                    "{} already exists, so another hook cannot be chained safely. Move it away and run this again.",
                    chained.display()
                ));
            }
            let backup = free_backup_name(&dir);
            std::fs::copy(&hook, &backup).map_err(io)?;
            std::fs::copy(&hook, &chained).map_err(io)?;
            make_executable(&chained);
            note = format!(
                "kept the existing hook: it runs first from {} (backup copy: {})",
                chained.display(),
                backup.display()
            );
        }
        std::fs::write(&hook, hook_script(&vdt)).map_err(io)?;
        make_executable(&hook);
        summary = format!("installed the pre-commit gate in {}", hook.display());
        writeln!(out, "{summary}").map_err(io)?;
        if !note.is_empty() {
            writeln!(out, "{note}").map_err(io)?;
        }
        writeln!(
            out,
            "It starts in shadow mode: it holds nothing until you switch to enforce on the Verdict page."
        )
        .map_err(io)?;
    } else {
        if hook.is_file() && !is_ours(&hook) {
            writeln!(
                out,
                "{} is not Verdict's hook, so it was left alone.",
                hook.display()
            )
            .map_err(io)?;
            return Ok(0);
        }
        if !hook.is_file() {
            writeln!(
                out,
                "no Verdict hook found in {}; nothing changed",
                dir.display()
            )
            .map_err(io)?;
            return Ok(0);
        }
        std::fs::remove_file(&hook).map_err(io)?;
        if chained.is_file() {
            std::fs::rename(&chained, &hook).map_err(io)?;
            writeln!(
                out,
                "put the hook that was there before back in {}",
                hook.display()
            )
            .map_err(io)?;
        }
        summary = format!("removed the pre-commit gate from {}", hook.display());
        writeln!(out, "{summary}").map_err(io)?;
    }

    let mut ledger = Ledger::open(&req.project)
        .map_err(|e| format!("the hook was changed but the decision could not be recorded: {e}"))?;
    let appended = ledger
        .append(
            Writer::Human,
            None,
            Record::Decision {
                decision: DecisionKind::InstallHooks,
                choice: format!("{verb} git pre-commit gate in {}", hook.display()),
                reason: None,
                who: Writer::Human,
            },
        )
        .map_err(|e| format!("the hook was changed but the decision could not be recorded: {e}"))?;
    writeln!(out, "decision recorded as ledger record {}", appended.seq).map_err(io)?;
    Ok(0)
}
