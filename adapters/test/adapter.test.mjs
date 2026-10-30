import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { runTask, validateTask } from '../codex-app-server.mjs';
const workspace = await mkdtemp(join(tmpdir(), 'aiconnect-adapter-'));
const base = () => ({ v: 1, taskId: 'task-1', messageId: 'message-1', sessionId: 'session-1', sender: 'peer-1', text: 'Return the marker.', workspace, expiresAt: Date.now() + 10000, budget: { timeoutMs: 5000, maxTurns: 1 }, permissions: { wake: true, task: true }, sandbox: 'read-only' });
const launch = { command: process.execPath, args: [resolve('adapters/test/fake-app-server.mjs')] };
test('reject unauthorized, expired, workspace mismatch and unsafe budget/sandbox before runtime launch', async () => {
  for (const [patch, expected] of [[{ permissions: { wake: true, task: false } }, 'task_not_authorized'], [{ expiresAt: 1 }, 'task_expired'], [{ sandbox: 'danger-full-access' }, 'unsupported_sandbox'], [{ budget: { timeoutMs: 300000, maxTurns: 1 } }, 'invalid_task_budget'], [{ workspace: process.cwd() }, 'workspace_mismatch']]) {
    const result = await runTask({ ...base(), ...patch }, { cwd: workspace, launch });
    assert.equal(result.error, expected);
    assert.equal(result.runtime.threadId, undefined);
  }
});
test('one-shot JSONL protocol uses completed reply, not streamed duplicate', async () => {
  const result = await runTask(base(), { cwd: workspace, launch });
  assert.equal(result.status, 'completed'); assert.equal(result.text, 'FAKE_REPLY'); assert.equal(result.runtime.turnId, 'fake-turn');
});
test('approval requests are denied and task fails closed', async () => {
  process.env.AICONNECT_FAKE_MODE = 'approval';
  try { assert.equal((await runTask(base(), { cwd: workspace, launch })).error, 'runtime_approval_required'); }
  finally { delete process.env.AICONNECT_FAKE_MODE; }
});
test('malformed runtime packets and runtime turn errors fail without leaking diagnostics', async () => {
  for (const mode of ['bad-json', 'failed']) {
    process.env.AICONNECT_FAKE_MODE = mode;
    try { const r = await runTask(base(), { cwd: workspace, launch }); assert.equal(r.status, 'failed'); assert.match(r.error, /^runtime_/); }
    finally { delete process.env.AICONNECT_FAKE_MODE; }
  }
});
test('deadline ends hung runtime', async () => {
  process.env.AICONNECT_FAKE_MODE = 'hang';
  try { const task = base(); task.budget.timeoutMs = 1000; assert.equal((await runTask(task, { cwd: workspace, launch })).error, 'task_deadline_exceeded'); }
  finally { delete process.env.AICONNECT_FAKE_MODE; }
});
