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
- **Phase 4 — desks, providers and generation.** The seven desk rubrics ship inside the binary. A
  Verdict-owned provider interface (one JSON request on stdin, one JSON answer on stdout) dispatches
  a lane's desks; every finding must cite a ledger record or a current file:line, an unresolvable
  citation becomes a hypothesis, and each desk leaves a receipt naming the provider, vendor,
  requested and served model, outcome and usage. Above the cross-vendor lane a same-vendor provider
  is refused unless the global policy downgrades it to warn-only (`single_vendor`); a project may
  not set that downgrade itself. The report caps each desk at five findings and counts the rest.
  `vdt desks` runs the desks on demand; the gate and `vdt verdict --run` reach them through the
  same runner.
- **Phase 4 — `vdt generate`.** Emits the seven skill files, the Claude Code and Codex hook
  registrations, the effective policy and a manifest as a pure function of the policy. No
  timestamp or path enters the output; a test proves two runs are byte-identical.
- **Phase 4 — Cursor and Antigravity adapters.** Registrations in their documented file shapes
  (Cursor's flat `version: 1` file, Antigravity's named `verdict` group), tolerant payload parsing,
  a four-host parity test, and an honest real-host status in `docs/HOSTS.md`. Their payload field
  names are not yet verified against a live host.
- **Exports.** `vdt export --sarif` writes SARIF 2.1.0 with the proof tier and ledger record in
  each result's properties; `vdt export --packet` writes the acceptance document (claims, findings,
  hypotheses, decisions, overrides, what was not established, and the report as printed). Also a
  `route-call` regression rule: a call served at the baseline and served by nothing now blocks.

### Changed

- The built-in default policy no longer names another application's npm commands, web-project
  paths, release paths, plan file or a machine-specific verifier. Project-approved configuration
  supplies them; a missing command earns no execution proof, and missing blast classification is
  stated in the report and never lowers a lane.
- A check's tree state is stamped when the check starts, so an edit made while it runs makes the
  check stale instead of trusted.

### Fixed

- The checkers module documentation said six checkers; there are seven (wiring was missing).
- The hook-install cards on the session report page were a link only; each now has a button that
  installs the entries through the adapters and records the human's decision, with the link kept.
- A retried hooks install no longer leaves or duplicates a backup.
- MCP claim and done-when text is redacted before it reaches the append-only ledger.

### Not built yet

- Phase 1/4 real-host status for Cursor and Antigravity: their payload field names are unverified
  against a live host (`docs/HOSTS.md`).
- Phase 4 acceptance across two physical machines: `vdt generate` is deterministic in tests, but
  the cross-machine byte comparison has not been run (one machine available).
- Phase 5: enforce-mode send-backs, firstrun in a clean container, the release checkers (`firstrun`,
  `release`, `mutation`), the GUI installer, and the v1.0 release lane.
