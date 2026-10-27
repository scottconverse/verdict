//! The one place the real checkers plug in.
//!
//! The gate and `vdt verdict --run` reach checkers only through [`verdict_core::gate::CheckRunner`].
//! [`RealRunner`] adapts it to `verdict_checkers::run_lane`, which writes each checker's `check`
//! record (as the core) and its findings (as the checker) into the ledger.

use std::path::Path;

use verdict_core::Ledger;
use verdict_core::gate::{CheckRunner, CheckSummary, RunOptions};

/// Runs the deterministic checkers from crate `verdict-checkers`.
pub struct RealRunner;

impl CheckRunner for RealRunner {
    fn run_lane(
        &self,
        ledger: &mut Ledger,
        root: &Path,
        session: &str,
        checkers: &[String],
        opts: &RunOptions,
    ) -> Result<Vec<CheckSummary>, String> {
        let o = verdict_checkers::RunOptions {
            lane: opts.lane,
            timeout: opts.timeout,
        };
        verdict_checkers::run_lane(ledger, root, session, checkers, &o).map(|v| {
            v.into_iter()
                .map(|s| CheckSummary {
                    name: s.name,
                    outcome: s.outcome,
                    findings: s.findings,
                    check_seq: s.check_seq,
                })
                .collect()
        })
    }
}

/// The runner `vdt` uses.
pub fn default_runner() -> Box<dyn CheckRunner> {
    Box::new(RealRunner)
}
