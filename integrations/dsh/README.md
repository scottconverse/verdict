# Verdict for DeepSeek Harness (optional)

This folder holds the native DeepSeek Harness plugin. It records a harness session in Verdict's
ledger through the same `vdt hook dsh <event>` adapter the other hosts use: session start, the
user's message, tool results, and the turn end with the assistant's last message. It never blocks,
never writes a verdict, and ignores every error (fail-open).

## Register it

The harness owns `cordis.patch.yml`; add these two entries to its `insert:` list (the MCP server and
the native plugin):

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
  - id: verdict-hooks
    name: file:///C:/path/to/verdict/integrations/dsh/index.mjs
```

Environment for the plugin process (the harness inherits its environment):

- `VDT_BIN` — the `vdt` executable; default `vdt` on `PATH`.
- `VDT_DSH_PROJECT` — the project whose ledger is written; default the process's working folder.
- `VDT_BIN_ARGS` — optional extra arguments before `hook` (the tests use it to run a stub).

## Verify it

```text
node --test integrations/dsh/test/plugin.test.mjs
```

The test delivers the harness's real event shapes (`session/created`, `session/event` with
`user/message`, `assistant/message`, `turn/end`, and `tools/result`) to the plugin with a stub
executable and asserts the four hook events and their payloads. A live run against the real `vdt`
binary was also performed: the ledger recorded a session, prompt, tool call and final claim, and
`vdt ledger verify` was intact.

## Status

- Built and tested here: the plugin, the MCP entry, and the hook adapter (five-host parity).
- Not done here: loading it into the running DSH process. This workstation's DSH configuration is
  preserved by instruction, so the entries above are the installation step, not something Verdict
  edited. DSH's plugin API is the `apply(ctx)` / `ctx.on(...)` contract used by the installed
  packages; the plugin only uses events those packages already subscribe to (`session/created`,
  `session/event`, `tools/result`).
