# Current evidence reconciliation — October 29, final candidate `300e9f4`

This section supersedes the historical status statements below. Scott requested
reconciliation against existing evidence; it does not erase earlier observations.
All 29 open major/blocker occurrences in the uncapped SARIF export are accounted
for. They are repeated historical observations, not 29 current product defects.

| Historical records | Current disposition and existing evidence |
| --- | --- |
| 267, 319, 378, 561 | Superseded: the actual AppImage is retained and checksummed in CANDIDATE-PROVENANCE.json. |
| 274, 325, 386, 483, 579 | Superseded: installed readiness identity and both rendered routes were observed; current GUI notice acceptance passed. Headless readback is not called a physical click. |
| 275, 327, 484, 580 | Superseded within the recorded scope: current six-job package/install/parity CI passed; exact four artifacts retained. Native macOS headless setup plus stub-based launcher decision checks are not Finder desktop proof. |
| 289, 344, 402, 499, 546, 595 | Historical overclaim corrected: F15 keeps all nine unsupported claims unproven. File existence remains static evidence. |
| 290, 345, 403, 498, 545, 596 | Historical mislabel corrected: record260 and the old session remain FAILED; neither is presented as a clean result. |
| 326 | Superseded: existing mutation records460,512,559 each report Ran, three killed mutants, zero survivors. No mutation test repeated; earlier failures retained. |
| 385 | Superseded by later exact-code CI and installed receipts; earlier failed/unclean exec attempts and killed lane26400 remain incomplete. |
| 268, 562 | Pending release action: v1.0.0 tag does not exist. This is not an implementation defect. |

These dispositions supersede 27 entries as current product blockers, with the
limits above. They do not alter their immutable ledger status. The only existing
human finding control records `AcceptFindingNotReal`; that would misdescribe real
historical failures, so no false-alarm decisions were appended. The 598-record
ledger and head remain byte-identical. Minor record435's old surviving-process
observation remains unresolved historically; no current survivor or product
failure is established by it.

