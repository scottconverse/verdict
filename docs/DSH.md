# DeepSeek Harness (DSH): the optional integration

DSH is one of Verdict's optional hosts. Nothing in the deterministic core needs it, and Verdict
never writes DSH's configuration. There are two integration paths, both shipped in this repo.

## 1. MCP connection

`vdt mcp` is a stdio MCP server. Add it to `cordis.patch.yml` the way the harness registers any MCP
server (`vdt mcp-install dsh` prints this exact entry):

```yaml
- insert:
  - id: mcp-verdict
    name: '@deepseek-ai/dsh-mcp-client'
    config:
      serverName: verdict
      transport: stdio
      command: C:/path/to/vdt.exe
      args:
      - mcp
      - --project
      - C:/path/to/the/project
      failOnStartupError: false
```

The five tools then appear: `verdict_check`, `verdict_scan`, `verdict_claim`, `verdict_done_when`
and `verdict_decisions`. They are read/claim only: an attempt to write a finding, decision, verdict,
mode or commit-anyway is refused by the ledger and recorded as a `refused_write`. `verdict_check`
runs the lane's checkers and desks now (the same fresh verification the Stop hook runs) and returns
the same report the GUI and `vdt report` show, with `VERDICT:` and `NOT ESTABLISHED` in it.

A session has to exist before `verdict_claim` will record anything; the MCP server does not open one
(decision 13 in `DECISIONS.md`). The native plugin below opens it.

## 2. Native plugin

`integrations/dsh/index.mjs` is a Cordis plugin (`export const name`, `export function apply(ctx)`)
that subscribes to the harness events the installed packages already use — `session/created`,
`session/event` (`user/message`, `assistant/message`, `turn/end`) and `tools/result` — and turns
them into `vdt hook dsh <event>` calls. Registration is one more `insert:` entry:

```yaml
  - id: verdict-hooks
    name: file:///C:/path/to/verdict/integrations/dsh/index.mjs
```

Environment: `VDT_BIN`, `VDT_DSH_PROJECT`, and the optional `VDT_BIN_ARGS`. A missing binary, a
crash or a slow hook is ignored; the agent is never held.

**The plugin is recording-only.** It captures prompts, tool results and the turn end. It has no
pre-edit hold, and this build does not send a Stop send-back to DSH (the reply shape is not
claimed). For enforce-mode holds, use Claude Code or Codex; for the other hosts the ledger still
records what happened, which is what the verdict reads.

## What is verified, and what is not

- **Verified here:** the MCP server speaks the protocol and its five tools behave as described
  (`crates/verdict-cli/tests/mcp.rs`); the hook adapter reaches parity with the other four hosts
  (`crates/verdict-adapters/tests/hooks.rs`); the native plugin maps the harness's event shapes to
  the four hook payloads (`node --test integrations/dsh/test/plugin.test.mjs`), and a live run against the real `vdt`
  binary recorded a session, prompt, tool call and final claim into an intact ledger.
- **Not done here:** loading the entries into the running DSH instance. This workstation's DSH
  configuration is preserved by instruction, so the YAML above is the installation step, not
  something Verdict did. The plugin is recording-only (no pre-edit hold, no Stop send-back); no
  part of the integration requires a Verdict-specific DSH build. The integration is included in
  the release archive under `integrations/dsh/`.
