# Verdict 1.1.0

A security release. It closes five ways an AI agent could get around the human's decisions, and
fixes the Linux AppImage build. Verdict's one rule is unchanged: **the agent writes claims, never
verdicts.**

## Security fixes

- **Desk provider programs need the human's command approval.** A repo's `verdict.json` could name
  any program as a desk provider. Now the provider settings are part of the approved command set,
  and an unapproved provider is never started.
- **A rule change during a session needs the human's acceptance.** If `verdict.json` or the global
  policy changes mid-session, the verdict cannot read Verified or Clear to ship until the human
  accepts the change with `vdt accept-policy --i-am-the-human`.
- **The local page answers only the browser that opened its one-time link.** A program that only
  knows the port can no longer read the page or press its buttons.
- **A human decision on the command line needs a person who types `yes`.** The
  `--i-am-the-human` flag alone is no longer enough. An agent's shell, which has no terminal, is
  refused and nothing changes.
- **The ledger is sealed with a key outside the project.** A fully rewritten ledger, a dropped
  seal or a row added by hand now reads BROKEN in `vdt ledger verify`.

## Changes you may notice

- `vdt serve` prints a one-time link. Open the page with that link, or with `vdt serve --open`.
- Command-line human decisions ask you to type `yes`. Run them in your own terminal window, or use
  the buttons on the Verdict page.
- Verdict makes `~/.verdict/seal.key` on its first ledger write. Keep it; it is what proves your
  ledger was not rewritten.
- A ledger from 1.0.0 reads OK with a "not sealed yet" note, and its next write seals it.
- A ledger copied to another user or machine reads BROKEN there, because that key differs.

## Install

Windows: double-click `Verdict-Setup.exe`; Linux: run the AppImage; macOS: open `Verdict.app`.
The script installers remain available for silent installation and recovery. Check each download
against its `.sha256` file.

## Honest limits

- The seal stops a rewrite by a program that cannot read your home folder. A program running as
  the same user can read the key and forge a seal.
- The typed `yes` raises the bar but is not proof of a person: a program that opens a
  pseudo-terminal can still type it. The page's buttons, behind the one-time link, are the other
  human surface.
- macOS is unsigned and not notarized, as in 1.0.0. Linux signing is optional.
- The host-adapter limits listed for 1.0.0 still apply; see `docs/HOSTS.md`.

The 1.0.0 notes and verification record are on the
[1.0.0 release](https://github.com/scottconverse/verdict/releases/tag/v1.0.0).

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
