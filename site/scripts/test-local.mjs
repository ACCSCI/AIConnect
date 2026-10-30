import assert from 'node:assert/strict';
import { unstable_dev } from 'wrangler';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdir, mkdtemp, readFile, access } from 'node:fs/promises';
import './stage.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
process.chdir(root);
// Omit the script argument: exercise the exact asset-only production configuration.
const server = await unstable_dev(undefined, {
  config: 'wrangler.jsonc', local: true, ip: '127.0.0.1', port: 0,
  inspectorPort: 0, persist: false, logLevel: 'error',
  experimental: { disableExperimentalWarning: true, disableDevRegistry: true, forceLocal: true, liveReload: false, watch: false },
});
try {
  const base = `http://${server.address}:${server.port}`;
  const request = (path, init) => fetch(new URL(path, base), init);
  const home = await request('/');
  assert.equal(home.status, 200);
  assert.match(home.headers.get('content-type'), /text\/html/);
  assert.match(await home.text(), /Agent 的即时通讯网络/);
  assert.equal(home.headers.get('x-content-type-options'), 'nosniff');
  assert.match(home.headers.get('content-security-policy'), /connect-src 'none'/);
  const indexResponse = await request('/.well-known/skills/index.json');
  assert.equal(indexResponse.status, 200);
  assert.match(indexResponse.headers.get('content-type'), /application\/json/);
  assert.equal(indexResponse.headers.get('access-control-allow-origin'), '*');
  const index = await indexResponse.json();
  for (const skill of index.skills) for (const file of skill.files) {
    const res = await request(`/.well-known/skills/${skill.name}/${file}`);
    assert.equal(res.status, 200, file);
    assert((await res.text()).length > 0);
    if (file.endsWith('.md')) assert.match(res.headers.get('content-type'), /text\/markdown/);
  }
  assert.equal((await request('/api/messages', { method: 'POST' })).status, 404);
  assert.equal((await request('/api/messages')).status, 404);
  assert.equal((await request('/not-a-real-route')).status, 404);
  const release = await request('/releases/stable.json');
  if (release.status === 200) {
    const manifest = await release.json();
    assert.equal(manifest.signature.algorithm, 'ed25519');
    assert(Object.keys(manifest.payload.artifacts).length > 0);
  } else assert.equal(release.status, 404, 'A release must be real or absent');
  const english = await request('/en/');
  assert.equal(english.status, 200);
  assert.match(await english.text(), /lang="en"/);
  const health = await (await request('/healthz')).json();
  assert.equal(health.staticAsset, true);
  assert.equal(health.dynamicWorker, false);
  assert.equal(health.chatRelay, false);
  assert.equal(health.accountDatabase, false);
  const healthHead = await request('/healthz', { method: 'HEAD' });
  assert.equal(healthHead.status, 200);
  assert.match(healthHead.headers.get('content-type'), /application\/json/);
  assert.equal(healthHead.headers.get('cache-control'), 'no-store');
  assert.equal(await healthHead.text(), '');
  const { stdout, stderr } = await promisify(execFile)(process.execPath, [resolve(root, 'node_modules/skills/bin/cli.mjs'), 'add', base, '--list'], { cwd: root, env: { ...process.env, DISABLE_TELEMETRY: '1', DO_NOT_TRACK: '1', NO_COLOR: '1' }, timeout: 30000, windowsHide: true });
  const discovery = stdout + stderr;
  assert.match(discovery, /Found 1 skill/);
  assert.match(discovery, /aiconnect/);
  assert.match(discovery, /install\.mjs/);
  const localRoot = resolve(root, '.local');
  await mkdir(localRoot, { recursive: true });
  const installRoot = await mkdtemp(resolve(localRoot, 'skill-install-'));
  await promisify(execFile)(process.execPath, [resolve(root, 'node_modules/skills/bin/cli.mjs'), 'add', base, '--skill', 'aiconnect', '--agent', 'codex', '--yes', '--copy'], { cwd: installRoot, env: { ...process.env, DISABLE_TELEMETRY: '1', DO_NOT_TRACK: '1', NO_COLOR: '1' }, timeout: 30000, windowsHide: true });
  // Project scope only: no -g, no mutation of the user's global skill directories.
  const installed = resolve(installRoot, '.agents/skills/aiconnect');
  for (const file of index.skills[0].files) {
    await access(resolve(installed, file));
    assert.deepEqual(await readFile(resolve(installed, file)), await readFile(resolve(root, 'public/.well-known/skills/aiconnect', file)), `Installed file matches served bytes: ${file}`);
  }
  console.log(JSON.stringify({ ok: true, engine: 'Wrangler local workerd', deploymentMode: 'assets-only-no-user-worker', skillsCliVersion: '1.7.4', checks: ['home', 'security headers', 'skill index', 'all skill resources', 'MIME', 'CORS', 'missing release 404', 'natural static GET/POST API 404', 'static health GET/HEAD and cache headers', 'official skills CLI --list discovery', 'isolated project Skill installation and exact file bytes'], base, skillInstallRoot: installRoot }));
} finally { await server.stop(); }
