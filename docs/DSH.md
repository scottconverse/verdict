# DeepSeek Harness (DSH): the optional integration

DSH is one of Verdict's optional hosts. Nothing in the deterministic core needs it, and Verdict
never writes DSH's configuration. There are two integration paths.

## 1. MCP connection (works today)

`vdt mcp` is a stdio MCP server. Register it with DSH the way DSH registers any MCP server; the
command and arguments are:

```json
{
  "command": "vdt",
  "args": ["mcp", "--project", "<the project folder>"]
}
```

The five tools then appear: `verdict_check`, `verdict_scan`, `verdict_claim`, `verdict_done_when`
and `verdict_decisions`. They are read/claim only: an attempt to write a finding, decision, verdict,
mode or commit-anyway is refused by the ledger and recorded as a `refused_write`. `verdict_check`
returns the same report the GUI and `vdt report` show, with `VERDICT:` and `NOT ESTABLISHED` in it.

A session has to exist before `verdict_claim` will record anything, and the MCP server does not
open one (decision 13 in `DECISIONS.md`). Until a DSH-native session adapter exists, start one from
the project with `vdt check` (it opens a session), or from another host's hook.

## 2. Command hook (for a harness version that can call one)

```text
vdt hook dsh <SessionStart|UserPromptSubmit|PreToolUse|PostToolUse|Stop>
```

reads one JSON payload on stdin, the same host-neutral contract the other adapters use
(`session_id`, `cwd`, `hook_event_name`, `prompt`, `tool_name`, `tool_input`, `tool_response`,
`last_assistant_message`, `model`), and records it in the ledger. The parity test proves a DSH
payload of that shape produces the same ledger records as Claude Code, Codex, Cursor and
Antigravity. DSH's session vendor is derived from the model it served, because the harness is not a
vendor.

`vdt hooks install dsh` is refused on purpose: DSH keeps its own configuration, and guessing a
path or schema would be worse than saying so. Point the harness's own hook configuration at the
command above.

## What is verified, and what is not

- **Verified here:** the MCP server speaks the protocol and its five tools behave as described
  (`crates/verdict-cli/tests/mcp.rs`); the DSH hook adapter parses the host-neutral payload and
  reaches parity with the other four hosts (`crates/verdict-adapters/tests/hooks.rs`).
- **Not verified here:** a live DSH session recording through either path. The installed DSH in
  this environment exposes no plugin manifest or hook API this build could target, so no native
  plugin package is claimed. That is a host limitation, not a Verdict default: the MCP connection
  above is the supported path and needs no plugin.
