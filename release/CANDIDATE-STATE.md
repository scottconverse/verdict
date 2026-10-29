# Candidate state — one page

Code head `b75010f` (the head this packet was regenerated at). Earlier heads `36f47ef`/`e02323f` and their records are history; record commits after `08ec95c` include `890f423`,
`db34af3`, `d2ee4a3`, `1b02c8c`, `c2a52a4`, `49fca08`, `c06751a` and the commit that carries this
file (docs/records only). Branch `merge-trial`; worktree clean; nothing merged, tagged or published.

Two different counts, both true and not interchangeable:

* **Contract rows:** `49 of 56 requirements verified` (Auditor-verified statuses). Open: SPEC-8,
  SPEC-13, SPEC-15, F25, F27, F28, F29, plus the four roll-up rows (FULL-12-0, VALIDATION,
  WORK-ORDER, SAFE-DELIVERY) pending the Auditor's close-out on this docs-only head.
* **Packet checks at this head:** `99 of 130 checks ran` at `b75010f`; build/tests, firstrun and
  mutation all ran under the Scott-authorized delegated approval (record 406 — Codex submitted the
  real `/decide` form on Scott's explicit instruction; see the reconciliation). No approval is
  outstanding, and the earlier "not run" line is history.

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

1. **Approval is already recorded** as an authorized delegated decision (Scott authorized it in
   Codex; Codex submitted the real `/decide` form; ledger record 406) — no further approval is needed.
2. **Look at the installed app** — **http://127.0.0.1:63341/** (an isolated project, not 4717). The
   approval page is **http://127.0.0.1:4761/session/s-60679a9557**. Stop whichever is running with
   `Stop-Process -Id <pid>` after checking its `PROCESS.json`/ownership; do not kill unknown vdt processes.

The release lane has already run at this head with exec, mutation and firstrun executed under that
approval; the packet is `release/CANDIDATE-PACKET.txt` (`VERDICT: FAILED`, 99 of 130 checks ran). No
re-run is needed before the next decision — only Scott's release decisions remain.

## Not claimed

Signing/notarization; publication; desktop click-through; the post-approval release lane; DSH
loaded-host acceptance; served-vendor enforcement beyond recording-level.
