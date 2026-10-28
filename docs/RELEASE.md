# Releasing Verdict

The tool runs its own release lane before any tag. This file is the procedure and the checklist.
The human owns the tag; `vdt` never creates one, and the release workflow creates a **draft**, never
a published release.

## Candidate

- Version: **1.0.0**
- Revision: the last commit on `merge-trial` / `rc/1.0.0` (the exact hash is in the candidate
  report; the tag is the human's and points at that commit).
- Proposed tag: **v1.0.0**
- Merge target: `main` (fast-forward from `rc/1.0.0`).
- Public assets: four archives, one per supported target, each with a `.sha256`; inside each
  archive: `vdt` (`vdt.exe`) and its own `vdt.sha256` (`vdt.exe.sha256`), the installer, README,
  LICENSE, NOTICE and `integrations/dsh/`. The draft release also carries `RELEASE.md`,
  `RELEASE-NOTES.md` and `PACKET-BEFORE-TAG.txt`.

## The archive layout, exactly

`release/package.sh <version> <target> <zip|tar.gz> <binary> <outdir>` is the only packaging path;
the release workflow and the clean-install workflows both call it, so CI installs the layout the
release publishes. It produces:

```text
vdt-1.0.0-x86_64-unknown-linux-gnu/
  vdt                  the binary
  vdt.sha256           SHA-256 of the binary (the installer checks this)
  install.sh
  README.md LICENSE NOTICE
  integrations/dsh/
vdt-1.0.0-x86_64-unknown-linux-gnu.tar.gz
vdt-1.0.0-x86_64-unknown-linux-gnu.tar.gz.sha256   SHA-256 of the downloaded archive
```

The outer `.sha256` is for checking the download before extracting. The inner one is what the
installer verifies after extraction; a missing inner checksum is an error, not a warning
(`-AllowUnverified` / `VDT_ALLOW_UNVERIFIED=1` is the explicit override).

## Before the tag (all runnable now)

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p verdict-checkers --locked --features stub-checkers --test fixtures -- red::   # must FAIL
sh release/package.sh 1.0.0 <target> <zip|tar.gz> target/release/vdt dist
vdt check --lane release --checkers '*' --project .
vdt verdict --run --lane release --project .
vdt export --packet --out release/packet.txt
vdt export --sarif  --out release/findings.sarif
```

The fourth command must fail: with the checkers stubbed out every red fixture has to go red.

The release check runs the `release` block of `verdict.json` **first** (the project's own
release-truth commands), then the generic checks: tag on HEAD, the version read from the manifest's
own package section, product name and lockfile pin, configured assets and their hashes, the saved
published-release record, README links and commands, and any configured suite-wide pins.

A pre-tag run must read FAILED for `release:tag-missing`, and it also reads not-established for the
repo commands and the desks on a machine where the human has not approved the commands or
configured a cross-vendor provider. That is the honest pre-tag state, not a clear packet.

## What a release needs from the human

1. Approve the version and revision.
2. Merge the candidate to `main` and push.
3. Create the `v1.0.0` tag and push it. The `release` workflow builds the four archives, checks
   every download checksum, and attaches the archives, their `.sha256` files, the installers'
   documents and the pre-tag packet to a **draft** GitHub release.
4. On the tagged revision, on a machine where the human has approved the project's commands and a
   cross-vendor desk provider is configured, run the release lane and export the packet. It must
   say `CLEAR TO SHIP` for this tag. Attach it to the draft.
5. Supply signing certificates if signed installers are required. Without them the archives are
   checksummed but **unsigned**, and the release notes say so.
6. Publish the draft. Download one archive, check its `.sha256`, and run the installer. The
   installer places `vdt`, makes a desktop shortcut on that project on Windows, and opens the local
   page. After that the workflow is no-terminal: every decision is a button on
   `http://127.0.0.1:4717/`.

## Install, upgrade and rollback

- Windows: `powershell -ExecutionPolicy Bypass -File install.ps1` from the archive. Upgrade: run
  the newer installer; it replaces `%LOCALAPPDATA%\Verdict\bin\vdt.exe`. Rollback: run the previous
  archive's installer.
- Linux/macOS: `sh install.sh` from the archive; it installs `~/.local/bin/vdt`. Same upgrade and
  rollback.
- Hooks and the git gate are per project and reversible: `vdt hooks uninstall <host>
  --i-am-the-human` and `vdt gate uninstall --i-am-the-human`. Both put back what was there.
- Ledgers are local (`<project>/.verdict/`), git-ignored, and are never included in a release.

## Checks that can only be finalized after the authorized tag

- The `release` checker's tag test and the attachment of assets to the release, which need the tag
  to exist.
- The `CLEAR TO SHIP` packet for the tag itself: it is computed on the tagged revision, with the
  human's command approval and a configured cross-vendor provider, and attached to the release.
- Signed-installer verification, which needs the certificates.
