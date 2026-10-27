//! The one place the real checkers plug in.
//!
//! The gate and `vdt verdict --run` reach checkers only through [`verdict_core::gate::CheckRunner`].
//! Part 2a builds `verdict_checkers::run_lane`; the coordinator wires it here at merge:
//!
//! ```ignore
//! struct RealRunner;
//! impl CheckRunner for RealRunner {
//!     fn run_lane(&self, ledger: &mut Ledger, root: &Path, session: &str,
//!                 checkers: &[String], opts: &RunOptions) -> Result<Vec<CheckSummary>, String> {
//!         let o = verdict_checkers::RunOptions { lane: opts.lane, timeout: opts.timeout };
//!         verdict_checkers::run_lane(ledger, root, session, checkers, &o).map(|v| {
//!             v.into_iter().map(|s| CheckSummary {
//!                 name: s.name, outcome: s.outcome, findings: s.findings, check_seq: s.check_seq,
//!             }).collect()
//!         })
//!     }
//! }
//! ```
//!
//! Until then [`default_runner`] returns [`NoRunner`], which runs nothing and says so: every
//! required checker is recorded as unavailable and the verdict reads PARTIAL, never clean.

use verdict_core::gate::{CheckRunner, NoRunner};

/// The runner `vdt` uses. Replace the body at merge (see the module docs).
pub fn default_runner() -> Box<dyn CheckRunner> {
    Box::new(NoRunner)
}
