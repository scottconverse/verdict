# Candidate acceptance tooling

`cross-vendor-receipt.ps1` builds the core and Codex adapter from a clean committed
candidate, exports that exact Git tree, and copies the existing ledger through
SQLite's online backup API. Python 3 and PowerShell are requirements of this
developer harness, not of the installed Verdict product. The original head must
match the copied tail and remain stable during backup; `vdt ledger verify` must
pass before any provider call. Prior receipts are retained and compared verbatim.
Exactly one new successful desk receipt for the requested session and desk is
required, irrespective of how many historical desk receipts exist.

Run `pwsh -File release/cross-vendor-receipt.ps1 -DryRun` for build, copy and
preflight without a model call. A real run requires authorization for the desk
acceptance. `-Session` selects the existing builder session and `-Model` requests
a verifier model; empty model preserves the CLI default. These are requested
settings, not proof of the model served. The signed-in CLI configuration stays
in its supported location; credentials are never copied into the acceptance
folder. Temporary HOME and Verdict state are restored on exit.

The harness invokes one desk; the existing provider has at most two CLI attempts.
It does not automatically repeat a failed desk. Keep its log, binary hashes,
candidate commit, backup receipt and ledger on failure as well as success. A
successful desk receipt does not establish a full release lane or CLEAR TO SHIP.

Focused non-model checks:

```powershell
python -B release/test-copy-ledger.py
pwsh -File release/test-ledger-receipt-delta.ps1
```

The clean-install workflow retains its successfully tested Linux archive and
AppImage, Windows archive, macOS archive and checksum files as candidate artifacts
named by platform and full commit. Download those run artifacts for reconciliation;
they are unsigned candidates, not a published or signed release. Existing two-host
generation parity compares only the `generated-*` artifacts.
