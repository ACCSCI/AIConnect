#!/usr/bin/env node
/** Read-only public relay metadata and subscription probe. Never publishes EVENTs. */
import { writeFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';

const targets = ['https://nos.lol/', 'https://relay.damus.io/'];
const report = {
  checkedAt: new Date().toISOString(),
  scope: 'Read-only NIP-11 metadata plus bounded NIP-01 REQ/EOSE on public Nostr relays. No EVENT publication, identity, contact, private message, model or billing credentials transmitted.',
  interpretation: 'Connectivity/read support is not publication permission, free production certification, persistence guarantee, or iroh relay compatibility.',
  results: [],
};
async function websocketProbe(httpsUrl) {
  const url = new URL(httpsUrl); url.protocol = 'wss:';
  const subscription = `aic-readonly-${randomUUID()}`;
  return new Promise(resolveProbe => {
    const ws = new WebSocket(url);
    let finished = false, messages = 0, events = 0;
    const finish = result => {
      if (finished) return; finished = true; clearTimeout(timer);
      if (ws.readyState === WebSocket.OPEN) { ws.send(JSON.stringify(['CLOSE', subscription])); ws.close(); }
      else { try { ws.close(); } catch {} }
      resolveProbe({ ...result, messages, observedEvents: events, sentEvent: false });
    };
    const timer = setTimeout(() => finish({ connected: ws.readyState === WebSocket.OPEN, eose: false, error: 'bounded_read_timeout' }), 10000);
    ws.addEventListener('open', () => ws.send(JSON.stringify(['REQ', subscription, { kinds: [30078], '#d': ['aiconnect.readonly-probe.no-publication'], limit: 1 }])));
    ws.addEventListener('message', event => {
      messages++;
      if (String(event.data).length > 131072 || messages > 20) return finish({ connected: true, eose: false, error: 'response_resource_limit' });
      let value; try { value = JSON.parse(String(event.data)); } catch { return; }
      if (value[0] === 'EVENT') events++;
      if (value[0] === 'EOSE' && value[1] === subscription) finish({ connected: true, eose: true });
      if (value[0] === 'AUTH') finish({ connected: true, eose: false, authRequired: true });
      if (value[0] === 'CLOSED' && value[1] === subscription) finish({ connected: true, eose: false, closedReason: String(value[2] || '').slice(0, 300) });
    });
    ws.addEventListener('error', () => finish({ connected: false, eose: false, error: 'websocket_connection_error' }));
    ws.addEventListener('close', event => finish({ connected: false, eose: false, closeCode: event.code }));
  });
}
await Promise.all(targets.map(async url => {
  const entry = { url, checkedAt: new Date().toISOString(), productionDefaultApproved: false };
  try {
    const response = await fetch(url, { headers: { Accept: 'application/nostr+json' }, redirect: 'error', signal: AbortSignal.timeout(10000) });
    entry.httpStatus = response.status;
    const body = await response.text();
    if (body.length > 131072) throw Error('metadata_resource_limit');
    const metadata = JSON.parse(body);
    entry.nip11 = {
      name: metadata.name, description: metadata.description, software: metadata.software,
      version: metadata.version, supportedNips: metadata.supported_nips,
      limitation: metadata.limitation, fees: metadata.fees,
      privacyPolicy: metadata.privacy_policy, termsOfService: metadata.terms_of_service,
    };
    entry.freeReadDeclared = typeof metadata.limitation?.payment_required === 'boolean' ? !metadata.limitation.payment_required : null;
  } catch (error) { entry.metadataError = error.message; }
  entry.subscription = await websocketProbe(url);
  report.results.push(entry);
}));
report.results.sort((a, b) => a.url.localeCompare(b.url));
report.completedAt = new Date().toISOString();
const directory = resolve('.local'); await mkdir(directory, { recursive: true });
const reportPath = resolve(directory, `six-layer-relay-probe-${Date.now()}.json`);
await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ ...report, reportPath }));
