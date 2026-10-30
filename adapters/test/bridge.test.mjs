import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';

const workspace = await mkdtemp(join(tmpdir(), 'aiconnect-bridge-'));
const executable = resolve('adapters/codex-app-server.mjs');
const fake = resolve('adapters/test/fake-app-server.mjs');
const request = () => ({ v: 1, taskId: 'bridge-task', messageId: 'bridge-message', sessionId: 'bridge-session', sender: 'bridge-peer', workspace, text: 'Return marker.', expiresAt: Date.now() + 10000, budget: { timeoutMs: 5000, maxTurns: 1 }, permissions: { wake: true, task: true } });
function invoke(input, { splitAt, mode } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [executable], { cwd: workspace, windowsHide: true, env: { ...process.env, AICONNECT_CODEX_BIN: fake, ...(mode ? { AICONNECT_FAKE_MODE: mode } : {}) }, stdio: ['pipe', 'pipe', 'pipe'] });
    let stdout = '', stderr = '';
    const timeout = setTimeout(() => { child.kill(); reject(new Error('bridge test deadline')); }, 10000);
    child.stdout.on('data', data => { stdout += data; }); child.stderr.on('data', data => { stderr += data; });
    child.on('error', reject);
    child.on('exit', code => { clearTimeout(timeout); resolve({ code, stdout, stderr }); });
    child.stdin.on('error', () => {});
    if (splitAt !== undefined) {
      child.stdin.write(input.subarray(0, splitAt));
      setTimeout(() => child.stdin.end(input.subarray(splitAt)), 200);
    } else child.stdin.end(input);
  });
}
test('executable bridge emits exactly one JSON line and exits successfully', async () => {
  const result = await invoke(`${JSON.stringify(request())}\n`);
  assert.equal(result.code, 0); assert.equal(result.stderr, '');
  const lines = result.stdout.trim().split('\n'); assert.equal(lines.length, 1);
  assert.equal(JSON.parse(lines[0]).text, 'FAKE_REPLY');
});
test('executable bridge rejects malformed JSON without provider startup', async () => {
  const result = await invoke('not-json\n');
  assert.equal(result.code, 1); assert.equal(JSON.parse(result.stdout).error, 'invalid_json');
});
test('executable bridge rejects oversized request', async () => {
  const result = await invoke(`${'x'.repeat(33000)}\n`);
  assert.equal(result.code, 1); assert.equal(JSON.parse(result.stdout).error, 'request_too_large');
});
test('executable bridge preserves Chinese request split inside a UTF-8 character', async () => {
  const task = request(); task.text = '临时连接：两个 Agent 正确交换中文。';
  const bytes = Buffer.from(`${JSON.stringify(task)}\n`, 'utf8');
  const splitAt = bytes.indexOf(Buffer.from('临', 'utf8')) + 1;
  assert(splitAt > 0);
  const result = await invoke(bytes, { splitAt, mode: 'echo' });
  assert.equal(result.code, 0);
  assert.equal(JSON.parse(result.stdout).text, task.text);
});
