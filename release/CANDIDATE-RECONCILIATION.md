# Candidate reconciliation at the frozen head `08ec95c` (earlier heads are history)

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

9. **This artifact.** At head `08ec95c` the candidate packet was regenerated from a fresh
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
host from this tree at head `08ec95c`, with their sizes and SHA-256 recorded, and that the same core
produced the candidate packet (earlier `1d60138` sizes/hashes are history, not current evidence). It
does **not** prove: that the pre-existing Linux `tar.gz` came from
this head, that the AppImage on this host exists (the AppImage is CI-built only, run 40793472907 at
head `92a72b9`, hash unavailable), that packaged behavior was exercised (installed acceptance),
or that anything was published. It is not signing evidence.

## WORK-ORDER

FIRST-SEVERITY and FIRST-APPIMAGE were closed with Auditor verification (A-001…A-009 dispositions
recorded). `HANDOFF.md`, `STARTER-PROMPT.md` and the original worker reports were read; their
backlog rows, thirteen design questions, seven generic corrections and five phase conditions are
reconciled above. The stdin-writer deadline, the Windows job-comment truthfulness cleanup, the F28
hook-core fix (quoted absolute installed core, verbatim-prefix strip) and the installer port
separation landed by `08ec95c`.

Evidence at `08ec95c`: the three-OS `test` run **40804156318** is green (ubuntu 123218973230, macOS
123218972950, windows 123218973242); clean-install-and-parity **40804156470** was still in flight
when this was written and is **not claimed**.

Build/tests inside the packet read **not run**: `verdict.json`'s exec and mutation command set is
unapproved in the ledger (the approval hash predates it), so exec correctly refuses until Scott
approves the repository's commands — the GUI decision card or
`vdt approve-commands --i-am-the-human`.

Still executable: F28's visible steps (SmartScreen/UAC or Gatekeeper trust, double-click launch,
visible Install and editor clicks, enforce/fail-open behaviour), then re-running the affected
evidence with Scott's approval in place. External or the human's: signing/notarization and published
artifacts (F25); the tool's own `CLEAR TO SHIP` gate as a real release (F27, SPEC-13, SPEC-15);
served-vendor acceptance (SPEC-8); loaded DSH acceptance (F29, Codex-owned); and Scott's merge, tag
and publication.

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
* **SAFE-DELIVERY:** the final candidate packet and artifact record are refreshed at
  `08ec95c` before re-verification (`release/CANDIDATE-PACKET.txt`,
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

## F28 — packaged install, hooks, readiness (head 8ec95c)

Established from the packaged Windows artifact (zip c7af8759…645ba, inner core
8c3b18b…ae63): the wizard on its own port (4753, not 4717) completed the real install form;
the installed server got its own free port (63341, pid 16584); /readyz returned app id +
canonical project + matching pid; the installed core hash equals the package's inner hash; and the
installed page's Install-hooks action wrote "C:\…\install\vdt.exe" hook claude <Event> — quoted
absolute core, no bare dt, unrelated 
ode policy-check.js hook claude Stop preserved. The Stop
command executed verbatim through cmd exits 0 and resolves to dt 1.0.0. A \\?\ verbatim-path
defect found in the first run is fixed at 8ec95c (prefix stripped, unit-tested).

Still needs Scott's visible click-through (not claimed): SmartScreen/UAC trust of the unsigned
package, a real double-click launch, a human-visible Install click (the POST here was scripted), and
editor/enforce/fail-open behaviour. Signing and notarization remain external.

### Exec/test evidence in this packet is blocked on a human decision (expected)

The candidate packet's build/tests line reads **not run** because the repo's approved command set no
longer matches the policy: erdict.json now declares xec.commands
(cargo test --workspace --locked, cargo clippy --workspace --all-targets --locked -- -D warnings)
plus mutation.command (cargo run --quiet --bin vdt-mutation-runner), and the ledger's approval
hash predates that command set, so exec correctly refuses to run. This is the designed human-decision
boundary, not a defect: it was not forced, not self-approved and the packet was not hand-edited.

Scott must approve the repository's commands — in the GUI (the decision card on the project page) or
with dt approve-commands --i-am-the-human — before exec/test evidence can appear in a packet. The

ot run line stays visible until then.

The isolated installed server for the click-through is running at **http://127.0.0.1:63341/** with
pid 45164 (it was restarted standalone: the earlier instance, pid 16584, exited with the setup job
that spawned it). Its log/state root is
C:\Users\scott\AppData\Local\Temp\vdt-f28e-20261028-200435; stop it with
Stop-Process -Id 45164 (or 	askkill /PID 45164 /F). The wizard itself has exited.
