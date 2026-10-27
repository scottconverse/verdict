# Changelog

All notable changes to this project are recorded here. The format follows Keep a Changelog, and
the project uses semantic versioning once it ships (v1.0 is Phase 5).

## [Unreleased]

### Added

- **Phase 1 — ledger and adapters.** SQLite ledger at `.verdict/ledger.db` with an append-only
  blake3 hash chain, JSONL export and a `ledger verify` that names a broken record. Session,
  prompt, claim, tool_call, check, finding, decision, verdict, refused_write, check_unavailable and
  correction records. Claude Code and Codex hook adapters doing the three jobs (prompt, tool call,
  Stop) with a fail-open budget, an installer that shows the exact block before writing and keeps a
  backup, and a payload parity test. MCP server on stdio with `verdict_check`, `verdict_scan`,
  `verdict_claim`, `verdict_done_when` and `verdict_decisions`; an agent write of a finding,
  decision, verdict or mode switch is refused by the ledger and recorded. Read-only localhost page.
- **Phase 2 — deterministic checkers.** baseline_diff, regressions, tests_weakened, claims_vs_code,
  unfinished and exec, each with red and green mutation fixtures; the `stub-checkers` feature and
  the `fixtures-red-must-fail` workflow that fails when a stubbed checker lets a red fixture pass.
  Verdict computation, lane classification, the pre-commit git hook, commit anyway with a typed
  reason, and the decisions queue (approve commands, accept a finding, accept a prompt translation,
  raise the lane, switch mode, pause).
- **Phase 3 — wiring and scan.** The tree-sitter extractor crate and the wiring checker built in
  house: FastAPI (APIRouter prefixes, include_router), SQLAlchemy/asyncpg data writes, Alembic
  migrations, Typer, Python and TypeScript MCP tools, TypeScript fetch and openapi-ts clients,
  TanStack Start server functions, Kysely writes, Tauri commands and React controls. The five
  wiring words with ticked/unticked severity, SKIPPED never clean, `vdt scan` and `verdict_scan`,
  and a 1,000-file scan well under the 30-second requirement.
- Policy files: `policy/default.json` (now neutral: no plan, commands, blast globs, verifier names
  or project paths) and `policy/schema.json`.
- Workflows: `test` (Ubuntu, macOS, Windows), `fixtures-red-must-fail`, and a `dogfood-release`
  placeholder for Phase 5.

### Changed

- The built-in default policy no longer names another application's npm commands, web-project
  paths, release paths, plan file or a machine-specific verifier. Project-approved configuration
  supplies them; a missing command earns no execution proof, and missing blast classification is
  stated in the report and never lowers a lane.

### Fixed

- The checkers module documentation said six checkers; there are seven (wiring was missing).
- The hook-install cards on the session report page were a link only; each now has a button that
  records the human's decision, with the link kept.

### Not built yet

- Phase 4: packaged desk rubrics, desk dispatch with citations, cross-vendor providers and refusal,
  `vdt generate`, Cursor and Antigravity adapters.
- Phase 5: enforce-mode send-backs, release lane, SARIF and packet export, GUI installer.
