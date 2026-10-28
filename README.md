# Verdict

**Status: 1.0.0 release candidate.** What each piece can prove is below, and every claim is
limited to the evidence named for it. [docs/HOSTS.md](docs/HOSTS.md) says which host adapters have
been recorded against a live host; [docs/RELEASE.md](docs/RELEASE.md) is the release procedure.

The candidate is not a published release. The human creates the tag and publishes; the release
workflow builds a draft. The parts that still need a live host or a certificate are named in
"Honest status" at the end of this file, not papered over.

Verdict is one open-source tool that verifies work done by AI coding agents.

## The one rule

**The agent writes claims, never verdicts.**

Claims go into an append-only evidence ledger. Verdicts come out of the ledger from things that are
not the builder: deterministic checkers, a model from a different vendor, or the human.

## What runs today

| Piece | State |
| --- | --- |
| Evidence ledger (SQLite, blake3 hash chain, append-only, JSONL export) | built |
| Claude Code and Codex hook adapters (`vdt hook`, `vdt hooks install`) | built; live sessions recorded on this machine |
| Cursor and Antigravity hook adapters (`vdt hook cursor|antigravity`, `vdt hooks install`) | built; documented file shapes, payload field names not yet verified against a live host ([docs/HOSTS.md](docs/HOSTS.md)) |
| DeepSeek Harness integration: MCP entry, `vdt hook dsh`, and the native plugin in `integrations/dsh` | built; plugin maps the harness's real events and a live run against `vdt` recorded an intact ledger; not loaded into this workstation's DSH config ([docs/DSH.md](docs/DSH.md)) |
| MCP server (`vdt mcp`): `verdict_check`, `verdict_scan`, `verdict_claim`, `verdict_done_when`, `verdict_decisions` | built |
| `vdt mcp-install` for Claude Code, Cursor, Codex, DSH and Antigravity (human-gated) | built; Claude, Cursor and Codex exercised |
| Deterministic checkers: wiring, baseline_diff, regressions, tests_weakened, claims_vs_code, unfinished, exec, mutation, firstrun, release | built, each with red and green fixtures |
| Tree-sitter extractors: FastAPI, SQLAlchemy/asyncpg, Alembic, Typer, Python and TypeScript MCP tools, TypeScript fetch/openapi-ts/TanStack/Kysely, Tauri, React | built |
| `vdt scan` (whole tree, no plan, writes nothing) | built |
| Verdict computation, lanes, pre-commit gate, commit anyway, decisions queue | built |
| Localhost page (`vdt serve`, port 4717) | built: sessions, reports, claims and evidence, unplanned work, record log, decisions, mode, pause, per-host install, packet/SARIF download |
| Stop and `verdict_check` run fresh verification | built: both run the lane's checkers and desks, then write the verdict and the general gate-result record |
| LLM desks: seven packaged rubrics, provider dispatch, citations, hypotheses, five-finding cap | built |
| Cross-vendor routing, refusal, global warn-only (`single_vendor`) and per-desk receipts | built; the receipt records the served vendor/model when the provider states it, effective settings and usage, and refuses an answer whose stated vendor is the builder's on a run that requires another |
| Enforce mode: pre-edit hold and Stop-hook send-backs (at most two), then release and tell the human | built (Claude Code verified; other hosts' output shapes unverified) |
| `vdt generate` (skills + hook registrations from the policy, byte-identical) | built |
| `vdt export --sarif`, `--packet` and `--decisions` | built |
| Release lane, installers, clean-container and two-machine checks | built: Windows GUI installer (`install.ps1`), POSIX installer, tag-triggered archives built by one packaging script the installers and CI share; the clean-container and two-machine runs are CI jobs on a pushed revision ([docs/RELEASE.md](docs/RELEASE.md)) |

## Names

- Product and repository: Verdict
- Crate: `verdict-gate`
- Command: `vdt`

## Commands

```text
vdt ledger [verify|export]      read or export the ledger
vdt sessions                    list recorded sessions
vdt report [--session ID]       the plain-text report
vdt hook <host> <event>         what a host hook calls (never fails, never prints)
vdt hooks install|uninstall <host>
vdt mcp [--project DIR]         the MCP server on stdio
vdt mcp-install <host>          register the MCP server (claude, cursor, codex, dsh, antigravity)
vdt check --checkers a,b        run named deterministic checkers
vdt scan [--project DIR]        whole-tree scan, writes nothing
vdt verdict [--session ID] [--run]
vdt desks [--session ID] [--lane L]   run the lane's LLM desks, one receipt each
vdt generate [--out DIR]        emit skills and hook registrations from the policy
vdt export --sarif|--packet|--decisions [--out FILE]
vdt gate pre-commit|install|uninstall
vdt commit-anyway
vdt serve [--port N]
```

## Policy

The built-in default (`policy/default.json`) is neutral: no plan file, no commands, no blast globs,
no verifier names and no project paths. A project supplies its own in `verdict.json` at its root;
a global default lives at `~/.verdict/policy.json`. Missing configuration never becomes a pass:
no configured command means no `ran` evidence, and missing blast classification is stated in the
report and can never lower a lane.

`policy/schema.json` is the JSON Schema for both files.

## Desks and providers

The seven desks (engineer, docs, qa, ux, security, legal, refuter) ship as rubric files inside the
binary. A desk is dispatched only through a provider the policy names:

```json
"vendors": {
  "cross_vendor_required_from_lane": "standard",
  "single_vendor": "refuse",
  "providers": [
    { "name": "verifier", "vendor": "openai", "command": "my-verifier",
      "args": [], "model": "gpt-6.1-sol", "timeout_secs": 300 }
  ]
}
```

A provider reads one JSON request (`{protocol, model, prompt}`) on stdin and writes one JSON
answer on stdout:

```json
{"findings":[{"severity":"major","title":"...","evidence":[{"record":12}],"fix":"..."}],
 "hypotheses":["..."],
 "proposals":[{"checker":"mutation","reason":"the tax branch changed"}],
 "usage":{"input_tokens":11},
 "served":{"vendor":"openai","model":"gpt-6.1-sol","settings":{"temperature":0}}}
```

Any model, gateway or routing tool can be a provider; none is required. Above the cross-vendor
lane a desk is dispatched only to a provider whose vendor differs from the builder's; with none
available the run is refused and recorded, unless the **global** policy sets
`vendors.single_vendor: "warn"`, in which case the receipt says `single_vendor`. When the answer
states the vendor that served it, a builder-vendor answer on a cross-vendor run is refused and
recorded rather than counted. `vendors.verifier_order` is followed when it names vendors or
providers. Every finding must cite a ledger record (`{"record": N}`) or a current `file:line`; a
citation that does not resolve makes the answer a hypothesis. A proposal is recorded and never run
by the desk; with `desks.run_proposals: true` the core runs a proposed mutation or firstrun checker.

Every lane requires its desks' receipts: in the standard lane and above, a missing desk makes the
verdict PARTIAL, never clean.

## Install

From a release archive:

- Windows: run `install.ps1` (it opens a small installer window: project folder, MCP registration,
  shortcut, then it opens the page).
- Linux/macOS: `sh install.sh`.

The archive holds `vdt` (`vdt.exe`) and `vdt.sha256` / `vdt.exe.sha256`, plus the installer, README,
LICENSE, NOTICE and the optional DSH integration. The installer checks the binary's own SHA-256 and
**refuses to install without one** unless it is told to (`-AllowUnverified` on Windows,
`VDT_ALLOW_UNVERIFIED=1` on Linux/macOS). The `.sha256` beside the downloaded archive is for
checking the download itself before extracting. The Windows installer installs `vdt`, registers the
Verdict MCP server for Claude Code and Cursor in the chosen project, puts a desktop shortcut on that
same project (`serve --project <folder>`), waits for the page, and opens
`http://127.0.0.1:4717/`. The shell installer does the same for the current folder. After that every
step is a button on that page; no terminal is needed. The archives are unsigned unless the release
says otherwise; checksums and rollback are in [docs/RELEASE.md](docs/RELEASE.md).

## Hosts and docs

- [docs/HOSTS.md](docs/HOSTS.md): which adapters have been recorded against a live host, and what
  is still unverified.
- [docs/DSH.md](docs/DSH.md): the optional DeepSeek Harness integration (MCP connection and the
  native plugin in [integrations/dsh](integrations/dsh)).
- [docs/DECISIONS.md](docs/DECISIONS.md): every design question the workers raised, with its ruling.
- [docs/RELEASE.md](docs/RELEASE.md): the release lane, the tag, the installer and rollback.

## Design

The full design is in [docs/design.md](docs/design.md).

## Building

Rust 1.97 or newer, edition 2024.

```text
cargo build --workspace
cargo test --workspace
cargo test -p verdict-checkers --features stub-checkers --test fixtures -- red::
```

The second-to-last command runs the suite. The last one must **fail**: with the checkers stubbed
out, every red fixture has to go red, or the fixtures are not proving anything.

## Honest status

These are limits of the evidence, not claims of completion:

- **Live hosts.** Claude Code and Codex were recorded from real sessions (see docs/HOSTS.md).
  Cursor and Antigravity payload shapes are built and unit-tested but were not recorded against a
  live host (neither is installed here). The DSH plugin is built and tested with the harness's real
  event shapes, but it is **recording-only** and was not loaded into this workstation's DSH
  configuration; it has no pre-edit hold and cannot apply a Stop send-back. It is optional and the
  generic core does not depend on it.
- **Signing.** There are no Authenticode or Apple notarization credentials here. The archives are
  built and checksummed but unsigned. A native signed installer per OS remains an external
  dependency.
- **The tool's own release.** The release check's tag test cannot pass before the human creates the
  tag, so the pre-tag packet correctly reads FAILED for `release:tag-missing`. A CLEAR TO SHIP
  packet also needs the human's command approval and a configured cross-vendor desk provider in
  the ledger it is computed from; CI has neither. The workflow therefore builds a draft release,
  and the human runs the release lane on the tag, attaches the packet and publishes.
- **macOS.** The macOS artifacts are built by CI; no local macOS build was claimed.

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Historical rubric sources may seed the
packaged desk rubrics; their attribution lives in `NOTICE`.

## Inside an editor

The page is a local web app, so it also runs inside an editor: with `vdt serve` running, open
`http://127.0.0.1:4717/` in VS Code's or Cursor's built-in Simple Browser panel. No extension is
required, and the editor and a normal browser show the same ledger and the same buttons.
