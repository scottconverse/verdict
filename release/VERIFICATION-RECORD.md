# Historical verification record — reconciled against current evidence

Verdict 1.0.0 preserves the earlier development session's test attempts and
findings for traceability. This historical record does not describe the current
product's status. Current dispositions are recorded in
`release/CANDIDATE-RECONCILIATION.md`; original machine-generated results remain
unaltered in `HISTORICAL-VERIFICATION-PACKET.txt` and repository history.

## Current release evidence

- Tested runtime: `300e9f4cedb9c21df79b51884293ca455cd3caf2`. Later source commits
  change release documentation only; the published packages reuse this runtime.
- Three-platform Rust tests: [run40930130637](https://github.com/scottconverse/verdict/actions/runs/40930130637), successful.
- Six-job package/install/parity checks: [run40930130633](https://github.com/scottconverse/verdict/actions/runs/40930130633), successful.
- Windows Azure sign-only: [run40935832675](https://github.com/scottconverse/civiccast-native/actions/runs/40935832675), successful.
  Authenticode and Microsoft timestamp were verified locally; the actual signed
  archive was installed in isolated settings and reported vdt1.0.0.
- All four published artifact hashes are pinned in `CANDIDATE-PROVENANCE.json`
  and their adjacent `.sha256` downloads.

## What the reconciliation means

The uncapped historical export contains 29 major/blocker occurrences. Existing
evidence supersedes 27 as current product blockers. Two describe the absent
release tag before publication. Some earlier execution attempts actually failed
or were interrupted; those outcomes remain in history. Earlier static-evidence
overclaims were corrected, without promoting file existence into runtime proof.
Existing later mutation receipts each report three killed mutants and no
survivors. No tests or model calls were repeated to relabel this record.

## Evidence limits retained

The old session lacks accepted baseline/final-claim inputs and fresh current
ledger checks. Separate CI and installed receipts are not silently inserted as
fresh ledger records. This is not a fresh post-tag CLEAR TO SHIP result. The
maintainer expressly authorized publication after the evidence reconciliation;
the product's CLEAR-only gate and original machine results remain unchanged.

Windows signing is complete. macOS is intentionally unsigned and not notarized.
The served-vendor identity side check is optional by maintainer decision; no
identity is inferred. Native macOS headless setup and launcher decision checks
are not claimed as a Finder desktop run. Historical unclean-process and
interrupted-lane receipts are not relabeled as passes. Host-specific limits are
listed in `docs/HOSTS.md` and the release notes.

Publication was explicitly authorized by Scott on October 29, 2026 and executed
by Codex on his behalf. No physical human click or autonomous triad completion
is claimed.
