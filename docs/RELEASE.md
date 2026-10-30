# Releasing Verdict

The tool runs its own release lane before any tag. This file is the procedure and the checklist.
The human owns the tag; `vdt` never creates one, and the release workflow creates a **draft**, never
a published release.

## History: owner-authorized v1.0.0 disposition, October 29, 2026

Scott explicitly authorized merge, tag and publication of the already-tested,
provenance-pinned artifacts after reconciling the historical verification record.
For this release, retained CI/installed evidence and the disclosed historical
record accompany publication; a fresh post-tag CLEAR result is not claimed.
This disposition does not alter the product's verdict rules or CLEAR-only gate.
Use the existing signed Windows package; do not rebuild it for a documentation
commit. `release/VERIFICATION-RECORD.md` states the evidence and its limits.
The general procedure below remains guidance for future releases.
## Current release

- Version: **1.1.0**, tag **v1.1.0**, published October 30, 2026.
- Revision: the commit the tag points at, on `main`.
- Next release: bump the version in `Cargo.toml`, the crates, `Cargo.lock`, `verdict.json`, the
  installers, the README status line and the CI labels together; add a CHANGELOG section and
  rewrite `release/RELEASE-NOTES.md`. Merge to `main`, then tag.
- Public assets: the per-target archives and the Linux AppImage, each with a `.sha256`. Inside each
  archive: `vdt` (`vdt.exe`) and its own checksum, the double-click entry point
  (`Verdict-Setup.exe`, `vdt-setup`, or `Verdict.app` on macOS), README, LICENSE, NOTICE and
  `integrations/dsh/`. The draft release also carries `RELEASE.md` and `RELEASE-NOTES.md`
  (and, for v1.0.0 only, the historical `PACKET-BEFORE-TAG.txt`).

## The archive layout, exactly

`release/package.sh <version> <target> <zip|tar.gz> <binary> <outdir>` is the only packaging path;
the release workflow and the clean-install workflows both call it, so CI installs the layout the
release publishes. It produces:

```text
vdt-1.1.0-x86_64-unknown-linux-gnu/
  vdt                  the core
  vdt.sha256           SHA-256 of the core (the setup program checks this)
  vdt-setup            the double-click entry point (same program, setup mode)
  install.sh           legacy/silent script
  README.md LICENSE NOTICE
  integrations/dsh/
vdt-1.1.0-x86_64-unknown-linux-gnu.tar.gz
vdt-1.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256   SHA-256 of the downloaded archive
Verdict-1.1.0-x86_64.AppImage                       the Linux double-click installer
```

The outer `.sha256` is for checking the download before extracting. The inner one is what the
installer verifies after extraction; a missing inner checksum is an error, not a warning
(`-AllowUnverified` / `VDT_ALLOW_UNVERIFIED=1` is the explicit override).

## Before the tag (reuse completed evidence)

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p verdict-checkers --locked --features stub-checkers --test fixtures -- red::   # must FAIL
sh release/package.sh <version> <target> <zip|tar.gz> target/release/vdt dist
vdt check --lane release --checkers '*' --project .
vdt verdict --run --lane release --project .
vdt export --packet --lane release --out "$TEMP/verdict-packet.txt"
vdt export --sarif  --out release/findings.sarif
```

The commands above document the required checks; they are not a request to repeat
the completed candidate CI, mutation or desk runs. Reuse the exact results and
artifact provenance in `release/CANDIDATE-PROVENANCE.json`. The stubbed-checker
command must fail when it is required by a changed checker; it is not an extra
release-loop step for documentation or signing.

The release check runs the `release` block of `verdict.json` **first** (the project's own
release-truth commands), then the generic checks: tag on HEAD, the version read from the manifest's
own package section, product name and lockfile pin, configured assets and their hashes, the saved
published-release record, README links and commands, and any configured suite-wide pins.

A pre-tag run must read FAILED for `release:tag-missing`, and it also reads not-established for the
repo commands and the desks on a machine where the human has not approved the commands or
configured a cross-vendor provider. That is the honest pre-tag state, not a clear packet.

## What a release needs from the human

1. Approve the version and revision.
2. Merge the release branch to `main` and push.
3. Create the `v<version>` tag and push it. The `release` workflow builds the four archives,
   checks every download checksum, and attaches the archives, their `.sha256` files and the
   installers' documents to a **draft** GitHub release.
4. On the tagged revision, on a machine where the human has approved the project's commands and a
   cross-vendor desk provider is configured, run the release lane and export the packet. It must
   say `CLEAR TO SHIP` for this tag. Attach it to the draft.
5. Sign the Windows `vdt.exe` with the sign-only workflow (`release/RELEASE-RUNBOOK.md`,
   section e), rebuild the Windows zip with the signed `vdt.exe` and `Verdict-Setup.exe`, and
   refresh the inner and outer checksums on the draft. macOS is intentionally unsigned/not
   notarized by owner decision.
6. Publish the draft. Download one archive, check its `.sha256`, and run the installer. The
   installer places `vdt`, makes a desktop shortcut on that project on Windows, and opens the local
   page. After that the workflow is no-terminal: every decision is a button on the page at
   `http://127.0.0.1:4717/`, which the installer opens with its one-time link.

## Install, upgrade and rollback

- Windows: double-click the Azure-signed `Verdict-Setup.exe` from the archive.
  Its signature identifies Scott Converse. The wizard installs `vdt` under
  `%LOCALAPPDATA%\Verdict\bin`, registers the hosts you tick, makes a shortcut for the chosen
  project and opens its page. Upgrade: run the newer package; rollback: run the previous one.
- macOS: double-click `Verdict.app` (unsigned: right-click → Open the first time). It installs the
  app under `~/Applications`, the core under `~/.local/bin`, registers the chosen hosts, and opens
  the project page.
- Linux: make the AppImage executable if needed and double-click it. The wizard installs `vdt` in
  `~/.local/bin`, writes the desktop entry, registers the chosen hosts, and opens the project page.
- The `install.sh`/`install.ps1` scripts remain as silent/legacy paths for CI and recovery; they
  are not the user entry point.
- Hooks and the git gate are per project and reversible: `vdt hooks uninstall <host>
  --i-am-the-human` and `vdt gate uninstall --i-am-the-human`. Both put back what was there.
- Ledgers are local (`<project>/.verdict/`), git-ignored, and are never included in a release.

## Checks that can only be finalized after the authorized tag

- The `release` checker's tag test and the attachment of assets to the release, which need the tag
  to exist.
- The `CLEAR TO SHIP` packet for the tag itself: it is computed on the tagged revision, with the
  human's command approval and a configured cross-vendor provider, and attached to the release.
- Windows signing and its verification (Authenticode, timestamp, checksums) for each new
  Windows binary; macOS signing is not required by the owner.

Write the release packet outside the project and attach it after it is computed: writing it into
the tree first would make the tree differ from the one the checks were stamped on, and every check
would read stale. `--lane release` makes the packet state the lane the run was made in, never lower
than the lane the diff implies.

Current closeout: the preserved historical session still computes FAILED; see
release/RELEASE-RUNBOOK.md for the exact open records and owner decision. The
served-identity side check is optional, not a new completion blocker. Never
claim CLEAR TO SHIP by editing a packet or silently dismissing old findings.
