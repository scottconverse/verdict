# Changelog

All notable changes to this project are recorded here. The format follows Keep a Changelog, and
the project uses semantic versioning from 1.0.0.

## [1.0.0] - 2026-10-27

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
  and a five-host parity test (Claude Code, Codex, Cursor, Antigravity, DeepSeek Harness), with an
  honest real-host status in `docs/HOSTS.md`. Their payload field names are not yet verified
  against a live host.
- **Phase 4 — DeepSeek Harness.** `vdt hook dsh <event>` accepts the host-neutral payload and
  records a session, and `vdt mcp` is the supported MCP connection; `vdt hooks install dsh` is
  refused because the harness owns its configuration (`docs/DSH.md`).
- **Phase 5 — enforce mode.** A human mode switch turns it on; a pre-edit hold stops an edit until
  a claim covers the file the prompt named, and the Stop hook sends the agent back with the finding
  list at most `gate.max_sendbacks` times (two by default), then releases it and records that the
  human must look. Shadow mode stays the default and everything fails open.
- **Phase 5 — mutation, firstrun and release checkers.** Mutation runs the configured runner (or
  reads its report) only when the policy names a reason, records survivors with pointers, and
  reports missing tooling as unavailable. firstrun runs the configured first-run commands and blocks
  a release dead end. release checks the configured tag on HEAD, version files, assets, README
  links and the project's own release checks. Each has a red fixture that must fail and a green one
  that must stay quiet.
- **Phase 5 — GUI and installer.** The page lists claims with the findings about each, the
  unplanned work, the decisions queue, mode, timed pause, commit-anyway and per-host hook install,
  and downloads the acceptance packet and SARIF. `release/install.ps1` and `release/install.sh`
  verify the checksum, install `vdt`, and open the page with no terminal step after.
- **Phase 5 — release lane.** Version 1.0.0, `verdict.json` with the project's own release block,
  a tag-triggered `release` workflow that builds the four target archives with SHA-256 files and
  attaches them, a clean-container install job and a two-machine byte comparison of `vdt generate`
  (`docs/RELEASE.md`, `release/RELEASE-NOTES.md`).
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

### Known limits

- Cursor, Antigravity and DeepSeek Harness have not been recorded against a live host in this
  release; their payload field names are documented but unverified, and an unreadable payload is a
  `check_unavailable`, never a clean result (`docs/HOSTS.md`).
- The two-machine byte comparison and the clean-container install run as CI jobs
  (`.github/workflows/clean-install-and-parity.yml`); their result belongs to the revision they ran
  on, not to a local run.
- The release archives are unsigned until Windows Authenticode and Apple notary certificates are
  supplied; the release notes say so.
- The release checker's tag test and the attachment of assets can only pass after the human
  creates the tag (`docs/RELEASE.md`).
- The Unix process-tree path is covered by CI and was not exercised by hand on Windows here.
