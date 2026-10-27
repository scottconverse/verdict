# Verdict

**Status: Phases 1–3 are built and merged. Phase 4 (LLM desks, cross-vendor routing, `vdt generate`,
Cursor and Antigravity adapters) is in progress. Phase 5 (full GUI, enforce mode, release lane,
installer) is not built.** The sections below say exactly what runs today.

Verdict is one open-source tool that verifies work done by AI coding agents.

## The one rule

**The agent writes claims, never verdicts.**

Claims go into an append-only evidence ledger. Verdicts come out of the ledger from things that are
not the builder: deterministic checkers, a model from a different vendor, or the human.

## What runs today

| Piece | State |
| --- | --- |
| Evidence ledger (SQLite, blake3 hash chain, append-only, JSONL export) | built |
| Claude Code and Codex hook adapters (`vdt hook`, `vdt hooks install`) | built; real-host recording is verified in the phase notes |
| MCP server (`vdt mcp`): `verdict_check`, `verdict_scan`, `verdict_claim`, `verdict_done_when`, `verdict_decisions` | built |
| Deterministic checkers: wiring, baseline_diff, regressions, tests_weakened, claims_vs_code, unfinished, exec | built, each with red and green fixtures |
| Tree-sitter extractors: FastAPI, SQLAlchemy/asyncpg, Alembic, Typer, Python and TypeScript MCP tools, TypeScript fetch/openapi-ts/TanStack/Kysely, Tauri, React | built |
| `vdt scan` (whole tree, no plan, writes nothing) | built |
| Verdict computation, lanes, pre-commit gate, commit anyway, decisions queue | built |
| Localhost page (`vdt serve`, port 4717) | built: sessions, reports, record log, decisions, mode, pause |
| LLM desks, cross-vendor providers, `vdt generate` | Phase 4, in progress |
| Enforce-mode send-backs, release lane, SARIF/packet export, installer | Phase 5, not built |

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
