# Releasing Verdict

The tool runs its own release lane before any tag. This file is the procedure and the checklist.
The human owns the tag; `vdt` never creates one.

## Candidate

- Version: **1.0.0**
- Revision: the commit this file ships in (`git rev-parse HEAD` on `main` after the merge).
- Proposed tag: **v1.0.0**
- Merge target: `main` (fast-forward from the implementation branch).
- Public assets: four archives, one per supported target, each with a `.sha256`, plus
  `RELEASE.md`, `RELEASE-NOTES.md` and the installers inside the archives.

## Before the tag (all runnable now)

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test -p verdict-checkers --locked --features stub-checkers --test fixtures -- red::   # must FAIL
vdt check --checkers release --lane release --project .
vdt export --packet --out release/packet.txt
vdt export --sarif  --out release/findings.sarif
```

The fourth command must fail: with the checkers stubbed out every red fixture has to go red. The
release check runs the `release` block of `verdict.json`; it cannot pass its tag check before the
human creates the tag, which is why the tag is the last step and not a check the agent can satisfy.

The clean-install and two-machine checks run in CI (`.github/workflows/clean-install-and-parity.yml`):
a clean Ubuntu container follows the README, and Ubuntu and Windows each generate the host files and
a third job compares them byte for byte.

## What a release needs from the human

1. Approve the version and revision.
2. Merge the candidate to `main` and push.
3. Create the `v1.0.0` tag and push it. The `release` workflow builds the four archives, computes
   their SHA-256 files and attaches them, the installers and these documents to the GitHub release.
4. Supply signing certificates if signed installers are required. Without them the archives are
   checksummed but **unsigned**, and the release notes say so.
5. Download one archive, check its `.sha256`, and run the installer. The installer places `vdt`,
   makes a desktop shortcut on Windows, and opens the local page. After that the workflow is
   no-terminal: every decision is a button on `http://127.0.0.1:4717/`.

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
- Signed-installer verification, which needs the certificates.
- The final `CLEAR TO SHIP` packet for the tag itself: it is produced from the tagged revision and
  attached to the release. It cannot be honestly produced before the tag exists.
