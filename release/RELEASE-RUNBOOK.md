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

## e) Signing — requires credentials not present in this environment

**Windows (Authenticode).** A signer needs a code-signing certificate (PFX or a hardware/cloud
signing profile) and its password/timestamp authority. The exact shape:

```powershell
signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
  /f <path-to.pfx> /p <password> `
  dist\vdt-1.0.0-x86_64-pc-windows-msvc\vdt.exe
signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
  /f <path-to.pfx> /p <password> `
  dist\vdt-1.0.0-x86_64-pc-windows-msvc\Verdict-Setup.exe
signtool verify /pa dist\vdt-1.0.0-x86_64-pc-windows-msvc\Verdict-Setup.exe
```

Sign the inner executables **before** zipping (the archive's hash changes afterwards, so rebuild
`dist\*.zip` and its `.sha256` after signing).

**macOS (codesign + notarytool).** A signer needs a Developer ID Application certificate in the
keychain, an Apple ID/team id and an app-specific password (or API key). The exact shape:

```sh
codesign --force --options runtime --timestamp --sign "Developer ID Application: <Name> (<TEAMID>)" \
  dist/vdt-1.0.0-aarch64-apple-darwin/Verdict.app
xcrun notarytool submit <Verdict-1.0.0-aarch64-apple-darwin.tar.gz> \
  --apple-id <apple-id> --team-id <TEAMID> --password <app-specific-password> --wait
xcrun stapler staple dist/vdt-1.0.0-aarch64-apple-darwin/Verdict.app
```

Linux AppImage signing (optional, if a key exists): `gpg --detach-sign` beside the AppImage.

If required signing cannot run, retain the unsigned candidates with that limitation
stated. This does not satisfy required signing acceptance or authorize unsigned
publication. Never describe an unsigned artifact as signed.

## f) Tag and publish — human only

These are the human's actions; no tool in this repository performs them.

```powershell
git tag -a v1.0.0 -m "Verdict 1.0.0"
git push origin v1.0.0
gh release create v1.0.0 --draft --title "Verdict 1.0.0" --notes-file release\RELEASE-NOTES.md
```

Attach to the release, from the same tag build:

* the acceptance packet (`release\RELEASE-PACKET.txt`) — attached beside the tag's notes, with any
  commit-anyway override printed next to the `FAILED` verdict it went past;
* the per-OS artifacts: `dist\vdt-1.0.0-x86_64-pc-windows-msvc.zip`,
  `dist\vdt-1.0.0-x86_64-unknown-linux-gnu.tar.gz`,
  `dist\vdt-1.0.0-aarch64-apple-darwin.tar.gz`, `dist\Verdict-1.0.0-x86_64.AppImage`;
* every `*.sha256` beside them;
* the signed executables (if signing was possible) and the signing statement.

Publication stays a separate, deliberate press by the human. Nothing in this runbook tags, merges,
signs or publishes on its own.
