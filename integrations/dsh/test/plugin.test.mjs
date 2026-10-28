// Tests the DeepSeek Harness plugin without a harness: a fake ctx delivers the events DSH emits,
// and a stub executable records what the plugin sends to `vdt hook dsh <event>`.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const dir = mkdtempSync(join(tmpdir(), 'vdt-dsh-plugin-'));
const stub = join(dir, 'stub.cjs');
const log = join(dir, 'events.jsonl');
writeFileSync(
  stub,
  `const fs = require('node:fs');
let raw = '';
process.stdin.on('data', (c) => { raw += c; });
process.stdin.on('end', () => {
  fs.appendFileSync(process.env.FAKE_LOG, JSON.stringify({ argv: process.argv.slice(2), payload: JSON.parse(raw || '{}') }) + '\\n');
});`,
);

process.env.VDT_BIN = process.execPath;
process.env.VDT_BIN_ARGS = stub;
process.env.VDT_DSH_PROJECT = dir;
process.env.FAKE_LOG = log;

const handlers = new Map();
const ctx = { on: (event, fn) => handlers.set(event, fn) };
const { apply } = await import('../index.mjs');
apply(ctx);

const session = { id: 'dsh-test-1' };
handlers.get('session/created')(session);
handlers.get('session/event')(session, {
  type: 'user/message',
  data: { text: 'add a login page', messageId: 'm1' },
});
handlers.get('tools/result')(
  {
    agent: { session: { id: 'dsh-test-1' } },
    tool: { name: 'Bash' },
    input: { command: 'cargo test' },
    callId: 'c1',
  },
  { content: 'test result: ok' },
);
handlers.get('session/event')(session, {
  type: 'assistant/message',
  data: { text: 'Login page added; tests pass.' },
});
handlers.get('session/event')(session, { type: 'turn/end', data: { turnId: 't1' } });

async function waitForLog(count) {
  for (let i = 0; i < 200; i++) {
    if (existsSync(log)) {
      const lines = readFileSync(log, 'utf8').trim().split('\n').filter(Boolean);
      if (lines.length >= count) return lines.map((l) => JSON.parse(l));
    }
    await new Promise((r) => setTimeout(r, 25));
  }
  throw new Error('the plugin did not send the expected events');
}

test('the plugin sends the same five hook events the other hosts record', async () => {
  const events = await waitForLog(4);
  const sent = events.map((e) => e.argv.slice(-1)[0]);
  assert.deepEqual(sent, ['SessionStart', 'UserPromptSubmit', 'PostToolUse', 'Stop']);
  for (const e of events) {
    assert.deepEqual(e.argv.slice(0, 2), ['hook', 'dsh']);
    assert.equal(e.payload.session_id, 'dsh-test-1');
    assert.equal(e.payload.cwd, dir);
  }
  const prompt = events[1].payload;
  assert.equal(prompt.prompt, 'add a login page');
  assert.equal(prompt.hook_event_name, 'UserPromptSubmit');
  const tool = events[2].payload;
  assert.equal(tool.tool_name, 'Bash');
  assert.equal(tool.tool_input.command, 'cargo test');
  assert.equal(tool.tool_response, 'test result: ok');
  const stop = events[3].payload;
  assert.equal(stop.last_assistant_message, 'Login page added; tests pass.');
  assert.equal(stop.turn_id, 't1');
});
