//! The one place the real checkers and desks plug in.
//!
//! The gate and `vdt verdict --run` reach checkers only through [`verdict_core::gate::CheckRunner`].
//! [`RealRunner`] now lives in `verdict-engine`, together with the fresh-verification path the Stop
//! hook and the MCP `verdict_check` use, so there is exactly one checker and desk implementation.

use std::path::Path;
use std::time::Duration;

use verdict_core::gate::CheckRunner;

/// The runner `vdt` uses.
pub fn default_runner() -> Box<dyn CheckRunner> {
    Box::new(verdict_engine::RealRunner::default())
}

/// `vdt desks`: run the lane's desks for the newest session (or `--session`) and print one line
/// per receipt. Returns the exit code: 0 ran, 3 refused by the cross-vendor rule.
pub fn desks(
    root: &Path,
    session: Option<&str>,
    lane: Option<&str>,
    out: &mut dyn std::io::Write,
) -> Result<u8, String> {
    let mut ledger = verdict_core::Ledger::open(root).map_err(|e| e.to_string())?;
    let session = verdict_core::part1c::resolve_session(&ledger, session)
        .map_err(|e| e.to_string())?
        .ok_or("this project's ledger has no sessions yet")?;
    let (policy, warning) =
        verdict_core::verdict::load_policy(root, None).map_err(|e| e.to_string())?;
    if let Some(w) = warning {
        writeln!(out, "policy problem, the built-in default is in force: {w}")
            .map_err(|e| e.to_string())?;
    }
    let lane = match lane {
        Some(l) => verdict_core::lane::parse_lane(l)
            .ok_or_else(|| format!("unknown lane {l:?} (use low, standard, high or release)"))?,
        None => {
            let live = verdict_core::verdict::live_input(root, &ledger, &session, None)
                .map_err(|e| e.to_string())?;
            verdict_core::lane::effective_lane(&ledger, &session, live.input.implied_lane)
                .map_err(|e| e.to_string())?
        }
    };
    let specs = verdict_desks::providers_from_policy(&policy)?;
    let providers: Vec<Box<dyn verdict_desks::DeskProvider>> = specs
        .into_iter()
        .map(|p| Box::new(p) as Box<dyn verdict_desks::DeskProvider>)
        .collect();
    let opts = verdict_desks::DeskOptions {
        lane,
        timeout: Duration::from_secs(300),
    };
    match verdict_desks::run(&mut ledger, root, &session, &policy, &opts, &providers)
        .map_err(|e| e.to_string())?
    {
        verdict_desks::Outcome::Ran {
            receipts,
            single_vendor,
        } => {
            if receipts.is_empty() {
                writeln!(
                    out,
                    "no desks run in the {} lane",
                    verdict_core::lane::lane_name(lane)
                )
                .map_err(|e| e.to_string())?;
            }
            for r in &receipts {
                writeln!(out, "{}", verdict_desks::receipt_line(r)).map_err(|e| e.to_string())?;
            }
            if single_vendor {
                writeln!(
                    out,
                    "single_vendor: the cross-vendor rule was downgraded to warn-only by the global policy; this run is not independent."
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(0)
        }
        verdict_desks::Outcome::Refused { reason, desks } => {
            writeln!(
                out,
                "REFUSED: {reason}\nNo desk ran ({}); the ledger records one check_unavailable per desk, so no verdict can read this as clean.",
                desks.join(", ")
            )
            .map_err(|e| e.to_string())?;
            Ok(3)
        }
    }
}
