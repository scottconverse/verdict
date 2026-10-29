# Candidate state — one page

Code head `08ec95c` (all code receipts and CI runs refer to it). Record commits after it: `890f423`,
`db34af3`, `d2ee4a3`, `1b02c8c`, `c2a52a4`, `49fca08`, `c06751a` and the commit that carries this
file (docs/records only). Branch `merge-trial`; worktree clean; nothing merged, tagged or published.

Two different counts, both true and not interchangeable:

* **Contract rows:** `49 of 56 requirements verified` (Auditor-verified statuses; the seven open rows
  are SPEC-8, SPEC-13, SPEC-15, F25, F27, F28, F29 — see *Open*).
* **Packet checks at this head:** `40 of 55 checks ran`, and its build/tests read **not run** because
  the repo's command set is not yet approved (see *What Scott can do next*). The release-lane exec
  evidence is incomplete for that reason, not because checks failed.

Requirement rows are the agreed contract; checks are what the ledger recorded at this head.

Packet verdict line: `VERDICT: FAILED  (lane: release, policy v1, builder: claude/unknown
[anthropic], verifier: none)`.

## Verified — evidence groups

(a) **Severity normalizer** — `crates/verdict-desks/src/bin/vdt-provider-codex.rs` `normalize_answer`
keeps `critical`; unknown stays `minor`; a Critical desk finding blocks the release lane.
Unit + consumer tests, red-before/green-after captured.

(b) **AppImage / FUSE packaging + checksums** — `release/package-appimage.sh` runs appimagetool under
`APPIMAGE_EXTRACT_AND_RUN=1`, no FUSE fallback; `release/package.sh` writes standard `*.sha256`.
CI (agent-provided): clean-install run `40804156470` at `08ec95c` — Ubuntu job `123218973466` built
and installed `Verdict-1.0.0-x86_64.AppImage` (checksum OK; hash not uploaded), macOS `123218973357`,
Windows `123218973526`.

(c) **Snapshot binding** — `crates/verdict-checkers/src/context.rs` `cache_matches_tree` (content
digest) + `cache_matches_changes` (diff fingerprint) bind the recorded stamp; torn reads record no
stamp and every requested checker `FailedToRun`; `read_asset` covers build assets only.

(d) **Provider containment** — `crates/verdict-desks/src/lib.rs`: tree kill before child kill (Windows
Job Object / `/T`, Unix process group), bounded stdin writer, immediate timeout return. Evidence is
split: `e47d4b0` (run `40795187701`) proves fixture lifetime + F11 usage handling; the bounded writer
first lands at `d89dd9c` (run `40796396150`, green ubuntu `123193787872` / macOS `123193787922` /
windows `123193787704`, agent-provided).

(e) **Observed-vs-claimed identity** — `apply_observation` keeps `model`/`served`/`usage` from CLI
events only, answer claims under `claimed`; effective invocation settings recorded. Served vendor is
`not_stated` (the CLI states no vendor).

(f) **Installer readiness / port separation / macOS re-open** — `crates/verdict-gui/src/setup.rs`
allocates the installed server its own free port, `/readyz` (app id + canonical project + spawned
pid) gates success; `release/package.sh` ships the macOS `vdt-launch` serve launcher reading
`~/.verdict/last-project`. Packaged retest recorded in `release/CANDIDATE-ARTIFACTS.txt`.

(g) **Hooks ownership / quoting / installed-core binding** — `crates/verdict-adapters/src/install.rs`
owns only `<vdt program> hook <host> <Event>` (via `program_is_vdt`), quotes any path, refuses
shell-expansion characters, strips Windows `\\?\`; the installed page writes the quoted absolute
core. Packaged proof: isolated project got
`"C:\…\install\vdt.exe" hook claude Stop`, no bare `vdt`, unrelated `node policy-check.js` hook kept,
Stop hook executed to `vdt 1.0.0`.

(h) **Dogfood gate** — `release/dogfood-gate.sh` exits 0 only on an anchored `VERDICT: CLEAR TO SHIP`,
otherwise prints FIX FIRST / NOT ESTABLISHED and exits 1; it never tags, releases or publishes.

(i) **Docs / roll-up** — `release/CANDIDATE-PACKET.txt`, `CANDIDATE-REPORT.txt`,
`CANDIDATE-ARTIFACTS.txt`, `CANDIDATE-RECONCILIATION.md`, `RELEASE-RUNBOOK.md`; three-OS `test` run
`40804156318` green at `08ec95c` (ubuntu `123218973230`, macOS `123218972950`, windows `123218973242`).

## Open

* **SPEC-8** — served-vendor enforcement is recording-level only (`not_stated`); a real
  cross-vendor served-vendor receipt needs Scott's acceptance run.
* **SPEC-13 / SPEC-15** — the tool's own `CLEAR TO SHIP` release has not happened; Scott decides when
  the gate's conditions are met.
* **F25** — native signed/notarized installers: needs signing credentials (external).
* **F27** — the dogfood gate is red as expected; it goes green only with a release-ready candidate
  and Scott's tag/publication decision.
* **F28** — visible desktop click-through (SmartScreen/UAC/Gatekeeper, double-click, visible Install
  and editor clicks, enforce/fail-open): Scott.
* **F29** — DSH loaded-host acceptance: Codex-owned; the plugin is optional and recording-only.

## What Scott can do next

1. **Approve the repo's commands** — the decision that turns the packet's `build/tests: not run` into
   real evidence. GUI: **http://127.0.0.1:4761/session/s-60679a9557** (pid `792`, port 4761) — press
   the approval card. CLI alternative: `target\release\vdt.exe approve-commands --i-am-the-human`.
2. **Look at the installed app** — **http://127.0.0.1:63341/** (pid `55884`; isolated project, not
   4717). Stop with `Stop-Process -Id 55884` / `Stop-Process -Id 792`.

After approval, re-run the release lane and re-export the packet (`release/RELEASE-RUNBOOK.md` steps
b–c) so exec/test evidence appears.

## Not claimed

Signing/notarization; publication; desktop click-through; the post-approval release lane; DSH
loaded-host acceptance; served-vendor enforcement beyond recording-level.
