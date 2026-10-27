# Verdict

**Status: 1.0.0 release candidate. Phases 1–5 are built.** What each piece can prove is below;
[docs/HOSTS.md](docs/HOSTS.md) says which host adapters have been recorded against a live host, and
[docs/RELEASE.md](docs/RELEASE.md) is the release procedure.

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
| DeepSeek Harness integration (`vdt mcp`, `vdt hook dsh`) | built; MCP is the supported path, no live session recorded ([docs/DSH.md](docs/DSH.md)) |
| MCP server (`vdt mcp`): `verdict_check`, `verdict_scan`, `verdict_claim`, `verdict_done_when`, `verdict_decisions` | built |
| Deterministic checkers: wiring, baseline_diff, regressions, tests_weakened, claims_vs_code, unfinished, exec, mutation, firstrun, release | built, each with red and green fixtures |
| Tree-sitter extractors: FastAPI, SQLAlchemy/asyncpg, Alembic, Typer, Python and TypeScript MCP tools, TypeScript fetch/openapi-ts/TanStack/Kysely, Tauri, React | built |
| `vdt scan` (whole tree, no plan, writes nothing) | built |
| Verdict computation, lanes, pre-commit gate, commit anyway, decisions queue | built |
| Localhost page (`vdt serve`, port 4717) | built: sessions, reports, claims and evidence, unplanned work, record log, decisions, mode, pause, per-host install, packet/SARIF download |
| LLM desks: seven packaged rubrics, provider dispatch, citations, hypotheses, five-finding cap | built |
| Cross-vendor routing, refusal, global warn-only (`single_vendor`) and per-desk receipts | built |
| Enforce mode: pre-edit hold and Stop-hook send-backs (at most two), then release and tell the human | built (Claude Code verified; other hosts' output shapes unverified) |
| `vdt generate` (skills + hook registrations from the policy, byte-identical) | built |
| `vdt export --sarif` and `--packet` | built |
| Release lane, installers, clean-container and two-machine checks | built; the clean-container and two-machine runs are CI jobs that execute on a pushed revision ([docs/RELEASE.md](docs/RELEASE.md)) |

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
vdt check --checkers a,b        run named deterministic checkers
vdt scan [--project DIR]        whole-tree scan, writes nothing
vdt verdict [--session ID] [--run]
vdt desks [--session ID] [--lane L]   run the lane's LLM desks, one receipt each
vdt generate [--out DIR]        emit skills and hook registrations from the policy
vdt export --sarif|--packet [--out FILE]
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
answer (`{findings: [...], hypotheses: [...], usage: {...}}`) on stdout. Any model, gateway or
routing tool can be a provider; none is required. Above the cross-vendor lane a desk is dispatched
only to a provider whose vendor differs from the builder's; with none available the run is refused
and recorded, unless the **global** policy sets `vendors.single_vendor: "warn"`, in which case the
receipt says `single_vendor`. Every finding must cite a ledger record (`{"record": N}`) or a
current `file:line`; a citation that does not resolve makes the answer a hypothesis.

## Install

From a release archive:

- Windows: `powershell -ExecutionPolicy Bypass -File install.ps1`
- Linux/macOS: `sh install.sh`

Each verifies the archive's adjacent `.sha256`, installs `vdt`, and opens the page at
`http://127.0.0.1:4717/`. After that every step is a button on that page; no terminal is needed.
The archives are unsigned unless the release says otherwise; checksums and rollback are in
[docs/RELEASE.md](docs/RELEASE.md).

## Hosts and docs

- [docs/HOSTS.md](docs/HOSTS.md): which adapters have been recorded against a live host, and what
  is still unverified.
- [docs/DSH.md](docs/DSH.md): the optional DeepSeek Harness integration (MCP connection and hook
  command).
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

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). Historical rubric sources may seed the
packaged desk rubrics; their attribution lives in `NOTICE`.
