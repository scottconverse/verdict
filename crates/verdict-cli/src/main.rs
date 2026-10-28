//! `vdt`, the Verdict command line (crate `verdict-gate`).
//!
//! Every command works from any folder inside a project: the project is the nearest folder above
//! the current one that holds `.verdict/ledger.db`, else the nearest git root, else the current
//! folder. Read commands never create files.
//!
//! Exit codes: 0 fine, 1 the request could not be served (for example an unknown session),
//! 2 the ledger failed verification.

mod p2b;
mod runner;

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use verdict_core::verdict::report_for_project;
use verdict_core::{Ledger, LedgerError, find_project_root, list_sessions};

#[derive(Parser)]
#[command(
    name = "vdt",
    version,
    about = "Verdict: the agent writes claims, never verdicts."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Work with the evidence ledger.
    Ledger {
        #[command(subcommand)]
        action: LedgerAction,
    },
    /// Print the plain-text report for one session (default: the latest).
    Report {
        /// Session id, as shown by `vdt sessions`.
        #[arg(long)]
        session: Option<String>,
    },
    /// List the sessions in this project's ledger.
    Sessions,
    /// Called by Claude Code and Codex hooks: `vdt hook <claude|codex> <event>`. Reads the host's
    /// JSON payload on stdin, records it in shadow mode, prints nothing and always exits 0.
    Hook {
        /// The host and the event, for example `claude PostToolUse`.
        #[arg(num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install or remove the hook registration for a host.
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Register the Verdict MCP server with a host (human-only, like installing hooks).
    McpInstall {
        /// The host: claude, cursor, codex, dsh or antigravity.
        host: String,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        /// You, the human, are doing this. Without it vdt only prints what it would change.
        #[arg(long = "i-am-the-human")]
        i_am_the_human: bool,
        /// Path to vdt to register (default: plain `vdt`, found on PATH).
        #[arg(long)]
        vdt_path: Option<String>,
        /// Codex home folder (default: $CODEX_HOME, else ~/.codex).
        #[arg(long)]
        codex_home: Option<std::path::PathBuf>,
    },
    /// Run the MCP server on standard input and output (for an agent to connect to).
    Mcp {
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// Run deterministic checkers on the newest session (or start one) and print one line each.
    Check {
        /// Checker names, comma separated: baseline_diff, regressions, tests_weakened,
        /// claims_vs_code, unfinished, exec. `*` means all; `-name` removes one.
        #[arg(long)]
        checkers: String,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
        /// Lane to run under: low, standard, high or release.
        #[arg(long, default_value = "standard")]
        lane: String,
    },
    /// Compute the verdict of a session from the ledger, write it (as the core) and print the report.
    Verdict {
        /// Session id (default: the latest).
        #[arg(long)]
        session: Option<String>,
        /// Run the lane's checkers first (through the checker runner wired into this build).
        #[arg(long)]
        run: bool,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// The pre-commit gate: check, compute the verdict, hold only in enforce mode.
    Gate {
        #[command(subcommand)]
        action: p2b::GateAction,
    },
    /// Commit past a FAILED pre-commit gate, with a reason you type (human only).
    CommitAnyway(p2b::CommitAnywayArgs),
    /// Read the whole project for broken calls, routes nothing calls, controls with no handler,
    /// called stubs, unfinished work and regressions. No session, no claims; writes nothing and
    /// always exits 0.
    Scan {
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// Run the lane's LLM desks through the providers the policy configures, and print one line
    /// per receipt. Above the cross-vendor lane a same-vendor provider is refused (exit 3).
    Desks {
        /// Session id (default: the latest).
        #[arg(long)]
        session: Option<String>,
        /// Lane to run under (default: the lane the session's changes imply).
        #[arg(long)]
        lane: Option<String>,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// Write the host skill files and hook registrations from the effective policy. The output is
    /// byte-identical for the same policy on any machine.
    Generate {
        /// Folder to write the tree into (default: `<project>/.verdict/generated`).
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// Export one session: `--sarif` for an editor or CI, `--packet` for a release's acceptance
    /// document. Neither writes to the ledger.
    Export {
        /// SARIF 2.1.0 (GitHub code scanning, VS Code's SARIF viewer, most CI).
        #[arg(long)]
        sarif: bool,
        /// The acceptance packet: claims, findings, decisions, overrides, what was not established.
        #[arg(long)]
        packet: bool,
        /// Session id (default: the latest).
        #[arg(long)]
        session: Option<String>,
        /// Write to this file instead of standard output.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
    /// Show the ledger in a browser on this machine only (127.0.0.1). Pressing a button on the
    /// page records one of your decisions; nothing else on it writes.
    Serve {
        /// Port to listen on.
        #[arg(long, default_value_t = verdict_gui::DEFAULT_PORT)]
        port: u16,
        /// Project folder (default: found from the current folder).
        #[arg(long)]
        project: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum HooksAction {
    /// Add the vdt hooks to a host's settings, merged with what is there.
    Install(HooksArgs),
    /// Remove only the vdt hooks from a host's settings.
    Uninstall(HooksArgs),
}

#[derive(clap::Args)]
struct HooksArgs {
    /// The host: claude or codex.
    host: String,
    /// The project folder (Claude Code settings live in its .claude folder; the decision is
    /// recorded in its ledger). Default: the current folder's project.
    #[arg(long)]
    project: Option<std::path::PathBuf>,
    /// You, the human, are doing this. Without it vdt only prints what it would change.
    #[arg(long = "i-am-the-human")]
    i_am_the_human: bool,
    /// Path to vdt to write into the hook commands (default: plain `vdt`, found on PATH).
    #[arg(long)]
    vdt_path: Option<String>,
}

#[derive(Subcommand)]
enum LedgerAction {
    /// Check the hash chain; say which record broke if it did not hold.
    Verify,
    /// Write every record as JSON lines to standard output.
    Export {
        /// JSON lines (the only format; accepted for clarity).
        #[arg(long)]
        jsonl: bool,
    },
}

fn main() -> ExitCode {
    // A hook must never fail the agent, so it is handled before argument parsing can reject
    // anything: `vdt hook ...` always exits 0.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("hook") {
        verdict_adapters::run_hook_cli(&argv[1..]);
        return ExitCode::SUCCESS;
    }
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("vdt: cannot read the current folder: {e}");
            return ExitCode::from(1);
        }
    };
    match &cli.command {
        Command::Mcp { project } => {
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            return match verdict_mcp::serve_stdio(root) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("vdt: mcp server stopped: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Check {
            checkers,
            project,
            lane,
        } => {
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            return match verdict_checkers::cli_check(&root, checkers, lane, &mut std::io::stdout())
            {
                Ok(code) => ExitCode::from(code),
                Err(e) => {
                    eprintln!("vdt: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Scan { project } => {
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            let report = verdict_checkers::scan::scan(&root, None).render();
            print!("{report}");
            return ExitCode::SUCCESS;
        }
        Command::Desks {
            session,
            lane,
            project,
        } => {
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            let mut out = std::io::stdout().lock();
            return match runner::desks(&root, session.as_deref(), lane.as_deref(), &mut out) {
                Ok(code) => ExitCode::from(code),
                Err(e) => {
                    eprintln!("vdt: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Generate { out, project } => {
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            let (policy, warning) = match verdict_core::verdict::load_policy(&root, None) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("vdt: {e}");
                    return ExitCode::from(1);
                }
            };
            if let Some(w) = warning {
                eprintln!(
                    "vdt: the policy file could not be used, so the built-in default is in force: {w}"
                );
            }
            let out = out
                .clone()
                .unwrap_or_else(|| root.join(".verdict/generated"));
            return match verdict_generate::write_tree(&policy, &out) {
                Ok(paths) => {
                    println!(
                        "generated {} file(s) in {} from policy v{} ({})",
                        paths.len(),
                        out.display(),
                        policy.version,
                        policy.hash
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("vdt: cannot write the generated tree: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Export {
            sarif,
            packet,
            session,
            out,
            project,
        } => {
            if *sarif == *packet {
                eprintln!("vdt: choose exactly one of --sarif or --packet");
                return ExitCode::from(1);
            }
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            let ledger = match Ledger::open_existing(&root) {
                Ok(Some(l)) => l,
                Ok(None) => {
                    eprintln!("vdt: there is no ledger in {} yet", root.display());
                    return ExitCode::from(1);
                }
                Err(e) => {
                    eprintln!("vdt: {e}");
                    return ExitCode::from(1);
                }
            };
            let session = match verdict_core::part1c::resolve_session(&ledger, session.as_deref()) {
                Ok(Some(s)) => s,
                Ok(None) => {
                    eprintln!("vdt: this project's ledger has no sessions yet");
                    return ExitCode::from(1);
                }
                Err(e) => {
                    eprintln!("vdt: {e}");
                    return ExitCode::from(1);
                }
            };
            let text = if *sarif {
                verdict_core::export::sarif(&ledger, Some(&session))
            } else {
                verdict_core::export::packet(&root, &ledger, &session)
            };
            return match text {
                Ok(t) => match out {
                    Some(path) => match std::fs::write(path, &t) {
                        Ok(()) => {
                            println!("wrote {}", path.display());
                            ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("vdt: cannot write {}: {e}", path.display());
                            ExitCode::from(1)
                        }
                    },
                    None => {
                        print!("{t}");
                        ExitCode::SUCCESS
                    }
                },
                Err(e) => {
                    eprintln!("vdt: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Serve { port, project } => {
            let root = find_project_root(project.as_deref().unwrap_or(&cwd));
            return match verdict_gui::serve(root, *port) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("vdt: cannot serve the page on 127.0.0.1:{port}: {e}");
                    ExitCode::from(1)
                }
            };
        }
        Command::Verdict {
            session,
            run,
            project,
        } => return p2b::verdict(session.as_deref(), *run, project.as_deref(), &cwd),
        Command::Gate { action } => return p2b::gate(action, &cwd),
        Command::CommitAnyway(args) => return p2b::commit_anyway(args, &cwd),
        _ => {}
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match run(cli, &cwd, &mut out) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("vdt: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli, cwd: &Path, out: &mut dyn Write) -> Result<u8, LedgerError> {
    match &cli.command {
        Command::Hook { args } => {
            verdict_adapters::run_hook_cli(args);
            return Ok(0);
        }
        Command::Hooks { action } => {
            let (install, a) = match action {
                HooksAction::Install(a) => (true, a),
                HooksAction::Uninstall(a) => (false, a),
            };
            let Some(host) = verdict_adapters::Host::parse(&a.host) else {
                eprintln!(
                    "vdt: unknown host {:?} (use claude, codex, cursor or antigravity)",
                    a.host
                );
                return Ok(1);
            };
            let req = verdict_adapters::HooksRequest {
                install,
                host,
                project: a.project.clone(),
                codex_home: None,
                i_am_the_human: a.i_am_the_human,
                vdt_path: a.vdt_path.clone(),
            };
            return match verdict_adapters::hooks_command(&req, cwd, out) {
                Ok(code) => Ok(code),
                Err(e) => {
                    eprintln!("vdt: {e}");
                    Ok(1)
                }
            };
        }
        Command::McpInstall {
            host,
            project,
            i_am_the_human,
            vdt_path,
            codex_home,
        } => {
            let Some(host) = verdict_adapters::McpHost::parse(host) else {
                eprintln!(
                    "vdt: unknown host {host:?} (use claude, cursor, codex, dsh or antigravity)"
                );
                return Ok(1);
            };
            let req = verdict_adapters::McpRequest {
                host,
                project: project.clone(),
                vdt_path: vdt_path.clone(),
                codex_home: codex_home.clone(),
                i_am_the_human: *i_am_the_human,
            };
            return match verdict_adapters::mcp_command(&req, cwd, out) {
                Ok(code) => Ok(code),
                Err(e) => {
                    eprintln!("vdt: {e}");
                    Ok(1)
                }
            };
        }
        _ => {}
    }
    let root = find_project_root(cwd);
    let Some(ledger) = Ledger::open_existing(&root)? else {
        match cli.command {
            Command::Ledger {
                action: LedgerAction::Export { .. },
            } => {} // an empty export is empty output
            Command::Sessions => writeln!(
                out,
                "no sessions recorded yet (no ledger in {})",
                root.display()
            )?,
            Command::Report { .. } => writeln!(
                out,
                "no sessions recorded yet (no ledger in {})",
                root.display()
            )?,
            Command::Hook { .. } | Command::Hooks { .. } | Command::McpInstall { .. } => {}
            Command::Ledger { .. } => writeln!(
                out,
                "no ledger in {} yet: nothing to verify",
                root.display()
            )?,
            Command::Mcp { .. }
            | Command::Serve { .. }
            | Command::Scan { .. }
            | Command::Check { .. }
            | Command::Desks { .. }
            | Command::Generate { .. }
            | Command::Export { .. }
            | Command::Verdict { .. }
            | Command::Gate { .. }
            | Command::CommitAnyway(_) => {} // handled in main
        }
        return Ok(0);
    };
    match cli.command {
        Command::Ledger {
            action: LedgerAction::Verify,
        } => {
            let report = ledger.verify()?;
            writeln!(out, "{}", report.summary())?;
            Ok(if report.is_ok() { 0 } else { 2 })
        }
        Command::Ledger {
            action: LedgerAction::Export { .. },
        } => {
            ledger.export_jsonl(out)?;
            Ok(0)
        }
        Command::Sessions => {
            let sessions = list_sessions(&ledger)?;
            if sessions.is_empty() {
                writeln!(out, "no sessions recorded yet")?;
            }
            for s in sessions {
                writeln!(
                    out,
                    "{}  started {}  agent {}  model {}  vendor {}  policy v{}  {} records",
                    s.id,
                    s.started_at,
                    s.agent,
                    s.model,
                    s.vendor,
                    s.policy_version,
                    s.record_count
                )?;
            }
            Ok(0)
        }
        Command::Report { session } => {
            if session.is_none() && list_sessions(&ledger)?.is_empty() {
                writeln!(out, "no sessions recorded yet")?;
                return Ok(0);
            }
            let text = report_for_project(&root, &ledger, session.as_deref(), None)?;
            write!(out, "{text}")?;
            Ok(if ledger.verify()?.is_ok() { 0 } else { 2 })
        }
        Command::Hook { .. } | Command::Hooks { .. } | Command::McpInstall { .. } => Ok(0),
        Command::Mcp { .. }
        | Command::Serve { .. }
        | Command::Scan { .. }
        | Command::Check { .. }
        | Command::Desks { .. }
        | Command::Generate { .. }
        | Command::Export { .. }
        | Command::Verdict { .. }
        | Command::Gate { .. }
        | Command::CommitAnyway(_) => Ok(0), // handled in main
    }
}
