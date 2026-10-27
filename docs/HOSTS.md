# Host adapters: what is verified and what is not

Verdict's adapters are optional. Each implements the same three-job contract (capture the prompt,
record tool calls, hold the "done" moment) against that host's own hook payload. This file is the
honest status of each one. A host whose real session has not been recorded is labeled, so nobody
reads a fixture as a live proof.

| Host | Registration file | Real session recorded | Payload fields verified | Notes |
| --- | --- | --- | --- | --- |
| Claude Code | `.claude/settings.json` (project) | **Yes** — Claude Code 2.1.296, Oct 27 2026 | Yes (SessionStart, UserPromptSubmit, PostToolUse, Stop captured verbatim) | The payload does **not** carry the model on any event in this version, so the session's model reads `unknown`; the vendor is `anthropic`. `tool_response` for a read is an object with `type`/`file`, for a shell it has `stdout`/`stderr`. |
| Codex | `$CODEX_HOME/hooks.json` (usually `~/.codex/hooks.json`) | **Yes** — codex-cli 0.162.0-alpha.2, Oct 27 2026 | Yes for the fields used (session id, turn id, model, prompt, tool name/input, final message) | Codex requires the hooks to be trusted (`/hooks`) or the run to pass `--dangerously-bypass-hook-trust`. The session recorded `model: gpt-6.1-sol`, `vendor: openai`. |
| Cursor | `.cursor/hooks.json` (project) | **No** | **No** — the file schema is from Cursor's documented hooks page (`version: 1`, flat per-event entries, `command`/`matcher`/`timeout`), but the payload field names are read tolerantly and are not checked against a live session | Events registered: `sessionStart`, `beforeSubmitPrompt`, `preToolUse`, `postToolUse`, `stop`. If the live payload uses other names, the adapter records `check_unavailable` rather than an invented record. |
| Antigravity | `.agents/hooks.json` (project) | **No** | **No** — the file schema (named groups, `SessionStart`/`PreInvocation`/`PreToolUse`/`PostToolUse`/`Stop`) is from Antigravity's documented hooks page; the payload field names are unverified | Antigravity has no prompt-submit event, so the prompt is captured at `PreInvocation`. Tool names `run_command`, `write_to_file`, `replace_file_content` and `multi_replace_file_content` are mapped to the host-neutral names. |

## What a fixture proves, and what it does not

A recorded-payload test proves that a payload of the documented shape becomes the same ledger
records across hosts (the parity test). It does not prove the host sends that shape. For Claude
Code and Codex the payloads came from live sessions and are quoted in `fixtures/`; for Cursor and
Antigravity the fixtures are written from the documented schemas and are labeled unverified here.

## Installing and removing

`vdt hooks install <host> --i-am-the-human` merges the entries and keeps everything else; the page's
Install button does the same. Uninstalling removes only entries whose command is exactly
`<program> hook <host> <event>`, and restores a backup otherwise. Cursor's flat file and
Antigravity's named `verdict` group are handled separately, because writing one shape into the
other would corrupt the file.
