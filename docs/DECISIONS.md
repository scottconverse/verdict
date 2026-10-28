# Decisions the worker reports left open

Every question a phase report listed, with the ruling and why. A ruling is a design decision, not a
claim that the work is done; the implementation notes say where the code follows it. Undo lines
name the smallest reversible change.

## 1. Is `hypothesis` its own record kind?

**Ruling: yes, keep `hypothesis` as a record kind (as built).**

The design says a finding without an evidence pointer "is stored as a hypothesis, not a finding".
A separate kind makes that sentence checkable in the ledger and in every reader, and keeps
`Finding` counts honest. `Ledger::append` still normalizes a `Finding` with no pointer into a
`Hypothesis`. Undo: none needed; no behavior depends on a flag.

## 2. An already-red exec result: Major or Blocker?

**Ruling: Major, and it never blocks a commit; `failed-baseline-unknown` stays a Blocker.**

An already-red command was red before the session touched anything, so it is not a regression the
builder caused. It still forces NEEDS YOU and blocks CLEAR TO SHIP until someone fixes or accepts
it. When the baseline re-run could not establish the same conditions (missing installed
dependencies, a timeout, a dirty baseline), the answer is unknown, not already-red, and that stays
a Blocker. Undo: `crates/verdict-checkers/src/exec.rs`, the `already-red` severity.

## 3. When is a check's tree state stamped?

**Ruling: when the check starts.**

The check proves something about the code it read. Stamping at the end would let an edit made
while the check ran be stamped as "seen". The runner now takes the stamp before `checker.run` and
stores that; the verdict compares it with the tree now, so an edit during a check makes the check
stale (PARTIAL), never trusted. Undo: `run_lane_with_home` / `append_check` in
`crates/verdict-checkers/src/lib.rs`.

## 4. What is the scope of an acceptance?

**Ruling: an exact acceptance is project-wide; an old id-only acceptance stays in its own session
and closes only occurrences recorded before it.**

This is what fix 2b built: `AcceptChoice` names the finding record (and its evidence identity), so
accepting one occurrence never silences a later, different occurrence with the same id. Undo:
`verdict.rs::closes` / `Accept::applies`.

## 5. `core.hooksPath`

**Ruling: supported already; no new code.**

`githook::hooks_dir` asks `git rev-parse --git-path hooks`, which honors `core.hooksPath`,
worktrees and a symlinked hooks folder. A test now proves a configured `core.hooksPath` is where
the hook is written and that `.git/hooks` is left alone. Undo: none.

## 6. Unticked plan items: a Nit finding, or no finding?

**Ruling: a Nit finding, listed but never blocking.**

The item is real work the plan names and the code does not have. It belongs in the report so the
human sees it, but the plan did not tick it, so it is not a claim that the work is done. Undo:
`wiring.rs`, `is_ticked` / the severity choice.

## 7. Does a `done_when` block?

**Ruling: yes, a done-when counts as ticked.**

It is the agent's own statement of a checkable success condition; the builder may not grade it,
and the checker may. It is recorded before the work, but a verdict is computed when work is judged
(Stop, gate, `vdt verdict`), not on every edit, so it does not make mid-session noise. Undo:
`wiring.rs::is_ticked` (remove `ClaimSource::DoneWhen`).

## 8. A `dynamic` flag on `Endpoint`

**Ruling: no new field in this release.**

An endpoint whose full path cannot be read already carries `router: Some(...)` and a `{}` segment
for an unreadable part; readers must treat both as unknown. A second flag would duplicate that
convention, and the extract crate's model is a contract other phases build on. Documented in
`verdict-extract/src/model.rs`. Undo: none.

## 9. Typer entry point

**Ruling: keep the single-unmounted-root heuristic; do not read `pyproject.toml`.**

With one unmounted root the root is the app; with several, none is guessed and the commands stay
unresolved (unverified, never clean). Reading build metadata is a new extractor input, not a
correctness fix. Undo: `py/typer.rs`.

## 10. Should `unfinished` read the wiring facts?

**Ruling: no.**

Checkers never call each other; the core orders them. The disagreement in the hand run (unfinished
saying "no caller found by name" while wiring found `invoke("load_settings")`) is a naming
limitation recorded in the check detail, not a verdict error: the wiring checker owns the call
graph. Undo: none.

## 11. Redaction of commands and claim text

**Ruling: redact on every path that writes free text to the ledger.**

