//! `vdt`, the Verdict command line (crate `verdict-gate`).
//!
//! Every command works from any folder inside a project: the project is the nearest folder above
//! the current one that holds `.verdict/ledger.db`, else the nearest git root, else the current
//! folder. Read commands never create files.
//!
//! Exit codes: 0 fine, 1 the request could not be served (for example an unknown session),
//! 2 the ledger failed verification.

use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use verdict_core::{Ledger, LedgerError, find_project_root, list_sessions, render_report};

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
    let cli = Cli::parse();
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("vdt: cannot read the current folder: {e}");
            return ExitCode::from(1);
        }
    };
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
            Command::Ledger { .. } => writeln!(
                out,
                "no ledger in {} yet: nothing to verify",
                root.display()
            )?,
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
            let text = render_report(&ledger, session.as_deref())?;
            write!(out, "{text}")?;
            Ok(if ledger.verify()?.is_ok() { 0 } else { 2 })
        }
    }
}
