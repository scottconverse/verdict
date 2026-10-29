# Candidate reconciliation at the frozen head `1d60138`

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
| Windows Job Object grandchild escape | Checker exec starts suspended, assigns the job, resumes; tests pass. The desk provider path now has its own tree kill (Job Object / `/T`; Unix process group) with bounded timeout and tests |
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

9. **This artifact.** The candidate packet was regenerated from a fresh full-claim session at the
   frozen head with the real export command; the accumulated packet `release/PACKET-BEFORE-TAG.txt`
   is retained as history, not replaced. `release/CANDIDATE-ARTIFACTS.txt` records provenance
   honestly, including that the AppImage does not exist here and the Linux archive predates the
   head. Merge, tag, publication and signing remain Scott's decision or an external credential
   dependency. Candidate commits and pushes only.
1–8 (as previously stated): fresh verification now runs from Stop with the final gate record;
   snapshot binding and the mutation/browser/first-run evidence semantics were repaired; the
   command-approval card lists every command; the specified human decisions and stale citations;
   desk artifacts/proposals/dissent and served identity (limitation above); generic release checks
   and packets; archive/install/checksum/shortcut/publish paths; DSH optional integration.

## VALIDATION — provenance

Before this record, provenance was **UNVERIFIED**: earlier archive hashes predated the head, no
AppImage was built locally, and nothing proved the packaged binaries came from the tested tree.
This record proves: the Windows archive and `target/release/vdt.exe` were built on this Windows host
from this tree at head `1d60138`, with their sizes and SHA-256 recorded, and that the same core
produced the candidate packet. It does **not** prove: that the pre-existing Linux `tar.gz` came from
this head, that any AppImage exists, that packaged behavior was exercised (installed acceptance),
or that anything was published. It is not signing evidence.

## WORK-ORDER

FIRST-SEVERITY and FIRST-APPIMAGE were closed with Auditor verification (A-001…A-008 dispositions
recorded). `HANDOFF.md`, `STARTER-PROMPT.md` and the original worker reports were read; their
backlog rows, thirteen design questions, seven generic corrections and five phase conditions are
reconciled above. The remaining executable work is this packet and provenance; signing/notarization,
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
* **SAFE-DELIVERY:** work confined to this `merge-trial` worktree; main, history, receipts and
  settings preserved; no evidence reset or weakened claim; candidate commits/pushes only; no merge,
  tag or publish.

## Artifacts in this record

* `release/CANDIDATE-PACKET.txt` — packet from session `s-60679a9557` (real export).
* `release/CANDIDATE-REPORT.txt` — the same computation as the plain report.
* `release/CANDIDATE-ARTIFACTS.txt` — head, artifact paths, sizes, SHA-256, provenance.
* `release/PACKET-BEFORE-TAG.txt`, `release/REPORT-BEFORE-TAG.txt` — retained history, unchanged.

Verdict line of this candidate: `VERDICT: FAILED  (lane: release, policy v1, builder: claude/unknown
[anthropic], verifier: none)`.
