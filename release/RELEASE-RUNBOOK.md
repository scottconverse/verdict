# Release runbook — candidate to tagged release

This is the human path from the candidate on `merge-trial` to a tagged, published release. Run the
steps in order. Commands are written for the repository root on Windows (PowerShell); the packaging
scripts are POSIX `sh` and also run on Linux/macOS.

## Current acceptance and reuse

The current tested runtime is `300e9f4`. Required test `40930130637` passed on
all three OSes, and package/install/parity `40930130633` passed all six jobs.
The exact Windows package was installed in isolated settings; both home and
session pages now accurately say hook installation edits the selected agent
settings while preserving unrelated settings. No decision, checker, desk or
model call was submitted. All four checksum-verified packages are staged in
`dist`; `CANDIDATE-PROVENANCE.json` identifies them and the installed core.
The earlier `a612b4d` packet/export proof below is reused: only two GUI notices
and documentation changed. Do not rebuild or rerun a lane for bookkeeping.

The current installed export labels all nine unsupported individual claims
unproven; the original 598-record ledger and head remain byte-identical.
Earlier installed readiness, absolute hook paths, unrelated-hook preservation,
silent fail-open, real Claude send-backs and editor-click receipts retain their
recorded scope. They are not a new packaged end-to-end host acceptance run.

The authorized real Codex desk call completed, but its returned events contained
neither a served model nor vendor. Transport success is not verified vendor
independence. Current code refuses unavailable identity; requested settings,
model-family hints and another identical call do not resolve the missing proof.

Scott's October 29 disposition: the served-identity side check above is optional,
not a completion blocker. Its actual receipt remains unavailable; no vendor
identity is inferred and the product's fail-closed checks are unchanged.

Windows is signed through Scott's established Azure account/profile
`scottconverse-signing` / `ScottConversePublic`. Sign-only workflow
`scottconverse/civiccast-native/sign-native-installer.yml`, run `40935832675`,
signed the exact already-tested core; the identical setup launcher uses the same
signed bytes. Local Authenticode and timestamp verification passed, as did silent
installation from the final signed archive. Internal and outer checksums were
refreshed; original unsigned CI packages remain retained. No rebuild was needed.

macOS is deliberately unsigned and not notarized: Scott states no macOS signing
certificate exists or will be obtained. Do not request one again or claim signed
macOS delivery. Applicable external-desktop evidence retains its recorded limits.
SmartScreen/UAC/Gatekeeper exercises are not added acceptance requirements.
Merge, tag, publication and post-tag CLEAR remain separate owner boundaries.
The failed historical packet must not be relabelled or reset.

## a) Existing command approval — do not repeat

Approval record 406 already records Scott's explicit authorization of the current
command set, submitted by Codex through the real approval page. It was a delegated
page POST, not a physical human click. Do not request the same approval again or
attribute historical unfinished checks to missing consent.

If the command set actually changes in future, its changed hash requires a fresh
human decision through the GUI before execution. This runbook grants no permission
to change that set or self-approve it.

## b) Future release-lane execution and packet export

The commands below describe a future authorized acceptance execution. Recover
existing owned work and resolve its specific failed/incomplete conditions first;
do not run this block merely because this document changed. Export/report alone
reuse recorded evidence and do not run checkers or desks.

```powershell
target\release\vdt.exe verdict --run --lane release --session <session-id>
target\release\vdt.exe export --packet --lane release --session <session-id> --out release\RELEASE-PACKET.txt
target\release\vdt.exe report --session <session-id> > release\RELEASE-REPORT.txt
```

`--session` is optional (the newest session is used). The packet and report are the evidence for the
tag; do not hand-edit them.

## c) Read the verdict line, then the dogfood gate

```powershell
Select-String -Path release\RELEASE-PACKET.txt -Pattern '^VERDICT:'
sh release/dogfood-gate.sh release/RELEASE-PACKET.txt
```

The gate exits **0 only** when the packet's own verdict line is exactly `VERDICT: CLEAR TO SHIP`
(anchored match; a `CLEAR TO SHIP` mentioned inside a `FAILED` line does not pass). Anything else
prints the packet's FIX FIRST and NOT ESTABLISHED evidence and exits 1. The gate never tags,
creates a release or publishes.

## d) Per-OS packaging

Windows archive (run on Windows, from the repo root, after `cargo build --release --locked --bin vdt`):

