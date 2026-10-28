// Verdict's optional DeepSeek Harness plugin. It records the harness's session, user messages,
// tool results and turn ends into the Verdict ledger by calling the same `vdt hook dsh <event>`
// adapter the other hosts use. It never blocks, never writes a verdict and never edits harness
// configuration; a missing `vdt`, a crash or a slow hook is ignored (fail-open).
//
// Register it from the harness's own `cordis.patch.yml` (the harness owns that file):
//
//   - insert:
//     - id: verdict-hooks
//       name: file:///C:/path/to/verdict/integrations/dsh/index.mjs
//
// Environment:
//   VDT_BIN           the vdt executable (default: `vdt` on PATH)
//   VDT_DSH_PROJECT   the project whose ledger is written (default: process.cwd())
//   VDT_BIN_ARGS      optional extra arguments before `hook` (tests use it to run a stub)

import { spawn } from 'node:child_process';

export const name = 'verdict-hooks';

const lastAssistant = new Map();
const MAX_TRACKED = 64;

function textOf(value) {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  if (Array.isArray(value)) return value.map(textOf).join('');
  if (typeof value !== 'object') return '';
  if (typeof value.text === 'string') return value.text;
  if (typeof value.content === 'string') return value.content;
  if (Array.isArray(value.content)) return textOf(value.content);
  if (value.message !== undefined) return textOf(value.message);
  return '';
}

function remembered(map, key, value) {
  map.set(key, value);
  while (map.size > MAX_TRACKED) map.delete(map.keys().next().value);
}

export function apply(ctx) {
  const bin = process.env.VDT_BIN || 'vdt';
  const project = process.env.VDT_DSH_PROJECT || process.cwd();
  const extra = (process.env.VDT_BIN_ARGS || '').split(/\s+/).filter(Boolean);

  function send(event, payload) {
    let child;
    try {
      child = spawn(bin, [...extra, 'hook', 'dsh', event], {
        cwd: project,
        env: process.env,
        stdio: ['pipe', 'ignore', 'ignore'],
        windowsHide: true,
      });
    } catch {
      return;
    }
    child.on('error', () => {});
    const timer = setTimeout(() => {
      try { child.kill(); } catch {}
    }, 5000);
    timer.unref?.();
    child.on('exit', () => clearTimeout(timer));
    try {
      child.stdin.end(JSON.stringify(payload));
    } catch {}
  }

  ctx.on('session/created', (session) => {
    const id = session?.id ?? session?.sessionId ?? session;
    if (id === undefined) return;
    const model = session?.model?.id ?? session?.model?.name ?? session?.model ?? session?.modelId;
    send('SessionStart', {
      session_id: String(id),
      cwd: project,
      hook_event_name: 'SessionStart',
      ...(typeof model === 'string' && model ? { model } : {}),
    });
  });

  ctx.on('session/event', (session, event) => {
    if (!event || typeof event.type !== 'string') return;
    const id = session?.id ?? session?.sessionId ?? session;
    if (id === undefined) return;
    const sessionId = String(id);
    if (event.type === 'user/message') {
      const prompt = textOf(event.data);
      if (!prompt) return;
      send('UserPromptSubmit', {
        session_id: sessionId,
        cwd: project,
        hook_event_name: 'UserPromptSubmit',
        prompt,
        prompt_id: event.data?.messageId ?? event.data?.id,
      });
    } else if (event.type === 'assistant/message') {
      const text = textOf(event.data);
      if (text) remembered(lastAssistant, sessionId, text);
    } else if (event.type === 'turn/end') {
      send('Stop', {
        session_id: sessionId,
        cwd: project,
        hook_event_name: 'Stop',
        last_assistant_message: lastAssistant.get(sessionId) ?? '',
        turn_id: event.data?.turnId ?? event.data?.id,
      });
    }
  });

  ctx.on('tools/result', (exec, result) => {
    if (!exec) return;
    const id = exec.agent?.session?.id ?? exec.agent?.session ?? exec.session?.id ?? exec.session;
    if (id === undefined) return;
    send('PostToolUse', {
      session_id: String(id),
      cwd: project,
      hook_event_name: 'PostToolUse',
      tool_name: exec.tool?.name ?? exec.toolName ?? exec.name ?? 'tool',
      tool_input: exec.input ?? exec.args ?? {},
      tool_response: result?.content ?? result?.output ?? result ?? '',
      tool_use_id: exec.callId ?? exec.token,
      duration_ms: result?.durationMs ?? result?.duration_ms,
    });
  });
}
