//! The Part 2b commands: `vdt verdict`, `vdt gate ...` and `vdt commit-anyway`.

use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use verdict_core::gate::{GateOptions, run_pre_commit, run_verdict};
use verdict_core::githook::{HookRequest, hook_command};
use verdict_core::human::{Action, apply, commit_anyway_plan};
use verdict_core::part1c::resolve_session;
use verdict_core::{Ledger, Record, Writer, find_project_root, render_report_with};

#[derive(Subcommand)]
pub enum GateAction {
    /// The pre-commit check (what the git hook runs): classify the lane, run its checkers,
    /// compute the verdict, print it. Shadow mode (the default) always exits 0; enforce mode
    /// exits 1 on FAILED unless you chose commit anyway for this exact staged tree.
    PreCommit {
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Write the git pre-commit hook (chains any hook already there, with a backup).
    Install(GateHookArgs),
    /// Remove Verdict's pre-commit hook (never touches another tool's hook).
    Uninstall(GateHookArgs),
}

#[derive(clap::Args)]
pub struct GateHookArgs {
    /// Project folder (default: found from the current folder).
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// You, the human, are doing this. Without it vdt only prints what it would change.
    #[arg(long = "i-am-the-human")]
    pub i_am_the_human: bool,
    /// Path to vdt for the hook to call (default: the vdt that is running now).
    #[arg(long)]
    pub vdt_path: Option<String>,
}

#[derive(clap::Args)]
pub struct CommitAnywayArgs {
    /// Why you are committing past a FAILED verdict. Required; you type it.
    #[arg(long)]
    pub reason: String,
    /// Session id (default: the latest).
    #[arg(long)]
    pub session: Option<String>,
    /// The staged tree this covers (default: what is staged now; the gate prints it when it holds).
    #[arg(long)]
    pub tree: Option<String>,
    /// Project folder (default: found from the current folder).
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// You, the human, are doing this. Without it vdt only prints what it would record.
    #[arg(long = "i-am-the-human")]
    pub i_am_the_human: bool,
}

/// `vdt gate ...`
pub fn gate(action: &GateAction, cwd: &Path) -> ExitCode {
    match action {
        GateAction::PreCommit { project } => {
            pre_commit(&find_project_root(project.as_deref().unwrap_or(cwd)))
        }
        GateAction::Install(a) => hooks(true, a, cwd),
        GateAction::Uninstall(a) => hooks(false, a, cwd),
    }
}

fn pre_commit(root: &Path) -> ExitCode {
    let runner = crate::runner::default_runner();
    let result = run_pre_commit(
        &GateOptions {
            root,
            home: None,
            now_ms: None,
        },
        runner.as_ref(),
    );
    match result {
        Ok(outcome) => {
            print!("{}", outcome.text);
            ExitCode::from(outcome.exit_code)
        }
        Err(e) => {
            // Fail open on the tool, never on the verdict: a crash here is not a FAILED verdict.
            eprintln!("vdt: the pre-commit gate could not run ({e}); letting this commit through.");
            if let Ok(mut l) = Ledger::open(root) {
                let session = resolve_session(&l, None).ok().flatten();
                let _ = l.append(
                    Writer::Core,
                    session.as_deref(),
                    Record::CheckUnavailable {
                        component: "gate".into(),
                        reason: e.to_string(),
                    },
                );
            }
            ExitCode::SUCCESS
        }
    }
}

fn hooks(install: bool, a: &GateHookArgs, cwd: &Path) -> ExitCode {
    let project = find_project_root(a.project.as_deref().unwrap_or(cwd));
    let vdt_path = a.vdt_path.clone().or_else(|| {
        let exe = std::env::current_exe().ok()?;
        let stem = exe.file_stem()?.to_string_lossy().to_ascii_lowercase();
        (stem == "vdt").then(|| exe.to_string_lossy().to_string())
    });
    let req = HookRequest {
        install,
        project,
        vdt_path,
        i_am_the_human: a.i_am_the_human,
    };
    let mut out = std::io::stdout().lock();
    match hook_command(&req, &mut out) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("vdt: {e}");
            ExitCode::from(1)
        }
    }
}

/// `vdt verdict`
pub fn verdict(session: Option<&str>, run: bool, project: Option<&Path>, cwd: &Path) -> ExitCode {
    let root = find_project_root(project.unwrap_or(cwd));
    let runner = crate::runner::default_runner();
    match run_verdict(&root, None, session, run.then_some(runner.as_ref())) {
        Ok((c, seq)) => {
            let ledger = match Ledger::open_existing(&root) {
                Ok(Some(l)) => l,
                _ => return ExitCode::from(1),
            };
            match render_report_with(&ledger, Some(&c.session), Some(&c)) {
                Ok(text) => {
                    print!("{text}");
                    println!("\nverdict recorded as ledger record {seq} (written by: core)");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("vdt: {e}");
                    ExitCode::from(1)
                }
            }
        }
        Err(e) => {
            eprintln!("vdt: {e}");
            ExitCode::from(1)
        }
    }
}

/// `vdt commit-anyway`
pub fn commit_anyway(a: &CommitAnywayArgs, cwd: &Path) -> ExitCode {
    let root = find_project_root(a.project.as_deref().unwrap_or(cwd));
    let say = |m: String| eprintln!("vdt: {m}");
    if a.reason.trim().is_empty() {
        say("commit anyway needs a reason you type; an empty reason is refused".into());
        return ExitCode::from(1);
    }
    let mut ledger = match Ledger::open_existing(&root) {
        Ok(Some(l)) => l,
        Ok(None) => {
            say(format!(
                "no ledger in {} yet, so there is no FAILED verdict to commit past",
                root.display()
            ));
            return ExitCode::from(1);
        }
        Err(e) => {
            say(e.to_string());
            return ExitCode::from(1);
        }
    };
    let session = match resolve_session(&ledger, a.session.as_deref()) {
        Ok(Some(s)) => s,
        Ok(None) => {
            say("this project's ledger has no sessions yet".into());
            return ExitCode::from(1);
        }
        Err(e) => {
            say(e.to_string());
            return ExitCode::from(1);
        }
    };
    if !a.i_am_the_human {
        match commit_anyway_plan(&root, &ledger, &session, a.tree.as_deref(), None) {
            Ok(plan) => {
                println!(
                    "Nothing was recorded. Commit anyway is a decision only you make, so vdt will not do it unless you add --i-am-the-human."
                );
                println!(
                    "It would record, as you: commit anyway for staged tree {}",
                    plan.tree
                );
                println!("  past: {}", plan.finding_ids.join(", "));
                println!("  reason you typed: {}", a.reason.trim());
                println!("The commit would go through; the verdict would stay FAILED.");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                say(e);
                return ExitCode::from(1);
            }
        }
    }
    match apply(
        &root,
        &mut ledger,
        &session,
        &Action::CommitAnyway {
            reason: a.reason.clone(),
            tree: a.tree.clone(),
        },
        None,
    ) {
        Ok(seq) => {
            println!("recorded commit anyway as decision record {seq} (written by: human).");
            println!(
                "Commit again: the gate lets this exact staged tree through. The verdict stays FAILED."
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            say(e);
            ExitCode::from(1)
        }
    }
}
