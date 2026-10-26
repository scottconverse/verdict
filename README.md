# Verdict

**Status: pre-alpha. Not usable yet.** This repository only reserves the layout. There is no check logic.

Verdict is one open-source tool that verifies work done by AI coding agents.

## The one rule

**The agent writes claims, never verdicts.**

Claims go into an append-only evidence ledger. Verdicts come out of the ledger from things that are not the builder: deterministic checkers, a model from a different vendor, or the human.

## Names

- Product and repository: Verdict
- Crate: `verdict-gate`
- Command: `vdt`

## Design

The full design is in [docs/design.md](docs/design.md).

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
