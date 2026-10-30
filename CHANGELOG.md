# Changelog

All notable changes to this project are recorded here. The format follows Keep a Changelog, and
the project uses semantic versioning from 1.0.0.

## [1.1.0] - 2026-10-30

Five security fixes from the post-release review, and a packaging fix.

### Security

- **Desk provider programs need the human's command approval.** The `vendors.providers` settings
  (program, arguments, model, timeout) are now part of the approved command set and are shown on
  the approval card. A provider that starts a program the human has not approved is recorded as
  `check_unavailable` and is never started.
- **A rule change during a session needs the human's acceptance.** A session records the policy
  hash it started with. If `verdict.json` or the global policy changes mid-session, the verdict can
  no longer read Verified or Clear to ship until the human runs
  `vdt accept-policy --i-am-the-human` or restores the old rules.
- **The local page answers only the browser that opened its one-time link.** `vdt serve` prints a
  one-time link; opening it sets an HttpOnly cookie. Every request except `/readyz` needs that
  cookie, so a program that only knows the port cannot read the form token or press a button. The
  installers, shortcuts and setup wizard open the page with `serve --open`.
- **A human decision on the command line needs a person who types `yes`.** `--i-am-the-human` alone
  is no longer enough for hooks, the gate, `mcp-install`, `approve-commands`, `accept-policy` and
  `commit-anyway`. With no terminal on standard input (an agent's shell tool) they refuse and
  change nothing.
- **The ledger is sealed with a key outside the project.** Each write stores a keyed blake3 hash of
  the new head, made with `~/.verdict/seal.key`. `vdt ledger verify` reads BROKEN for a rewritten
  ledger, a dropped seal or a row added with raw SQL. A ledger from before 1.1.0 reads OK with a
  "not sealed yet" note and is sealed by its next write. A ledger copied to another user or
  machine reads BROKEN there. A program running as the same user can still read the key.

### Fixed

- The Linux AppImage build ships a PNG icon and `.DirIcon`, which the current `appimagetool`
  requires.

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
  and downloads the acceptance packet and SARIF. The Windows installer is a real window (project
  folder, MCP registration, shortcut, open the page) with a silent path for tests; the POSIX
  installer does the same for the current folder. Both verify the checksum and leave no terminal
  step after.
- **Phase 5 — MCP registration.** `vdt mcp-install <host>` registers the Verdict MCP server with
  Claude Code and Cursor (project `.mcp.json` / `.cursor/mcp.json`, merged and backed up), delegates
  Codex to `codex mcp add` in an isolated `CODEX_HOME`, and prints the exact DSH entry without
  editing the harness. It is human-gated and records an `mcp_install` decision.
- **Phase 5 — DeepSeek Harness native plugin.** `integrations/dsh/index.mjs` maps the harness's
  `session/created`, `user/message`, `assistant/message`, `turn/end` and `tools/result` events to
  the four hook payloads; `node --test integrations/dsh/test/plugin.test.mjs` proves it, and a live run against the real
  `vdt` recorded an intact ledger. The harness configuration is not changed by Verdict.
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

- The checkers module documentation said six checkers; the registry has ten (wiring, baseline_diff,
  regressions, tests_weakened, unfinished, exec, mutation, firstrun, release, claims_vs_code).
- The hook-install cards on the session report page were a link only; each now has a button that
  installs the entries through the adapters and records the human's decision, with the link kept.

### Fixed in the completion pass (audit findings F01–F30 and the original backlog)

- **Fresh verification on the ordinary completion path.** The Stop hook and MCP `verdict_check` now
  run the lane's checkers and desks before computing the verdict, and both persist the general
  gate-result record (F01, F14). The Stop hook's budget is the policy's `gate.stop_timeout`; reading
  the payload is bounded too, so a host that never closes stdin cannot hang the agent.
- **A lane's desks are part of its verdict.** A standard-or-higher lane with a missing desk receipt
  is PARTIAL, never clean; a desk receipt on other code is stale like any check (F02).
- **Receipts are bound to the snapshot read.** The tree stamp is taken before the context caches
  the tree, so a stamp can never be newer than the data a checker inspected (F03).
- **First-run and browser evidence.** A web first run is `browsed` only with a real server start, a
  page load and a configured browser screenshot whose hash is recorded; a project declares
  `firstrun.web: false` for a CLI or library, and only then does `ran` satisfy the release bar. A
  `browsed` exec command without its screenshot earns `ran` and reads as not established (F04–F06).
- **Consent shows every command.** The approval card lists the exec commands, the mutation runner,
  the first-run workflow and the release checks, all of which the approval hash covers (F07).
- **Mutation cannot fake proof.** Not-required records `skipped_by_policy` with `unverified`; the
  report is parsed strictly (an array of objects, numeric counts, a consistent total), a failing
  command is not read as a report, documented success exit codes are configurable, and seeds are
  kept in the finding (F08–F10).
- **Provider identity, order and process I/O.** Receipts record the served vendor/model when the
  provider states them, effective settings and usage; a builder-vendor answer on a cross-vendor run
  is refused; `vendors.verifier_order` is followed; stdout, stderr, stdin and the wait run
  concurrently with a bounded drain, so a chatty provider cannot deadlock the parent (F11–F13).
- **Packet and exports.** Every claim is classified proven / failed / unproven / out of scope with
  its evidence; a release packet carries the release and first-run records and every configured
  asset's SHA-256; `vdt export --decisions` writes `decisions.jsonl` with the verdict in force at
  each decision and the override's tree and findings (F15, F16).
- **Policy semantics.** `gate.stop_timeout`, per-lane `bar`, `exec.shell`, the suite block and the
  new first-run/mutation/release fields are implemented and in `policy/schema.json` (F17, B01).
- **GUI.** The page can run the checkers or the desks, choose the plan file, withdraw a prompt,
  waive words and register the MCP server, and it marks stale evidence; every action writes a human
  decision (F18, F19).
- **Desks.** Each desk gets the artifacts the design assigns it (wiring and exec for Engineer, the
  README and release findings for Docs, exec/firstrun for QA, firstrun and wiring for UX,
  regressions for Security, manifests and licenses for Legal, clean results to refute for the
  Refuter); proposals are recorded and may be run with `desks.run_proposals`; disagreeing desk
  findings are shown as DESK DISSENT rather than averaged (F20).
- **Packaging.** One `release/package.sh` produces the archive layout the installers and CI both
  consume; the binary's inner SHA-256 is shipped and a missing checksum refuses the install; the
  Windows shortcut keeps the selected project (`serve --project`); the publish job has its checkout
  and creates a draft release (F21–F24).
- **Release checks.** The project's own checks run first; the version is read from the manifest's
  package section; product name and lockfile pin, asset hashes, the saved published-release record,
  README commands and suite-wide pins are checked (F26).
- **Docs.** README, RELEASE, HOSTS, DSH and the decisions log now state the actual behaviour and
  the remaining limits (F30).
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
