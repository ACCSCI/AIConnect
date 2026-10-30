import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, unlink } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { runFile, decodeJson } from '../lab/lib.mjs';

const binary = resolve(process.env.AICONNECT_TEST_BINARY || `target/debug/aiconnect${process.platform === 'win32' ? '.exe' : ''}`);
const fixture = resolve('audit/fixtures/runtime-marker-adapter.mjs');
const root = resolve('.local/runtime-policy-audit');
await mkdir(root, { recursive: true });
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(action, label, ms = 12000) {
  const deadline = Date.now() + ms;
  do { const value = await action(); if (value) return value; await delay(75); } while (Date.now() < deadline);
  throw new Error(`audit_deadline:${label}`);
}
async function cli(device, words) {
  const outcome = await runFile(binary, ['--data', device.data, ...words, '--json'], { timeoutMs: 35000 });
  assert.equal(outcome.timedOut, false, `${words[0]} timeout`);
  const result = decodeJson(outcome.stdout || outcome.stderr);
  assert.equal(outcome.code, 0, `${words.join(' ')} failed: ${JSON.stringify(result)}`);
  return result;
}
function cardId(card) { return JSON.parse(Buffer.from(card.body, 'base64url')).id; }
async function boot() {
  const directory = await mkdtemp(join(root, 'run-'));
  const a = { data: join(directory, 'a'), workspace: join(directory, 'workspace-a') };
  const b = { data: join(directory, 'b'), workspace: join(directory, 'workspace-b') };
  for (const device of [a, b]) await mkdir(device.workspace, { recursive: true });
  try {
    for (const device of [a, b]) await cli(device, ['setup', '--direct-only']);
    const cards = [await cli(a, ['identity']), await cli(b, ['identity'])];
    a.id = cardId(cards[0]); b.id = cardId(cards[1]);
    for (const [device, card, id] of [[a, cards[1], b.id], [b, cards[0], a.id]]) {
      const path = join(directory, `${id}.json`); await writeFile(path, JSON.stringify(card));
      await cli(device, ['contacts', 'import', path]);
      await cli(device, ['contacts', 'policy', id, '--chat', 'true', '--wake', 'true', '--task', 'true']);
    }
    const session = (await cli(a, ['connect', b.id])).session.id;
    return { directory, a, b, session, async close() { for (const device of [a, b]) { try { await cli(device, ['daemon', 'stop']); } catch {} } } };
  } catch (error) {
    for (const device of [a, b]) { try { await cli(device, ['daemon', 'stop']); } catch {} }
    throw error;
  }
}
async function executions(b) {
  try { return (await readFile(join(b.workspace, 'executions.jsonl'), 'utf8')).trim().split('\n').filter(Boolean).map(JSON.parse); }
  catch (error) { if (error.code === 'ENOENT') return []; throw error; }
}
async function attach(b) {
  await cli(b, ['agent', 'attach', '--name', 'audit', '--command', process.execPath, '--arg', fixture, '--workspace', b.workspace, '--timeout', '20']);
}
async function policy(b, peer, enabled) { await cli(b, ['contacts', 'policy', peer, '--wake', String(enabled), '--task', String(enabled)]); }
async function send(a, session, id, text, kind = 'task') {
  const result = await cli(a, ['send', session, '--kind', kind, '--id', id, '--text', text]);
  assert.equal(result.status, 'acked', `message ${id} not delivered`);
}
async function claim(b, id, status) {
  return waitFor(async () => { const value = (await cli(b, ['agent', 'status'])).state?.claims?.[id]; return value?.status === status && value; }, `${id}:${status}`);
}
async function refreshSession(a, b, directory) {
  const path = join(directory, 'refreshed-b.json'); await writeFile(path, JSON.stringify(await cli(b, ['identity'])));
  await cli(a, ['contacts', 'import', path]);
  return (await cli(a, ['connect', b.id])).session.id;
}

