# Release runbook — candidate to tagged release

This is the human path from the candidate on `merge-trial` to a tagged, published release. Run the
steps in order. Commands are written for the repository root on Windows (PowerShell); the packaging
scripts are POSIX `sh` and also run on Linux/macOS.

Everything **above the signing step** has been exercised on this candidate (packaged Windows
install, readiness/identity, hook path, Linux AppImage in CI). **Signing and publication have not been
exercised**: no Authenticode or Apple credentials exist in this environment, so those steps are
placeholders that name exactly what a signer must supply.

## a) Approve the repo's commands (must come first)

The ledger holds a human approval for a command set. This candidate's `verdict.json` changed the
exec and mutation commands, so the old approval no longer matches and exec correctly refuses —
that is why a packet can read `build/tests: not run`.

Approve in the GUI: open the project page's session view and press the approval card's button
(the card lists the exact commands). Or from a terminal:

```powershell
target\release\vdt.exe approve-commands --i-am-the-human
```

Why it must precede evidence: exec only runs commands the human approved by hash, so any packet
produced before this step cannot contain `ran`-tier build/test evidence, and a release decision that
leans on it would be resting on "not run".

## b) Run the release lane and export the packet

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

If any of these cannot run, say so and ship unsigned with that limitation stated — do not describe an
unsigned artifact as signed.

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