```powershell
sh release/package.sh 1.0.0 x86_64-pc-windows-msvc zip target/release/vdt.exe dist
```

Linux archive:

```sh
sh release/package.sh 1.0.0 x86_64-unknown-linux-gnu tar.gz target/release/vdt dist
```

macOS archive and app bundle:

```sh
sh release/package.sh 1.0.0 aarch64-apple-darwin tar.gz target/release/vdt dist
```

Linux AppImage (needs `appimagetool`; the script runs it under `APPIMAGE_EXTRACT_AND_RUN=1`):

```sh
APPIMAGETOOL=/tmp/appimagetool sh release/package-appimage.sh 1.0.0 target/release/vdt dist
```

Each produces its artifact **plus `<artifact>.sha256`**, and the archives contain the core (`vdt`,
`vdt.exe`) with its own `vdt.sha256`, which the setup program verifies before installing.

## e) Signing — current Windows candidate completed

The current Windows archive is already signed. Reuse the exact package and
checksums in `CANDIDATE-PROVENANCE.json`; do not sign or rebuild it again for
bookkeeping. Authenticode is Valid for Scott Converse, timestamped by Microsoft;
installation from the final signed archive passed.

For a future changed Windows binary, Scott's established sign-only process is
`scottconverse/civiccast-native/sign-native-installer.yml`, using Azure account
`scottconverse-signing`, profile `ScottConversePublic`. Its stored credentials
remain in that workflow. The inputs are an unpublished draft intake asset and
its exact SHA-256. It returns `signed-native-installer` with the signed file and
receipt. Verify the returned input/output hashes, Authenticode and timestamp;
refresh inner and outer checksums and verify affected signed installation.
Explicit destination approval is required before a future intake upload. Never
publish that intake draft or copy its secrets into another repository.

macOS is intentionally unsigned/not notarized under Scott's October 29 decision.
No macOS certificate will be obtained. Linux signing is optional. Neither is an
unresolved certificate request or a new completion blocker.

## f) Tag and publish — human only

These are the human's actions; no tool in this repository performs them.

```powershell
git tag -a v1.0.0 -m "Verdict 1.0.0"
git push origin v1.0.0
gh release create v1.0.0 --draft --title "Verdict 1.0.0" --notes-file release\RELEASE-NOTES.md
```

Attach the provenance-pinned tested artifacts to the release. Documentation-only commits do not require another runtime build. The existing tag workflow can produce a new unsigned candidate; it is not the signed Windows artifact. Before publication, use the signed Windows archive and checksum recorded in CANDIDATE-PROVENANCE.json. Never replace them with an unsigned rebuild.

* the acceptance packet (`release\RELEASE-PACKET.txt`) — attached beside the tag's notes, with any
  commit-anyway override printed next to the `FAILED` verdict it went past;
* the per-OS artifacts: `dist\vdt-1.0.0-x86_64-pc-windows-msvc.zip`,
  `dist\vdt-1.0.0-x86_64-unknown-linux-gnu.tar.gz`,
  `dist\vdt-1.0.0-aarch64-apple-darwin.tar.gz`, `dist\Verdict-1.0.0-x86_64.AppImage`;
* every `*.sha256` beside them;
* the signed executables (if signing was possible) and the signing statement.

Publication stays a separate, deliberate press by the human. Nothing in this runbook tags, merges,
signs or publishes on its own.

## Current release decision, October 29

Scott-authorized reconciliation is complete: all 29 historical open major/blocker
occurrences are mapped individually in CANDIDATE-RECONCILIATION.md. Existing
evidence supersedes 27 as current product blockers; records268/562 are the still
absent release tag. No fresh current implementation defect was established by
that reconciliation. Original ledger history and FAILED report remain intact.

The remaining owner decision is final merge/tag/publication of the pinned tested
artifacts with the historical FAILED self-verification packet and its limits
disclosed. This is not CLEAR TO SHIP: the old session lacks accepted baseline,
final claims and fresh ledger checks. The CLEAR-only gate is unchanged. A fresh
CLEAR result remains incomplete if required instead of release with disclosure.
Do not mark true historical findings "not real" merely to clear the display.

Use the signed Windows artifact in CANDIDATE-PROVENANCE.json; do not replace it
with an unsigned tag-workflow rebuild. No Mac signing certificate, optional
served-identity side check, trust-screen test or blanket external Desktop run is
required by this reconciliation. Final publication has not been performed.