The hook path already redacted prompts, final messages and shell targets. The MCP claim and
done-when paths now redact too (`verdict-adapters::redact`). The ledger is append-only, so this is
the only place a secret can still be kept out. Undo: the two `redact` calls in
`verdict-mcp/src/lib.rs`.

## 12. `host_session_id` in the ledger

**Ruling: no new field; the `.verdict/hosts/` sidecar keeps the mapping.**

The ledger is a record of what Verdict concluded, not a copy of the host transcript. The sidecar
maps the host's raw session and turn ids to ledger sessions and prompt ordinals, and it is the
right place because host ids can change format between host releases. Undo: none.

## 13. May the MCP server open a session?

**Ruling: no.**

Sessions are opened by an adapter or the core's own commands. An MCP-primary host gets an adapter
(DSH's plugin is the planned one); until then `verdict_claim` refuses with NO_SESSION and says why.
Letting an agent open a session would let it choose the baseline, which is the human's and the
adapter's to set. Undo: `verdict-mcp` `claim`/`done_when` session lookup.

## Backlog items fixed with their reports

- **1c installer hooks not wired end to end.** The page's Install button now performs the adapters'
  merge (project `.claude/settings.json` for Claude Code, isolated `CODEX_HOME`/`~/.codex/hooks.json`
  for Codex), records the human decision only after the write succeeded, and still shows the block.
  Undo: `verdict-gui/src/lib.rs::install_hooks`.
- **MCP claim text not redacted.** Fixed (decision 11).
- **The regressions checker's route-call part.** Implemented: a call whose target was served at
  the baseline and is served by nothing now is a Blocker `route-call`, with a file:line pointer.
  The fixture pair `fixtures/regressions` covers red and green. Undo: `regressions.rs`.
- **change_file timing gap.** Fixed (decision 3).
- **Extra backup file left on retry.** A retry that abandons a backup it just made removes it, and
  an existing backup holding identical bytes is reused instead of duplicated. Undo:
  `verdict-adapters/src/install.rs::backup` / `change_file`.
- **Windows Job Object: a grandchild can escape the exec kill.** **Fixed.** The command is created
  with `CREATE_SUSPENDED`, assigned to the job object, and only then has its first thread resumed,
  so no descendant can exist outside the job. If the resume fails the process is terminated and the
  check records the tree error rather than running an uncontrolled command. A test that leaves a
  process writing a file after the command ends proves the stray is still killed
  (`a_stray_process_is_really_killed`). Undo: `exec.rs`, `build_command` / `Tree::attach` /
  `resume_first_thread`.

## 14. The five-finding cap: at dispatch or at the report?

**Ruling: all findings are written to the ledger; the report shows five per desk and counts the
rest on one line.**

The cap is about what a person can read in a report body, not about how much a desk may record.
Dropping findings at dispatch would hide evidence from every later reader; the ledger keeps all of
them and `vdt report` says `(+N more from the <desk> desk, in the ledger)`. Undo:
`report.rs::cap_desks`.

## 15. Do desk findings block?

**Ruling: keep the existing rule: a desk finding does not block by severity at the standard or high
lane; at release, any open finding above minor does.**

A desk's judgment is a lead, not a deterministic contradiction. It forces NEEDS YOU, and the
Refuter's finding still fails a release. This is the same rule part 2b shipped, now exercised by
real desk findings. Undo: `verdict.rs::is_blocking`.

## 16. What is a provider, and does a routing skill have to exist?

**Ruling: a provider is a command speaking one JSON request/answer protocol. Any gateway, skill or
model wrapper can be one; none is required.**

This removes the mandatory external routing-skill dependency the brief calls out. The receipt names
the configured vendor and model and the model the answer says served it; the cross-vendor rule reads
the vendor, never the host app or machine. Undo: `verdict-desks/src/lib.rs`, `CommandProvider`.

## 17. Where may `single_vendor` be set?

**Ruling: in the global policy only. A project that sets it is refused, and the refusal is
recorded.**

The design says the downgrade can be changed "in the global policy, never per project". A project
that tries is refused before any desk runs, with one `check_unavailable` per required desk, so the
verdict cannot read clean. Undo: `verdict-desks::run`, the `PolicySource::Project` check.

## 18. What happens when desks are refused or fail?

**Ruling: each required desk gets a `check_unavailable` record; a provider's failure is a
`failed_to_run` receipt; no finding is invented.**

A refusal above the cross-vendor lane leaves no verdict that can read clean. A desk whose answer is
not the protocol is a failed receipt with the provider's own error. Undo: `verdict-desks::run`.

## 19. Why does a real Claude Code session record `model: unknown`?

**Ruling: record `unknown`, because the host does not send the model.**

A live Claude Code 2.1.296 SessionStart payload carries only `session_id`, `transcript_path`, `cwd`,
`hook_event_name` and `source`; none of the five events carries `model`. The vendor is recorded as
`anthropic` from the host. Inventing a model from the CLI's name would make the ledger lie about
what ran. Undo: none.


## 20. When does enforce mode hold an edit?

**Ruling: only an edit to a file the latest prompt named, and only while no claim covers it.**

The design's pre-edit hold is a speed bump, not a lock: a claim through `verdict_claim` releases
it, and a file the prompt did not name is never held. The hook stays fail-open: an error, a timeout
or a missing ledger records `check_unavailable` and the edit proceeds. Undo:
`verdict-adapters::recorder`, the `PreToolUse` arm.

## 21. How many send-backs, and what happens after the last one?

**Ruling: `gate.max_sendbacks` (two by default). After the last, the agent is released and a core
`check` record tells the human; the verdict stays FAILED.**

A send-back carries the report, so the agent sees the finding and its pointer. The limit is in the
host-session state file; losing it at worst allows one extra send-back. No send-back is invented in
shadow mode. Undo: `recorder.rs`, the `Stop` arm.

## 22. What is a mutation runner's protocol?

**Ruling: one JSON object — `{"survivors": [...], "killed": N, "total": N}` — from the configured
command's stdout, or from the configured report file.**

Verdict does not parse a tool's human output, and it does not guess a tool's exit-code meaning. A
runner that prints nothing parsable is unavailable; missing tooling is unavailable; a survivor is a
Major finding with a pointer when it names a file. Undo: `checkers::mutation`.

## 23. What does firstrun prove?

**Ruling: that the project's configured first-run commands run and pass, recorded with their
output. The clean-container walk is a release activity, run in CI.**

Commands needing a server Verdict does not start are marked not established, not skipped quietly.
Undo: `checkers::firstrun`.

## 24. What is release coherence?

**Ruling: the configured tag on HEAD, the configured version in the configured files, the
configured assets present, the README's relative links resolving, and the project's own release
checks passing.**

Nothing is assumed: an empty `release` block is unavailable, never clean. The tag check cannot pass
before the human creates the tag, which is the point of a release lane. Undo:
`checkers::release`.

## 25. What does the DSH integration include?

**Ruling: the MCP connection and the native plugin are both shipped; the plugin is recording-only.**

`integrations/dsh/index.mjs` is a Cordis plugin (`export const name`, `export function apply(ctx)`)
that subscribes to the harness events the installed packages already use and turns them into
`vdt hook dsh <event>` payloads. It records the session, the user's message, tool results and the
turn end; it never holds an edit and cannot apply a Stop send-back, because this build does not
claim DSH's reply shape (the same limit as Cursor and Antigravity). `docs/DSH.md` states the limits,
and the integration is optional: no part of the Rust core or the deterministic checks needs it.

The earlier ruling said no native plugin was claimed. That was true when it was written and is no
longer: the plugin exists, its tests use the harness's recorded event shapes, and it is packaged in
the release archive. What is still not done is loading it into this workstation's preserved DSH
configuration. Undo: delete `integrations/dsh/` and its `docs/DSH.md` section; the generic product
is unaffected.

## 26. Installer delivery: wizard reuse (approved 2026-10-28)

**Ruling: the localhost GUI is the install wizard; each OS gets a double-clickable package that
launches it.** The specification's architecture table names Tauri/cargo-dist. The owner approved
replacing that implementation approach while keeping every functional and acceptance requirement:
the package must install the core, register the selected hosts with the exact configuration, open
the chosen project's page, and work without a terminal. Windows ships `Verdict-Setup.exe`, macOS
`Verdict.app`, Linux an AppImage; the setup program runs the same checksummed core and calls the
same `mcp_command` registration path as `vdt mcp-install`. Signing is not waived: unsigned packages
are a delivery state, and signed/notarized packages still need the certificates.

Undo: remove the setup wizard and the per-OS wrappers, and return to the Tauri/cargo-dist plan.
