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
- **Windows Job Object: a grandchild can escape the exec kill.** Not fixed. A child started in the
  first instant after `CreateProcess` can spawn before the job assignment. A complete fix needs a
  suspended start (custom `CreateProcessW` with `CREATE_SUSPENDED`, job assignment, then
  `ResumeThread`), which std's `Command` does not expose. Recorded as a remaining platform risk,
  not claimed fixed. Undo: none.