Runtime evidence reused: [three-OS test run40930130637](https://github.com/scottconverse/verdict/actions/runs/40930130637)
and [six-job install/parity run40930130633](https://github.com/scottconverse/verdict/actions/runs/40930130633).
Windows Azure signing run40935832675 returned a Valid timestamped binary; the
actual signed archive installed successfully in isolated settings. All four
artifact hashes still match CANDIDATE-PROVENANCE.json. macOS unsigned distribution
and the optional served-identity side check follow Scott's explicit dispositions.

**Release authorization:** Scott explicitly authorized final merge/tag/publication of these
provenance-pinned artifacts with the historical verification record — reconciled against current evidence
and its evidence limits disclosed. No new current implementation defect is
established by this stale-finding reconciliation. The saved session still lacks
an accepted baseline/final claims and current ledger check inputs; external CI
receipts are not silently inserted as fresh ledger checks. Consequently this is
not a new CLEAR TO SHIP result, and the existing CLEAR-only gate is unchanged.
If Scott requires a fresh CLEAR instead of delivery with that disclosure, that
is still incomplete and cannot be obtained by relabeling these historical rows.

Private audit receipt: Documents/Codex/2026-10-28/verdict-direct-completion/
release-closeout-300e9f4/RECONCILIATION.json. No tests, desks, models, builds,
publication, settings changes or ledger mutations were performed for this reconciliation.

---
# Historical reconciliation at code head `b75010f`

The current direct-completion status is in `CANDIDATE-STATE.md`. Later changes include
acceptance tooling and an F11 served-identity enforcement repair, not just documents.
Older claims below remain history: 50/56 was the native board's count, installed
stream silence needed a separate captured check, and absent served identity cannot
count as independent verification. The missing retained AppImage was executable
artifact preparation, not a human publication decision; the CI retention fix at
`4fe42ff` now preserves the tested per-OS candidates. No failed or partial immutable
receipt has been rewritten or relabeled as passing.

Fresh full-claim session `s-60679a9557` in the same ledger (no ledger reset, nothing archived):
9 claim lines naming the files/symbols of the agreed acceptance, then the release lane, then
`vdt export --packet --lane release` and `vdt report`. Every item below is the current state, not a
completion claim.

## FULL-6-0 — original backlog, item by item

| Item | Current state |
| --- | --- |
| Correct checker-count documentation | README lists all ten registered checkers; no six-checker claim remains |
| Rewrite stale README/CHANGELOG | Rewritten; installer/checksum text matches the fixed layout (two-field outer checksums, inner `vdt.sha256` required). Packaging claims are now backed by the Windows archive built at this head and the Linux CI job |
| Install-hooks button and link | Present in the page; the installed click-through remains unestablished (F28) |
| Wire installer hooks end to end | Button calls the real merge/backup implementation; a real-host recording exists from an earlier session; full installed no-terminal workflow unestablished |
| Redact MCP claim text | Implemented on the claim/done-when paths through the shared redactor; not a proof that every unknown secret is recognizable |
| Route-call regressions | Source and fixture evidence retained; not re-run for this packet |
| Windows Job Object grandchild escape | Checker exec starts suspended, assigns the job, resumes; tests pass. F13 evidence split: `e47d4b0` (run 40795187701; ubuntu 123189835274, macOS 123189835674, windows 123189835842; agent-provided) proves fixture lifetime (parent-owned `Tmp`, helper marker written before the 1 s deadline, total run bound 3 s) and the F11 usage handling only — `git show e47d4b0` still has the unbounded `writer.join()`. The bounded stdin writer first lands at `d89dd9c`, CI run 40796396150, green on ubuntu 123193787872, macOS 123193787922 and windows 123193787704 (agent-provided). The 40.81 s red-before receipt is retained. |
| change_file/tree-stamp timing gap | Repaired: `cache_matches_tree` (content digest) and `cache_matches_changes` (diff fingerprint) both bind the stamp; torn reads record no stamp and every requested checker FailedToRun |
| Extra backup on retry | Retry cleanup/reuse implemented with existing test evidence; no contrary defect found |

**Limitation that must not be read as complete (F11):** served identity and effective settings are
recording-level only. Observed identity outranks answer-supplied claims (`apply_observation`), the
effective invocation settings are recorded, and answer claims are kept separately under `claimed` —
but the Codex CLI states a model and usage, not a vendor. `served_vendor` is therefore
`not_stated`, there is **no real cross-vendor served-vendor receipt**, and closing that row needs
the agreed real Claude-builder/Codex-verifier acceptance or an owner decision. It is not inferred
from the model name.

Design questions (13): unchanged from the historical reconciliation — hypothesis record, already-red
exec severity, tree-stamp (now repaired above), acceptance scope, `core.hooksPath`, unticked items as
Nit, `done_when` as a ticked condition, no dynamic endpoint field, Typer single-root heuristic,
no checker-to-checker calls, shared redaction, `host_session_id` sidecar, MCP cannot open a session.

Generic corrections (7): built-in exec commands empty; default plan/blast lists empty and absent
classification reported; machine-specific verifier removed (served identity gap as above);
command-provider interface without a required external skill; seven rubrics embedded; hosts optional
and the core standalone; isolated representative acceptance evidence retained, unsupported stacks
reported unverified.

## FULL-12-0 item 9 and items 1–8

9. **This artifact.** At head `d89dd9c` the candidate packet was regenerated from a fresh
   full-claim session with the real export command (`release/CANDIDATE-PACKET.txt`,
   `release/CANDIDATE-REPORT.txt`) and `release/CANDIDATE-ARTIFACTS.txt` was refreshed with the
   Windows binary/archive rebuilt locally at that head; earlier-head packets and hashes are retained
   as history, not replaced. The artifact record says plainly that the Linux archive predates this
   head and that the AppImage exists only as a CI-built artifact (run 40793472907, hash
   unavailable). Merge, tag, publication and signing remain Scott's decision or an external
   credential dependency. Candidate commits and pushes only.
1–8 (as previously stated): fresh verification now runs from Stop with the final gate record;
   snapshot binding and the mutation/browser/first-run evidence semantics were repaired; the
   command-approval card lists every command; the specified human decisions and stale citations;
   desk artifacts/proposals/dissent and served identity (limitation above); item 4 cites the split
   evidence — fixture lifetime and F11 usage at `e47d4b0` (run 40795187701), the bounded stdin
   writer at `d89dd9c` (run 40796396150, green on ubuntu 123193787872, macOS 123193787922 and windows 123193787704; agent-provided); generic
   release checks
   and packets; archive/install/checksum/shortcut/publish paths; DSH optional integration.

## VALIDATION — provenance

Before this record, provenance was **UNVERIFIED**: earlier archive hashes predated the head, no
AppImage was built locally, and nothing proved the packaged binaries came from the tested tree.
This record proves: the Windows archive and `target/release/vdt.exe` were rebuilt on this Windows
host from this tree at head `d89dd9c`, with their sizes and SHA-256 recorded, and that the same core
produced the candidate packet (earlier `1d60138` sizes/hashes are history, not current evidence). It
does **not** prove: that the pre-existing Linux `tar.gz` came from
this head, that the AppImage on this host exists (the AppImage is CI-built only, run 40793472907 at
head `92a72b9`, hash unavailable), that packaged behavior was exercised (installed acceptance),
or that anything was published. It is not signing evidence.

## WORK-ORDER

FIRST-SEVERITY and FIRST-APPIMAGE were closed with Auditor verification (A-001…A-008 dispositions
recorded). `HANDOFF.md`, `STARTER-PROMPT.md` and the original worker reports were read; their
backlog rows, thirteen design questions, seven generic corrections and five phase conditions are
reconciled above. The stdin-writer deadline and the Windows job-comment truthfulness cleanup have landed at `d89dd9c`; remaining executable work is this provenance/packet refresh at the current head; signing/notarization,
installed desktop click-through/enforce/fail-open, DSH deployment, and tag/publication are external
or the human's.

## One line each

* **SPEC-8 (desks):** rubrics, dispatch, cross-vendor refusal, hypothesis handling, proposals and
  receipts are implemented; served-vendor evidence is limited to `not_stated` (see F11), and the
  configured provider is the local Codex CLI.
* **SPEC-13 (testing the tool):** red/green fixtures per checker, golden reports, ledger invariants,
  cross-vendor refusal, commit-anyway, CI matrix and hook fail-open tests exist and ran this
  session; the tool's own release lane is not CLEAR, by design of the remaining blockers.
* **SPEC-15 (phasing):** phases 1–5 are implemented and exercised; phase 5's own CLEAR TO SHIP
  release has not happened and cannot be claimed.
* **F25 (installer delivery):** Windows archive and the AppImage step are real (Linux CI builds and
  installs the AppImage); native signed per-OS installers and macOS desktop click-through need
  credentials/hardware — external, not waived.
* **F27 (dogfood gate):** `release/dogfood-gate.sh` fails unless the packet's anchored verdict line
  is `CLEAR TO SHIP`; expected red on this candidate; it never tags, releases or publishes.
* **F28 (installed acceptance):** installer readiness now requires app id + canonical project +
  spawned pid via `/readyz`, and partial installs name what was completed; the physical desktop
  click-through and live enforce/fail-open acceptance remain unestablished.
* **F29 (optional DSH integration):** recording-only plugin plus MCP connection; a loaded DSH host
  acceptance is not established and is Codex-owned; the generic core does not depend on it.
* **SAFE-DELIVERY:** the final candidate packet and artifact record are refreshed at code head
  `b75010f` (earlier heads as history) before re-verification (`release/CANDIDATE-PACKET.txt`,
  `release/CANDIDATE-ARTIFACTS.txt`); signing/notarization, installed desktop acceptance, DSH
  deployment and tag/publication remain external; work confined to this `merge-trial` worktree;
  main, history, receipts and settings preserved; no evidence reset or weakened claim; candidate
  commits/pushes only; no merge, tag or publish.

## Artifacts in this record

* `release/CANDIDATE-PACKET.txt` — packet from session `s-60679a9557` (real export).
* `release/CANDIDATE-REPORT.txt` — the same computation as the plain report.
* `release/CANDIDATE-ARTIFACTS.txt` — head, artifact paths, sizes, SHA-256, provenance.
* `release/PACKET-BEFORE-TAG.txt`, `release/REPORT-BEFORE-TAG.txt` — retained history, unchanged.

Verdict line of this candidate: `VERDICT: FAILED  (lane: release, policy v1, builder: claude/unknown
[anthropic], verifier: none)`.

## Close-out at the commit carrying this record

Code head: the commit that carries this section (production hook-seam revert plus the ledger-polling
test). Earlier code heads `e02323f` and `36f47ef` with their CI receipts are history; the sections
above that name `08ec95c`/`d89dd9c` are history too, kept as written.

### A-011 — resolved by authorized delegation, not by a click

Ledger record `406` (`2026-10-29T02:17:21.459Z`, decision `run_project_commands`, choice
`approved:2dfe10c3…`, session `s-60679a9557`) was produced by **Codex submitting the real
`/decide` form on Scott's explicit instruction** ("Approve these commands"), not by a physical click
and not by the agent grading itself. Receipt:
`C:/Users/scott/Documents/Codex/2026-10-28/verdict-approval-page/APPROVAL-RECEIPT.json`
(recordedAt `2026-10-28T20:17:22.64-06:00`, actor "Codex submitted the real page form on Scott
explicit instruction; not a physical human click", `httpStatus: 200`, approval card and changed
commands no longer remaining). So the approval is an **authorized delegated human decision**. The
earlier quarantine is lifted and the exec-derived evidence is usable; no re-approval is requested and
record 406 is not to be withdrawn for this distinction.

### Windows flake and process residue — corrected

The hook test at `crates/verdict-cli/tests/hooks.rs` is now deterministic without a production
override: `record_budget` was removed, `fail()` uses the unchanged `UNAVAILABLE_BUDGET` (2 s), and the
test polls the ledger with a bounded deadline after the hook exits. The hook's exit bound is
unchanged; the assertions (silent exit 0, bounded, `check_unavailable` + "did not arrive" present)
are unchanged. CI at `36f47ef` was green on ubuntu `123237042745`, macOS `123237042698`, windows
`123237042514`, and packaging (`clean-install-and-parity` `40809765052`) was green at that head too.

Process-residue classification was wrong and is corrected here: PID 26400 was **Codex's own active
lane** (ownership in `C:/Users/scott/Documents/Codex/2026-10-28/verdict-approval-page/PROCESS.json`
plus ledger records 407–449), not an orphan survivor; I killed it to unlock builds, which was the
wrong action. Codex's approval page (pids 10692/59637) must not be touched. For records
414/418/420/423 (exec/firstrun/release checks that "exited with code 0 but left processes running
(they were stopped)"), the records carry no process identities, so the actual pids, parentage and
launch origin are **not established** — that residue remains unclassified, neither proven product
defect nor proven harness artefact, until identities are captured with parentage. Standing rule from
now on: never kill a `vdt` process that was not started here without checking its ownership records
first.

### F28 (packaged install at `08ec95c`) — what it established

From the packaged Windows artifact (zip `c7af8759…645ba`, inner core
`f8c3b18b0a183df39930291ca4e53284ddacff73c572234be57078296766ae63`): the wizard ran on port 4753
(not 4717) and completed the real install form; the installed server got its own free port (63341, pid
16584); `/readyz` returned app id + canonical project + matching pid; the installed core hash equals
the package's inner hash; the installed page's Install-hooks action wrote
`"C:\…\install\vdt.exe" hook claude <Event>` (quoted absolute core, no bare `vdt`, unrelated
`node policy-check.js hook claude Stop` preserved) and the Stop command executed verbatim through cmd
exits 0 resolving to `vdt 1.0.0`. A `\\?\` verbatim-path defect found in the first packaged run was
fixed at `08ec95c` (prefix stripped, unit-tested). Not established: signing/notarization, desktop
click-through, publication, DSH loaded acceptance, served-vendor enforcement beyond recording level
(`derived_vendor` is a labelled hint only; SPEC-8 still needs a real cross-vendor receipt or Scott's
acceptance).
## Final close-out at code head `b75010f`

Code head `b75010f` (record commits to follow carry docs/packet only). CI: `test` run `40814185027` **success** on all
three — ubuntu `123251292525`, macOS `123251292835`, windows `123251292807`; `clean-install-and-parity`
run `40814185033` **success** (macOS `123251292687`, windows archive `123251292839`, ubuntu
`123251292877` — whose AppImage step logged `built appdist/Verdict-1.0.0-x86_64.AppImage …`,
`Verdict-1.0.0-x86_64.AppImage: OK`, `Installed /tmp/app-home/bin/vdt` — the byte-identical compare is
`123251991750`).

A-012 resolution: `verdict-core` gained `Ledger::open_with_busy_timeout` /
`open_path_with_busy_timeout` (existing `open`/`open_path` keep their 5 s semantics); the adapter's
fail-open record path opens with `Duration::ZERO`, and `fail()` writes the record synchronously (no
detached writer that Windows can kill mid-write). Regressions: `a_locked_ledger_does_not_stop_the_agent_and_returns_inside_the_budget`
(writable-lock ⇒ fast return, no record) and the stdin-budget test (writable ledger ⇒ exactly one
`check_unavailable` + "did not arrive" record, silent exit 0, bounded) are both green, together with
the rest of both hook suites. The prior seam (`VDT_HOOK_RECORD_BUDGET_MS`) was removed; the product's
hook-exit budget is unchanged.

Residue wording and the historical 258/260 release-label finding stand as previously recorded: the
records carry no process identities (unidentifiable), and the historical designation cannot be
rewritten; the current release check reports its findings instead of calling the run clean.

Packet at this head: `VERDICT: FAILED  (lane: release, policy v1, builder: claude/unknown
[anthropic], verifier: none)`, `99 of 130 checks ran`, with exec, mutation and firstrun all having
run. Remaining blockers are the missing `dist/*.AppImage` asset, the absent `v1.0.0` tag, and the
desk findings about evidence labelling (records 289/344/402) plus the historical-clean designation
(records 290/345).
## Blockers at `b75010f`, with classification

Head and CI above are current (`b75010f`; test `40814185027` green on all three, clean-install
`40814185033` green). Of the five remaining blockers:

* **(c) release decisions for Scott:** the candidate `dist/*.AppImage` asset is missing, and HEAD has
  no `v1.0.0` tag.
* **(b) three desk judgment findings needing Scott's accept/supersede:** packaging/dogfood CONNECTED
  labels rest on file existence rather than execution (records 289/344/402); "release record 260
  designated clean despite two blockers" (records 290/345); and the first-run receipt does not
  exercise GUI readiness (records 274/325).
* **(a) no genuine product work outstanding:** the historical mutation-failure finding (record 326)
  is an earlier-run artifact that needs a human accept/supersede decision, not code. Residue from
  records 414/418/420/423 is unidentifiable (no pid/command/parent anywhere). External boundaries
  unchanged: signing/notarization, desktop click-through, publication, DSH loaded acceptance,
  served-vendor enforcement beyond recording.
## Reconciled receipts and the installed fail-open acceptance

Two earlier receipts are reconciled rather than relabelled: the **actual Install click and real
Claude Stop** were exercised in
`C:/Users/scott/Documents/Codex/2026-10-28/verdict-installed-acceptance` (the Codex-authored
INSTALL-RECEIPT.json / HOOK-PATH-FINDING.md there), and the three-role/DSH run is recorded in
`THREE-ROLE-STATE.md` under `C:/Users/scott/Documents/Codex/2026-10-27/dsh-web-incident`.
Those are evidence of exercised steps (**external to this worktree, so not machine-verifiable here**);
SmartScreen/UAC/Gatekeeper trust screens and the visible editor click
are **environment limitations noted, not outstanding requirements** of the agreed scope.

New, bounded installed acceptance against the current packaged core: `b75010f` package (zip
`9533eacf…3827`) was installed into a fresh isolated root
`C:\Users\scott\AppData\Local\Temp\vdt-installed-acc-20261028-235706`; installed core
`b1b32ce932dd017ab069c70c7c541d187023aab0a23d7d8ef746f477c410aa46` equals the package inner core; its
server ran on port 4771, pid 26456, `/readyz` app+canonical-project+pid verified, then stopped. The
fail-open path was exercised only: writable ledger ⇒ exit 0, 442 ms, exactly one
`check_unavailable`/“did not arrive” record; held write lock ⇒ exit 0, 1.897 s, no new record (fast
fail-open, verdict unverified). Full receipt: `release/INSTALLED-FAILOPEN-RECEIPT.md`.

Provenance correction: the earlier installed retest cited in `CANDIDATE-ARTIFACTS.txt` used the
`08ec95c` package's installed core `f8c3b18b…ae63`, **not** the current `b75010f` archive; the
artifact record now says so, and the current package install above is the first recorded install of
the `b75010f` build.
