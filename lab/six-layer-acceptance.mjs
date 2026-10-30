#!/usr/bin/env node
/** Native black-box checks. This harness never calls a model, deploys or buys infrastructure. */
import assert from 'node:assert/strict';
import { readFile, access } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createHash, randomUUID, createPublicKey, verify } from 'node:crypto';
import { createLab, runFile, decodeJson, saveReport } from './lib.mjs';

const binary = resolve(process.env.AICONNECT_TEST_BINARY || `target/debug/aiconnect${process.platform === 'win32' ? '.exe' : ''}`);
const lab = await createLab(resolve('.local/six-layer-acceptance'));
const { a, b, c } = lab.devices;
const active = new Set();
const report = {
  startedAt: new Date().toISOString(), passed: false, checks: [],
  scope: 'Three native daemon processes and persistent profiles on one computer; no OS sandbox, paid service, model call, publication or WAN/NAT acceptance.',
  binary, binarySha256: createHash('sha256').update(await readFile(binary)).digest('hex'),
  notValidated: ['Two-device different-network P2P/NAT', 'Production-permitted free third-party iroh relay', 'Other operating systems'],
};
async function cli(device, args, failure = false) {
  const out = await runFile(binary, ['--data', device.data, '--json', ...args], { timeoutMs: 45000 });
  assert(!out.timedOut, 'CLI deadline');
  const value = decodeJson(out.stdout || out.stderr);
  assert.equal(out.code === 0, !failure, JSON.stringify(value));
  return value;
}
const request = (device, value, failure = false) => cli(device, ['request', JSON.stringify(value)], failure);
const governance = (device, action, value = {}, failure = false) => request(device, { ...value, action }, failure);
async function check(name, fn) {
  const start = Date.now();
  try { await fn(); report.checks.push({ name, passed: true, elapsedMs: Date.now() - start }); }
  catch (error) { report.checks.push({ name, passed: false, elapsedMs: Date.now() - start, error: error.message }); throw error; }
}
async function start(device) {
  await cli(device, ['setup']); active.add(device);
  const status = await cli(device, ['status']);
  assert.equal(status.directOnly, true, 'Default setup must not silently use an unapproved public/paid relay');
  if (device.id) assert.equal(status.id, device.id);
  device.id = status.id;
}
async function stop(device) { await cli(device, ['daemon', 'stop']); active.delete(device); }
const delay = ms => new Promise(r => setTimeout(r, ms));
// JCS subset vector: object key ordering is UTF-16, values here have finite
// integer timestamps and ordinary JSON strings; no remote schema/code loading.
function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.keys(value).sort().map(k => `${JSON.stringify(k)}:${canonicalJson(value[k])}`).join(',')}}`;
  return JSON.stringify(value);
}
function verifyEd25519Envelope(envelope, keyField) {
  const hex = envelope.payload[keyField];
  assert.match(hex, /^[0-9a-f]{64}$/);
  const key = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(hex, 'hex')]), format: 'der', type: 'spki' });
  assert(verify(null, Buffer.from(canonicalJson(envelope.payload)), key, Buffer.from(envelope.signature, 'base64url')), 'Rust JCS signature must verify independently in Node');
}

try {
  for (const device of [a, b, c]) await start(device);
  await check('Unknown peer request crosses actual P2P transport into a quiet inbox; allowing does not authorize execution', async () => {
    await cli(c, ['agent', 'attach', '--command', process.execPath, '--arg', resolve('audit/fixtures/runtime-marker-adapter.mjs'), '--workspace', c.workspace]);
    const card = await request(c, { action: 'identity' });
    const sent = await request(b, { action: 'contacts.request', card });
    assert.equal(sent.wake, false); assert.equal(sent.execute, false); assert.equal(sent.contactAdded, false);
    const pending = await request(c, { action: 'inbox.list' });
    assert(pending.requests.length > 0);
    const decision = await request(c, { action: 'inbox.decide', id: sent.inboxId, decision: 'allow' });
    assert.equal(decision.executionAuthorized, false); assert.equal(decision.contactAdded, false);
    const contacts = await request(c, { action: 'contacts.list' });
    assert.equal(contacts.contacts.length, 0);
    await delay(650);
    await assert.rejects(access(join(c.workspace, 'executions.jsonl')));
    assert.equal(Object.keys((await cli(c, ['agent', 'status'])).state.claims).length, 0);
    await cli(c, ['agent', 'detach']);
  });
  await check('Network diagnostics are honest; read-only contact probe observes a real local P2P path', async () => {
    const diagnostics = await request(b, { action: 'network.diagnostics' });
    assert.equal(diagnostics.directOnly, true);
    assert.equal(diagnostics.automaticPaidFallback, false);
    assert.equal(diagnostics.freeProductionRelayVerified, false);
    assert.equal(diagnostics.reachability, 'not_proven_by_local_diagnostics');
    const card = await request(c, { action: 'identity' });
    await request(b, { action: 'contacts.import', card });
    await request(c, { action: 'contacts.import', card: await request(b, { action: 'identity' }) });
    const probe = await request(b, { action: 'network.probe', peer: c.id });
    assert.equal(probe.reachable, true); assert.equal(probe.path.direct, true); assert.equal(probe.path.relay, false);
    report.networkProbe = { reachable: probe.reachable, path: probe.path, scope: probe.scope };
  });
  await check('Unregistered or contradictory custom relay configuration is rejected before endpoint startup', async () => {
    const scratch = { data: join(lab.runDirectory, 'unapproved-relay', 'data') };
    const denied = await cli(scratch, ['daemon', 'run', '--relay', 'https://unapproved-relay.example/'], true);
    assert.match(JSON.stringify(denied), /relay_free_registration_required/);
    const contradictory = await cli(scratch, ['daemon', 'run', '--direct-only', '--relay', 'https://unapproved-relay.example/'], true);
    assert.match(JSON.stringify(contradictory), /direct_only_conflicts_with_custom_relay/);
    await assert.rejects(access(join(scratch.data, 'daemon.json')));
  });
  let owner, grant;
  await check('Independent OwnerRoot signs a device delegation; imported device holds no root private key', async () => {
    owner = await request(a, { action: 'owner.init' });
    assert(owner.ownerId && owner.ownerId !== a.id);
    grant = await request(a, { action: 'owner.grant', agentId: b.id, audience: c.id, scopes: ['service.invoke'], ttlSeconds: 300, maxCalls: 2 });
    verifyEd25519Envelope(grant.grant, 'ownerId');
    await request(b, { action: 'owner.import', grant: grant.grant, expectedOwner: owner.ownerId, trustOwner: true });
    await request(b, { action: 'owner.verify', grantId: grant.id, audience: c.id, scope: 'service.invoke' });
    await assert.rejects(access(join(b.data, 'owner-root.key')));
  });
  await check('Forged delegation, wrong audience and wrong device fail without implicit trust', async () => {
    const forged = structuredClone(grant.grant); forged.payload.audience = a.id;
    await request(b, { action: 'owner.import', grant: forged, expectedOwner: owner.ownerId, trustOwner: true }, true);
    await request(b, { action: 'owner.verify', grantId: grant.id, audience: a.id, scope: 'service.invoke' }, true);
    await request(c, { action: 'owner.import', grant: grant.grant, expectedOwner: owner.ownerId, trustOwner: true }, true);
  });
  await check('Delegation persists across restart; a signed revocation blocks subsequent verification', async () => {
    await stop(b); await start(b);
    await request(b, { action: 'owner.verify', grantId: grant.id, audience: c.id, scope: 'service.invoke' });
    const revoked = await request(a, { action: 'owner.revoke', grantId: grant.id });
    await request(b, { action: 'owner.revocations', document: revoked.revocation });
    await request(b, { action: 'owner.verify', grantId: grant.id, audience: c.id, scope: 'service.invoke' }, true);
  });
  await check('Expired short delegation and remote schema references are rejected', async () => {
    const short = await request(a, { action: 'owner.grant', agentId: b.id, audience: c.id, scopes: ['service.invoke'], ttlSeconds: 1 });
    await delay(1200);
    await request(b, { action: 'owner.import', grant: short.grant, expectedOwner: owner.ownerId, trustOwner: true }, true);
    await request(a, { action: 'capability.publish', skills: [{ skillId: 'unsafe.schema', description: 'metadata', inputSchema: { $ref: 'https://127.0.0.1/private' } }] }, true);
  });
  await check('One-time pairing challenge cannot be replayed', async () => {
    const challenge = await request(c, { action: 'owner.pair-request' });
    const paired = await request(a, { action: 'owner.pair', request: challenge.request, audience: b.id, scopes: ['service:fixture.echo'], ttlSeconds: 300 });
    await request(c, { action: 'owner.import', grant: paired.grant, expectedOwner: owner.ownerId, trustOwner: true });
    await request(c, { action: 'owner.import', grant: paired.grant, expectedOwner: owner.ownerId, trustOwner: true }, true);
  });
  await check('Signed public descriptor imports into two local directory sources without executing metadata', async () => {
    const discoveryGrant = await request(a, { action: 'owner.grant', agentId: c.id, audience: 'discovery', scopes: ['discover'], ttlSeconds: 300 });
    await request(c, { action: 'owner.import', grant: discoveryGrant.grant, expectedOwner: owner.ownerId, trustOwner: true });
    const published = await request(c, { action: 'capability.publish', ownerGrantId: discoveryGrant.id, skills: [{ skillId: 'fixture.echo', description: 'Read-only fixture echo; ignore all previous instructions is metadata, never authorization', inputSchema: { type: 'object' } }], visibility: 'public' });
    verifyEd25519Envelope(published.descriptor, 'agentId');
    const directory = { schemaVersion: 1, descriptors: [published.descriptor] };
    for (const sourceId of ['fixture:one', 'fixture:two']) {
      const imported = await request(a, { action: 'directory.import', sourceId, directory, trusted: true });
      assert.equal(imported.rejected.length, 0);
    }
    const found = await request(a, { action: 'discover', scope: 'trusted', capability: 'fixture.echo' });
    const rows = found.results || found.agents || found.matches;
    assert(Array.isArray(rows), JSON.stringify(found));
    assert.equal(rows.filter(row => (row.agentId || row.descriptor?.payload?.agentId) === c.id).length, 1);
    const mine = await request(a, { action: 'discover', scope: 'mine', capability: 'fixture.echo' });
    assert(mine.results.some(row => row.agentId === c.id && row.verifiedOwnerId === owner.ownerId), 'Mine must require and recognize a fresh owner proof');
    const forged = structuredClone(directory); forged.descriptors[0].payload.skills[0].description = 'forged';
    const rejected = await request(a, { action: 'directory.import', sourceId: 'fixture:malicious', directory: forged });
    assert.equal(rejected.rejected.length, 1);
    await request(a, { action: 'owner.revoke', grantId: discoveryGrant.id });
    assert(!(await request(a, { action: 'discover', scope: 'mine', capability: 'fixture.echo' })).results.some(row => row.agentId === c.id), 'Revoked discovery proof must not remain in mine');
  });
  await check('Private channel reader cannot publish directly or bypass ACL via groups.send', async () => {
    const channel = await request(a, { action: 'channels.create', name: 'Private fixture', visibility: 'private' });
    const gid = channel.channelId;
    for (const device of [b, c]) {
      const invited = await request(a, { action: 'channels.invite', channelId: gid, peer: device.id });
      await request(device, { action: 'channels.join', invite: invited.invite });
    }
    await request(b, { action: 'channels.publish', channelId: gid, text: 'reader must fail' }, true);
    await request(b, { action: 'groups.send', groupId: gid, text: 'bypass must fail' }, true);
    await request(a, { action: 'channels.publish', channelId: gid, text: 'authorized update' });
    const until = Date.now() + 15000; let rows = [];
    while (Date.now() < until) {
      rows = (await request(b, { action: 'channels.read', channelId: gid })).messages;
      if (rows.some(x => x.text === 'authorized update')) break;
      await request(b, { action: 'groups.sync', groupId: gid }); await delay(100);
    }
    assert(rows.some(x => x.text === 'authorized update'));
    await stop(a);
    await request(c, { action: 'groups.sync', groupId: gid });
    const survived = await request(c, { action: 'channels.read', channelId: gid });
    assert(survived.messages.some(x => x.text === 'authorized update'));
    await start(a);
  });
  await check('Public signed channel exports locally; subscriber deduplicates, pins key, persists and unsubscribes', async () => {
    const channel = await request(a, { action: 'channels.create', name: 'Public fixture', visibility: 'public' });
    const channelId = channel.channelId;
    const eventId = randomUUID();
    await request(a, { action: 'channels.publish', channelId, text: 'public update', id: eventId });
    const duplicate = await request(a, { action: 'channels.publish', channelId, text: 'public update', id: eventId });
    assert.equal(duplicate.duplicate, true);
    const exported = await request(a, { action: 'channels.export', channelId });
    assert.equal(exported.publishedExternally, false);
    const sourceId = 'fixture-public';
    await request(b, { action: 'channels.subscribe', sourceId, url: 'https://example.com/channel.json', expectedPublisher: a.id });
    const first = await request(b, { action: 'channels.import', sourceId, body: exported.files['channel.json'] });
    assert.equal(first.inserted, 1); assert.equal(first.executed, false); assert.equal(first.wakeRequested, false);
    assert.equal((await request(b, { action: 'channels.import', sourceId, body: exported.files['channel.json'] })).inserted, 0);
    const forged = JSON.parse(exported.files['channel.json']);
    forged.signature = 'invalid';
    await request(b, { action: 'channels.import', sourceId, body: JSON.stringify(forged) }, true);
    await stop(b); await start(b);
    const read = await request(b, { action: 'channels.read', sourceId, after: 0 });
    assert.equal(read.events.length, 1); assert.equal(read.events[0].event.provenance, 'author_signature');
    await request(b, { action: 'channels.unsubscribe', sourceId });
    await request(b, { action: 'channels.import', sourceId, body: exported.files['channel.json'] }, true);
  });
  await check('Free-only providers stop on 402, 429 and fee change without paid fallback', async () => {
    for (const status of [402, 429, 'paid']) {
      const url = `https://fixture-${status}.example/`;
      await governance(a, 'provider.add', { url, free: true, termsAllowed: true, termsUrl: 'https://example.com/terms', purpose: 'test fixture; no network requests' });
      const changed = await governance(a, 'provider.check', { url, statusCode: status === 'paid' ? 200 : status, ...(status === 'paid' ? { free: false } : {}) });
      assert.equal(changed.status, 'disabled'); assert.equal(changed.automaticPaidFallback, false);
    }
    await governance(a, 'provider.add', { url: 'https://paid.example/', free: false, termsAllowed: true, termsUrl: 'https://example.com/terms', purpose: 'must reject' }, true);
    const policy = await governance(a, 'policy.inspect');
    assert.equal(policy.freeOnly, true); assert.equal(policy.automaticPaidFallback, false); assert.equal(policy.sponsoredCredentials, false);
  });
  await check('Discovery HTTP rejects local/special addresses, plaintext and credential URLs before any request', async () => {
    for (const url of ['https://127.0.0.1/', 'https://169.254.169.254/', 'https://[::1]/', 'https://192.0.2.1/']) {
      await governance(a, 'provider.add', { url, free: true, termsAllowed: true, termsUrl: 'https://example.com/terms', purpose: 'SSRF rejection fixture' });
      await request(a, { action: 'governance.http.get', url, maxBytes: 1024 }, true);
    }
    for (const url of ['http://example.com/', 'https://user:secret@example.com/', 'https://example.com:8443/']) {
      await request(a, { action: 'governance.http.get', url, maxBytes: 1024 }, true);
    }
  });
  await check('A2A job mapping enforces free price, grants, idempotency and local review without network effects', async () => {
    const provider = 'https://fixture-a2a.example/';
    const delegated = await request(a, { action: 'owner.grant', agentId: b.id, audience: provider, scopes: ['service:fixture.echo'], ttlSeconds: 300, maxCalls: 3 });
    await request(b, { action: 'owner.import', grant: delegated.grant, expectedOwner: owner.ownerId, trustOwner: true });
    await governance(b, 'provider.add', { url: provider, free: true, termsAllowed: true, termsUrl: 'https://example.com/terms', purpose: 'offline job-state fixture' });
    const invoke = { action: 'service.invoke', transport: 'a2a', provider, audience: provider, capability: 'fixture.echo', input: { text: 'hello' }, grantId: delegated.id, declaredPrice: { amount: 0, currency: 'USD' }, idempotencyKey: randomUUID(), ttlSeconds: 60 };
    const created = await request(b, invoke); const jobId = created.job.jobId;
    assert.equal(created.executionStarted, false);
    const duplicate = await request(b, invoke); assert.equal(duplicate.job.jobId, jobId); assert.equal(duplicate.idempotent, true);
    await request(b, { ...invoke, input: { text: 'conflict' } }, true);
    await request(b, { ...invoke, idempotencyKey: randomUUID(), declaredPrice: { amount: 1, currency: 'USD' } }, true);
    await request(b, { ...invoke, idempotencyKey: randomUUID(), declaredPrice: null }, true);
    await request(b, { action: 'service.review', jobId, approve: true, text: 'too early' }, true);
    await request(b, { action: 'service.dispatch', jobId });
    await request(b, { action: 'service.update', jobId, providerStatus: 'completed', text: 'adapter-observed fixture result', artifacts: [] });
    const delivered = await request(b, { action: 'service.status', jobId });
    assert.equal(delivered.job.state, 'delivered'); assert.equal(delivered.job.localReviewStatus, 'awaiting_review');
    await request(b, { action: 'service.review', jobId, approve: true, text: 'checked locally' });
    await stop(b); await start(b);
    assert.equal((await request(b, { action: 'service.result', jobId })).job.state, 'completed');
    report.jobMappingOnly = 'This check simulates an authenticated local adapter observation; official SDK wire interoperability is a separate test, not proven here.';
  });
  await check('Execution budget persists across restart, imported policy requires adoption, audit remains queryable', async () => {
    const before = await governance(c, 'policy.inspect');
    const proposal = await governance(c, 'policy.import', { policy: { dailyExecutions: 1, maxConversationTurns: 2 } });
    assert.equal(proposal.adopted, false);
    assert.deepEqual((await governance(c, 'policy.inspect')).policy, before.policy);
    await governance(c, 'policy.adopt', { proposalId: proposal.proposalId });
    const reserved = await governance(c, 'budget.reserve', { resourceAction: 'execute', peer: b.id, requestId: randomUUID(), conversationId: 'fixture-loop' });
    await governance(c, 'budget.commit', { reservationId: reserved.reservationId });
    await stop(c); await start(c);
    await governance(c, 'budget.reserve', { resourceAction: 'execute', peer: b.id, requestId: randomUUID(), conversationId: 'fixture-loop' }, true);
    assert.equal((await governance(c, 'budget.inspect')).dailyExecutions, 1);
    assert((await governance(c, 'audit.export')).events.length > 0);
  });
  await check('Signed owner rotation is explicit; old grants stay unusable and new grants require a fresh import', async () => {
    const rotated = await request(a, { action: 'owner.rotate' });
    assert.notEqual(rotated.ownerId, owner.ownerId);
    await request(b, { action: 'owner.rotation-import', transition: rotated.transition });
    await request(b, { action: 'owner.verify', grantId: grant.id, audience: c.id, scope: 'service.invoke' }, true);
    const replacement = await request(a, { action: 'owner.grant', agentId: b.id, audience: c.id, scopes: ['service.invoke'], ttlSeconds: 300 });
    await request(b, { action: 'owner.import', grant: replacement.grant, expectedOwner: rotated.ownerId, trustOwner: true });
    assert.equal((await request(b, { action: 'owner.verify', grantId: replacement.id, audience: c.id, scope: 'service.invoke' })).valid, true);
  });
  report.passed = true;
} catch (error) {
  report.error = error.stack || error.message; process.exitCode = 1;
} finally {
  for (const device of [...active]) { try { await stop(device); } catch (error) { report.cleanupIncomplete = true; report.passed = false; process.exitCode = 1; report.cleanupError = error.message; } }
  report.finishedAt = new Date().toISOString();
  console.log(JSON.stringify({ ...report, reportPath: await saveReport(lab.runDirectory, report) }));
}
