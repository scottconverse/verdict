//! Verdict host adapters: Claude Code and Codex hooks, in shadow mode.
//!
//! An adapter does three jobs and nothing else: capture the prompt, record tool calls, and note
//! the "done" moment. Everything is written to the ledger as writer `adapter`. In this build
//! nothing blocks, denies or decides: the hook prints nothing and always lets the agent continue,
//! and when it cannot record it says so with a `check_unavailable` record.
//!
//! * [`hook::run_hook_cli`] is `vdt hook <host> <event>`.
//! * [`install::hooks_command`] is `vdt hooks install|uninstall <host>`.
//! * [`payload`] reads host payloads; [`recorder`] turns them into ledger records; [`state`] maps a
//!   host's raw session id to one ledger session.

pub mod hook;
pub mod install;
pub mod payload;
pub mod recorder;
pub mod state;

pub use hook::{run_hook, run_hook_cli};
pub use install::{HooksRequest, hooks_command};
pub use payload::{EventKind, Host};
pub use recorder::Options;
