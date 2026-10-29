# Candidate state — direct completion, October 29

This section supersedes the historical snapshot below. Codex owns the remaining
work directly; the triad's existing goal and evidence are preserved. The prior
50/56 roll-up is a worker claim, not a completed independent requirement audit.

* Acceptance tooling and tested-package retention landed at `4fe42ff`.
  Clean-install/parity run `40833377175` passed all six jobs. Unlike earlier runs,
  it retained the tested Linux archive/AppImage, Windows archive and macOS archive
  with their checksums as commit-named artifacts. They are unsigned candidates.
* A real, expressly authorized Codex engineer desk ran on an isolated copy of the
  existing Anthropic session `s-60679a9557`. All 598 prior records were preserved;
  receipt 605 records zero findings and six hypotheses. Requested model was
  `gpt-6.1-sol`; observed usage was 35,857 input and 477 output tokens. The CLI
  returned neither a model nor a served vendor. This establishes transport and
  receipt execution, **not verified served-vendor independence**.
* That run exposed the remaining F11 enforcement gap. This revision refuses
  required cross-vendor evidence when served identity is absent, blank, unknown,
  or the builder's vendor. It records unavailable/failed evidence rather than
  counting the answer's findings. The existing human-owned global warning policy
  remains available and explicitly sets `single_vendor`. Requested settings and
  model-family hints never become confirmed identity.
* The F11 regression failed before the repair (`ran` versus expected `failed`).
  Candidate `da55d1f` passed required three-platform test run `40834780675` and
  package/install/parity run `40834780544`. Its Windows package was silently
  installed in an isolated directory. The installed core rejected absent and
  same-vendor synthetic responses and accepted the independently identified
  fixture. All three copied ledgers retained the original 598-record prefix;
  original DB/head hashes were unchanged. This is synthetic installed proof,
  not a real served-vendor receipt. Core SHA-256:
  `b0a7a903a27e2d10034dc89231880d2405e9d3cb8cc9c4451e6f3c3ac6580ccc`.
* Exporting that candidate exposed an F15 defect: any successful aggregate check
  labelled all individual claims proven, including after the source changed.
  This revision removes that unsupported inference. A claim without positive
  evidence bound to it remains unproven; a finding can still classify it failed.
  Existing check receipts retain their own tiers and results. The affected test
  reproduces the overclaim before the fix and passes for all five aggregate
  check types on unchanged and changed trees. All six export tests pass. This
  production change requires its own CI and current package before delivery.
* Approval 406 remains valid: Scott authorized Codex to submit the real page form.
  No repeat command approval is required. The retired ports 4761 and 63341 must
  not be advertised as running pages.
* Installed fail-open evidence now explicitly captures empty stdout/stderr:
  writable 451 ms and locked 1,904 ms, both exit 0, on installed core
  `b1b32ce932dd017ab069c70c7c541d187023aab0a23d7d8ef746f477c410aa46`.
  A byte-identical copy retained the original record and added one unavailable
  record; original DB/head hashes stayed unchanged. This is installed synthetic
  proof. That fixture has no session; no session-level UNVERIFIED result was seen.
* Preserve the actual earlier installer Install click, real Claude two-send-back
  transcript and real VS Code webview action. They prove their stated behaviors;
  the Claude enforce run used a debug binary and manually configured hooks.
  They are not a new end-to-end packaged-host run. SmartScreen/UAC/Gatekeeper
  exercises are not added requirements.

Remaining: resolve usable served-identity evidence, complete applicable installed
acceptance and reconcile current candidate artifacts/packet. Signing/notarization,
unavailable external desktop acceptance, safe loaded-host DSH acceptance, and final
human merge/tag/publication/post-tag CLEAR remain explicitly incomplete. The
historical partial/killed lane and unclassified process-survivor receipts are not
converted into passes. No further broad/model lane was repeated.

## Historical native snapshot (retained for provenance, superseded above)

Code head `b75010f` (the head this packet was regenerated at). Earlier heads `36f47ef`/`e02323f` and their records are history; record commits after `08ec95c` include `890f423`,
`db34af3`, `d2ee4a3`, `1b02c8c`, `c2a52a4`, `49fca08`, `c06751a` and the commit that carries this
file (docs/records only). Branch `merge-trial`; worktree clean; nothing merged, tagged or published.

Two different counts, both true and not interchangeable:

* **Contract rows:** `50 of 56 requirements verified` (Auditor-verified; FULL-12-0, VALIDATION,
  WORK-ORDER, SAFE-DELIVERY and F28 are closed). Open: SPEC-8, SPEC-13, SPEC-15, F25, F27, F29.
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
CI (agent-provided) at `b75010f`: test run `40814185027` green on ubuntu `123251292525` / macOS `123251292835` / windows `123251292807`; clean-install run `40814185033` green — Ubuntu job `123251292877` built
and installed `Verdict-1.0.0-x86_64.AppImage` (checksum OK; hash not uploaded), macOS `123251292687`,
Windows archive `123251292839`, byte-identical compare `123251991750`.

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
`CANDIDATE-ARTIFACTS.txt`, `CANDIDATE-RECONCILIATION.md`, `RELEASE-RUNBOOK.md`,
`INSTALLED-FAILOPEN-RECEIPT.md`. Frozen-head CI: `test` run `40814185027` green at `b75010f`
(ubuntu `123251292525`, macOS `123251292835`, windows `123251292807`) and `clean-install-and-parity`
`40814185033` green (AppImage job `123251292877`, hash unavailable; macOS `123251292687`, Windows
archive `123251292839`, compare `123251991750`). History only: `test` `40804156318` at `08ec95c`
(ubuntu `123218973230`, macOS `123218972950`, windows `123218973242`).

## Open

* **SPEC-8** — served-vendor enforcement is recording-level only (`not_stated`); a real
  cross-vendor served-vendor receipt needs Scott's acceptance run.
* **SPEC-13 / SPEC-15** — the tool's own `CLEAR TO SHIP` release has not happened; Scott decides when
  the gate's conditions are met.
* **F25** — native signed/notarized installers: needs signing credentials (external).
* **F27** — the dogfood gate is red as expected; it goes green only with a release-ready candidate
  and Scott's tag/publication decision.
* **F28** — historical roll-up was too broad; use the evidence distinctions above.
* **F29** — DSH loaded-host acceptance: Codex-owned; the plugin is optional and recording-only.

## Historical approval status

1. **Approval is already recorded** as an authorized delegated decision (Scott authorized it in
   Codex; Codex submitted the real `/decide` form; ledger record 406) — no further approval is needed.
2. The earlier temporary page endpoints have ended. No live URL is asserted here.

The release lane has already run at this head with exec, mutation and firstrun executed under that
approval; the packet is `release/CANDIDATE-PACKET.txt` (`VERDICT: FAILED`, 99 of 130 checks ran). No
automatic broad re-run is warranted. Executable acceptance gaps are listed above;
they must not be collapsed into Scott's final release decision.

## Not claimed

Signing/notarization; publication; desktop click-through; the post-approval release lane; DSH
loaded-host acceptance; served-vendor enforcement beyond recording-level.
