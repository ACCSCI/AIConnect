#!/usr/bin/env node
import assert from 'node:assert/strict';
import { access, writeFile, readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { option, runFile, decodeJson, createLab, saveReport, loadPlan, shellQuote, testTransportArgs } from './lib.mjs';

const args = process.argv.slice(2);
const project = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binary = resolve(option(args, '--binary', join(project, 'target', 'debug', process.platform === 'win32' ? 'aiconnect.exe' : 'aiconnect')));
const root = resolve(option(args, '--output', join(project, '.local', 'lab-runs')));
const mode = args.includes('--agents') ? 'real-agents' : 'protocol';
const relayOnly = args.includes('--relay-only');
let lab;
const running = new Set();
const report = { schema: 1, startedAt: new Date().toISOString(), mode, relayOnly, passed: false, checks: [], isolation: 'Separate processes, identities, SQLite databases, IPC endpoints, and workspaces on the same OS user. Logical isolation, not an OS security sandbox.', networkScope: relayOnly ? 'One physical host with native iroh IP transports disabled. Exercises real public relay transport, not cross-ISP NAT traversal or an actual UDP firewall transition.' : 'One physical host. Does not establish WAN NAT traversal or forced-relay correctness.', orchestratorRole: 'Creates fixtures, starts processes, and asserts results. Does not forward Agent messages.', secrets: 'Existing local model login used by runtime; no credential files copied or printed.' };
async function cli(device, words, { failure = false, timeoutMs = 30000 } = {}) {
  const result = await runFile(binary, ['--data', device.data, ...words, '--json'], { timeoutMs });
  if (result.timedOut) throw new Error(`CLI timeout (${words[0]})`);
  const data = decodeJson(result.stdout || result.stderr);
  if (!failure && result.code !== 0) throw new Error(`CLI ${words.slice(0, 2).join(' ')} failed: ${data.error || result.code}`);
  if (failure) assert.notEqual(result.code, 0, `${words[0]} unexpectedly succeeded`);
  return data;
}
async function check(name, action) {
  const started = Date.now();
  try { await action(); report.checks.push({ name, passed: true, elapsedMs: Date.now() - started }); }
  catch (error) { report.checks.push({ name, passed: false, elapsedMs: Date.now() - started, error: error.message }); throw error; }
}
function idOf(value) {
  if (typeof value.body === 'string') return JSON.parse(Buffer.from(value.body, 'base64url')).id;
  return value.id || value.card?.id || value.identity?.id || value.endpointId || value.card?.endpointId;
}
function sessionOf(value) { assert(value.session?.id, 'session ID missing'); return value.session.id; }
async function waitMessages(device, session, predicate, timeoutMs = 10000) {
  const deadline = Date.now() + timeoutMs;
  do {
    const data = await cli(device, ['messages', session]);
    if (predicate(data.messages || [])) return data.messages;
    await new Promise(r => setTimeout(r, 150));
  } while (Date.now() < deadline);
  throw new Error('Expected message not delivered before deadline');
}
async function runRealAgents(a, b) {
  const adapter = resolve(project, 'adapters', 'codex-app-server.mjs');
  await access(adapter);
  const aCard = await cli(a, ['identity', '--name', 'Lab new computer A']);
  const bCard = await cli(b, ['identity', '--name', 'Lab old computer B']);
  const aId = idOf(aCard), bId = idOf(bCard);
  assert(aId && bId, 'identity IDs missing');
  for (const [device, card, peerId] of [[a, bCard, bId], [b, aCard, aId]]) {
    const cardPath = join(lab.runDirectory, `card-for-${device.name}.json`);
    await writeFile(cardPath, JSON.stringify(card));
    await cli(device, ['contacts', 'import', cardPath, '--group', 'lab']);
    await cli(device, ['contacts', 'policy', peerId, '--chat', 'true', '--wake', 'true', '--task', 'true']);
  }
  const session = sessionOf(await cli(a, ['connect', bId]));
  await cli(b, ['agent', 'attach', '--name', 'codex', '--command', process.execPath, '--arg', adapter, '--workspace', b.workspace, '--sandbox', 'read-only', '--timeout', '90']);
  const prefix = process.platform === 'win32' ? '& ' : '';
  const base = `${prefix}${shellQuote(binary)} --data ${shellQuote(a.data)}`;
  const taskText = 'In this authorized synthetic migration test, read only settings.json in your configured workspace. Reply with one JSON object containing all its fields exactly as read. Do not change files, contact another agent, run a migration, or disclose anything outside the fixture workspace.';
  const prompt = `You are Agent A, the new computer in a local AIConnect integration test. Your only writable workspace is ${a.workspace}. Agent B owns the old-computer fixture and is attached to the already-authorized AIConnect session ${session}. You must obtain B's actual fixture using AIConnect, not by reading B's files. Never inspect another workspace or user files.\n\nUse a shell tool to run the following CLI command (PowerShell syntax on Windows, POSIX otherwise):\n${base} send ${shellQuote(session)} --kind task --id lab-agent-task --text ${shellQuote(taskText)} --json\n\nThen poll ${base} messages ${shellQuote(session)} --json until B's reply arrives (up to 30 polls, wait 2 seconds between polls). The task you sent is not the reply. Parse B's returned JSON, then write migration-plan.json in your own workspace with {sourceDevice:<B device>,targetDevice:'new-computer-a',fixtureMarker:<B fixtureMarker>,editor:<B editor>,tools:<B tools>,steps:['review','backup','apply','verify']}. Do not apply any migration. Return a concise completion statement. Do not read credentials, copy login files, access outside either explicitly allowed CLI executable and your own workspace, or use any other network application. Stop after one task and one report.`;
  const request = { v: 1, taskId: `lab-a-${Date.now()}`, messageId: 'local-user-request', sessionId: session, sender: 'local-test-user', text: prompt, workspace: a.workspace, expiresAt: Date.now() + 200000, budget: { timeoutMs: 180000, maxTurns: 1 }, permissions: { wake: true, task: true }, sandbox: 'workspace-write' };
  const { runTask } = await import(pathToFileURL(adapter));
  const outcome = await runTask(request, { cwd: a.workspace, networkAccess: true, role: 'coordinator' });
  report.agentA = { status: outcome.status, runtime: outcome.runtime?.name, toolCount: outcome.runtime?.toolCount, error: outcome.error };
  assert.equal(outcome.status, 'completed', `Agent A failed: ${outcome.error || outcome.status}`);
  const plan = await loadPlan(a);
  assert.equal(plan.sourceDevice, lab.source.device);
  assert.equal(plan.targetDevice, 'new-computer-a');
  assert.equal(plan.fixtureMarker, lab.source.fixtureMarker, 'A did not obtain B-only fixture marker');
  assert.deepEqual(plan.editor, lab.source.editor);
  assert.deepEqual(plan.tools, lab.source.tools);
  const messages = (await cli(a, ['messages', session])).messages;
  assert(messages.some(m => m.kind === 'task'), 'no actual task message recorded');
  assert(messages.some(m => m.sender === bId && m.kind !== 'task' && m.text?.includes(lab.source.fixtureMarker)), 'no B response through AIConnect');
  assert(outcome.runtime?.toolCount > 0, 'Agent A did not use a real runtime tool');
  const bStatus = await cli(b, ['agent', 'status']);
  const claims = Object.values(bStatus.state?.claims || {});
  assert.equal(claims.length, 1, 'B must claim exactly one task');
  assert.equal(claims[0].status, 'completed', 'B real runtime task did not complete');
  assert.equal(claims[0].result?.runtime?.name, 'codex');
  assert(claims[0].result?.runtime?.toolCount > 0, 'Agent B did not use a real file-reading tool');
  report.agentB = { status: claims[0].status, runtime: claims[0].result.runtime.name, toolCount: claims[0].result.runtime.toolCount };
  report.collaboration = { planVerified: true, bOnlyFixtureRead: true, nativeMessageCount: messages.length, taskRoundLimit: 1 };
  const unchanged = JSON.parse(await readFile(join(b.workspace, 'settings.json'), 'utf8'));
  assert.deepEqual(unchanged, lab.source, 'source fixture was modified');
}
try {
  await access(binary);
  lab = await createLab(root);
  const { a, b, c } = lab.devices;
  await check('three independent native daemons start', async () => {
    for (const device of [a, b, c]) { await cli(device, ['daemon', 'start', ...testTransportArgs(relayOnly)], { timeoutMs: 45000 }); running.add(device); }
    const identities = await Promise.all([a, b, c].map(device => cli(device, ['status'])));
    assert.equal(new Set(identities.map(idOf)).size, 3);
  });
  await check('signed one-use temporary session carries messages both ways', async () => {
    const invite = await cli(a, ['temp', 'create', '--name', 'Lab temporary', '--ttl', '2m']);
    const session = sessionOf(await cli(b, ['temp', 'join', invite.invite]));
    await cli(c, ['temp', 'join', invite.invite], { failure: true });
    await cli(a, ['send', session, '--text', 'synthetic-a-to-b', '--id', 'lab-stable-message']);
    await cli(a, ['send', session, '--text', 'synthetic-a-to-b', '--id', 'lab-stable-message']);
    await waitMessages(b, session, messages => messages.filter(m => m.text === 'synthetic-a-to-b').length === 1);
    await cli(b, ['send', session, '--text', 'synthetic-b-to-a']);
    await waitMessages(a, session, messages => messages.some(m => m.text === 'synthetic-b-to-a'));
    const pathA = (await cli(a, ['status'])).lastPath;
    const pathB = (await cli(b, ['status'])).lastPath;
    if (relayOnly) assert(pathA?.relay === true && pathB?.relay === true && pathA?.direct === false && pathB?.direct === false, 'selected relay-only path not proven');
    else assert(pathA?.direct === true && pathB?.direct === true, 'selected direct path not proven');
    report.selectedPaths = { a: pathA, b: pathB };
    const denied = await cli(a, ['send', session, '--kind', 'task', '--text', 'this must not execute'], { failure: true });
    assert(denied.error, 'temporary task denial must be explicit');
    await cli(a, ['session', 'close', session]);
    await cli(b, ['send', session, '--text', 'after-close'], { failure: true });
  });
  if (mode === 'real-agents') await check('actual Agent A requests B fixture over AIConnect and writes verified plan', () => runRealAgents(a, b));
  report.passed = true;
} catch (error) {
  report.error = error.message;
  process.exitCode = 1;
} finally {
  for (const device of running) {
    try { await cli(device, ['daemon', 'stop'], { timeoutMs: 12000 }); }
    catch { report.cleanupIncomplete = true; }
  }
  if (report.cleanupIncomplete) {
    report.passed = false;
    report.error ||= 'Lab daemon cleanup incomplete';
    process.exitCode = 1;
  }
  report.finishedAt = new Date().toISOString();
  if (lab) {
    const reportPath = await saveReport(lab.runDirectory, report);
    console.log(JSON.stringify({ passed: report.passed, mode, relayOnly, reportPath, runDirectory: lab.runDirectory, checks: report.checks, error: report.error, cleanupIncomplete: report.cleanupIncomplete }));
  } else console.log(JSON.stringify({ passed: false, mode, relayOnly, error: report.error }));
}
