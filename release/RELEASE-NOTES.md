# Verdict 1.0.0

The first release. Verdict 1.0.0 is one open-source tool that verifies work done by AI coding
agents, built on one rule: **the agent writes claims, never verdicts.**

## What is in it

- An append-only, hash-chained evidence ledger (SQLite, blake3) with a JSONL export and a verifier
  that names the record that broke.
- Hook adapters for Claude Code, Codex, Cursor, Antigravity and DeepSeek Harness. Claude Code and
  Codex are proven against live sessions; Cursor, Antigravity and DSH document their status in
  `docs/HOSTS.md`.
- Ten deterministic checkers: wiring, baseline_diff, regressions, tests_weakened, unfinished, exec,
  mutation, firstrun, release, claims_vs_code, each with a red fixture that must fail and a green
  one that must stay quiet.
- Seven LLM desks with packaged rubrics, a Verdict-owned provider protocol, per-desk receipts,
  citation rules, hypotheses, cross-vendor refusal, the global warn-only `single_vendor` downgrade,
  and a five-finding report cap.
- Shadow mode by default; enforce mode with pre-edit holds and at most two Stop-hook send-backs,
  after which the agent is released and the human is told.
- A localhost page that owns every human decision: accept a finding, approve the repo's commands,
  accept a prompt translation, raise the lane, switch mode, pause with a duration, commit anyway
  with a typed reason, and download the packet and SARIF.
- `vdt generate`, which emits the host skills and hook registrations from the policy as a pure
  function of it, and `vdt export --sarif|--packet`.

## Install

Windows: double-click `Verdict-Setup.exe`; Linux: run the AppImage; macOS: open
`Verdict.app`. The Windows core and setup launcher are Azure-signed as Scott
Converse with a Microsoft timestamp; the final signed archive was installed and
verified. macOS is intentionally unsigned and not notarized under the owner's
decision. Linux signing is optional. The script installers remain available for
silent installation and recovery.

## Verification record

**Historical verification record — reconciled against current evidence.**
Earlier test attempts and findings are retained for traceability. Existing
evidence supersedes 27 historical occurrences as current product blockers; two
pre-release observations concern the then-absent release tag. This is not a new
list of product defects or a fresh post-tag self-verification result.

The maintainer authorized this release using the retained exact-runtime CI,
installed receipts and reconciliation. See the attached `VERIFICATION-RECORD.md`,
`CANDIDATE-PROVENANCE.json` and `HISTORICAL-VERIFICATION-PACKET.txt` for the scope,
limits and unaltered historical machine output. Runtime `300e9f4` passed
[three-platform tests](https://github.com/scottconverse/verdict/actions/runs/40930130637)
and [six-job installation/parity checks](https://github.com/scottconverse/verdict/actions/runs/40930130633).
The four published artifacts reuse that tested runtime; no bookkeeping rebuild
replaces the signed Windows package.

## Honest limits

- Cursor, Antigravity and DeepSeek Harness have not been recorded against a live host in this
  release; their payload shapes are documented and their failure mode is a recorded
  `check_unavailable`, never a clean result.
- The Windows process tree uses a suspended start and a job object; the Unix path is covered by CI
  but was not run by hand on this machine.
- Windows signing uses the existing ScottConversePublic Azure profile. No macOS
  signing certificate is planned; do not describe the macOS package as notarized.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