test('native runtime revalidates policy, advances denied queue and never replays duplicate tasks after restart', { timeout: 180000 }, async t => {
  const lab = await boot();
  const { a, b, session } = lab;
  try {
    await t.test('queued pre-attachment task and chat do not execute', async () => {
      await send(a, session, 'before-attach', 'SHOULD_NOT_RUN');
      await policy(b, a.id, false);
      await attach(b);
      await send(a, session, 'chat-only', 'Plain text must never execute', 'text');
      await delay(900);
      assert.equal((await executions(b)).length, 0);
    });
    await t.test('policy revoked after receipt is rechecked before queued execution', async () => {
      await policy(b, a.id, true);
      await send(a, session, 'hold-first', 'HOLD');
      await waitFor(async () => (await executions(b)).some(x => x.messageId === 'hold-first'), 'hold started');
      await send(a, session, 'revoke-queued', 'SHOULD_NOT_RUN');
      await policy(b, a.id, false);
      await writeFile(join(b.workspace, 'release-hold'), 'release');
      await claim(b, 'revoke-queued', 'denied');
      assert.deepEqual((await executions(b)).map(x => x.messageId), ['hold-first']);
    });
    await t.test('later authorized task is not starved by a denied queued task', async () => {
      await policy(b, a.id, true);
      await send(a, session, 'allowed-after-denial', 'RUN');
      await claim(b, 'allowed-after-denial', 'completed');
      assert.deepEqual((await executions(b)).map(x => x.messageId), ['hold-first', 'allowed-after-denial']);
    });
    await t.test('duplicate transport ID executes once and durable claims survive receiver restart', async () => {
      await send(a, session, 'allowed-after-denial', 'RUN');
      await delay(650);
      assert.equal((await executions(b)).filter(x => x.messageId === 'allowed-after-denial').length, 1);
      await cli(b, ['daemon', 'stop']);
      await cli(b, ['daemon', 'start', '--direct-only']);
      await delay(800);
      assert.equal((await executions(b)).length, 2);
      assert.equal((await cli(b, ['agent', 'status'])).state.claims['allowed-after-denial'].status, 'completed');
    });
    await t.test('corrupt claim journal disables execution while normal messaging remains operational', async () => {
      await cli(b, ['daemon', 'stop']);
      await writeFile(join(b.data, 'agent-state.json'), '{corrupted');
      await cli(b, ['daemon', 'start', '--direct-only']);
      const current = await refreshSession(a, b, lab.directory);
      await send(a, current, 'corrupt-journal-task', 'SHOULD_NOT_RUN');
      await delay(900);
      assert.equal((await executions(b)).length, 2);
      assert.equal((await cli(b, ['inbox'])).messages.some(x => x.id === 'corrupt-journal-task'), true);
    });
    await t.test('missing initialized claim journal also fails closed', async () => {
      await cli(b, ['daemon', 'stop']);
      await unlink(join(b.data, 'agent-state.json'));
      await cli(b, ['daemon', 'start', '--direct-only']);
      const current = await refreshSession(a, b, lab.directory);
      await send(a, current, 'missing-journal-task', 'SHOULD_NOT_RUN');
      await delay(900);
      assert.equal((await executions(b)).length, 2);
      assert.equal((await cli(b, ['inbox'])).messages.some(x => x.id === 'missing-journal-task'), true);
      const log = await readFile(join(b.data, 'daemon.log'), 'utf8');
      assert.match(log, /agent_runtime_disabled/);
    });
    await writeFile(join(lab.directory, 'evidence.json'), JSON.stringify({ passed: true, binary, at: new Date().toISOString(), checks: ['text-not-executable', 'no-history-replay-on-attach', 'fresh-policy-before-execution', 'denied-task-no-starvation', 'duplicate-one-execution', 'restart-no-replay', 'corrupt-journal-fail-closed', 'missing-journal-fail-closed'], sideEffects: (await executions(b)).length, isolation: 'independent native profiles and fixture workspaces; same OS user' }, null, 2));
  } finally { await lab.close(); }
});
