import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, access } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import '../scripts/stage.mjs';

const publicRoot = resolve(import.meta.dirname, '../public');
test('discovery index lists real downloadable skill resources', async () => {
  const index = JSON.parse(await readFile(join(publicRoot, '.well-known/skills/index.json'), 'utf8'));
  assert.equal(index.skills.length, 1);
  assert(index.skills[0].files.includes('scripts/codex-app-server.mjs'), 'Agent bridge must ship with the Skill');
  for (const skill of index.skills) {
    assert.match(skill.name, /^[a-z0-9]+(?:-[a-z0-9]+)*$/);
    assert(skill.description.length > 0 && skill.description.length <= 1024);
    assert(skill.files.includes('SKILL.md'));
    for (const path of skill.files) {
      assert(!path.includes('..') && !path.startsWith('/') && !path.includes('\\'));
      await access(join(publicRoot, '.well-known/skills', skill.name, path));
    }
    const md = await readFile(join(publicRoot, '.well-known/skills', skill.name, 'SKILL.md'), 'utf8');
    assert(md.startsWith('---\nname: aiconnect\n'));
  }
  assert.deepEqual(
    await readFile(join(publicRoot, '.well-known/skills/aiconnect/scripts/codex-app-server.mjs')),
    await readFile(resolve(import.meta.dirname, '../../adapters/codex-app-server.mjs')),
    'The distributed Agent bridge must match the maintained adapter source',
  );
});

test('production configuration serves only static assets without a Worker entry point', async () => {
  const config = JSON.parse(await readFile(resolve(import.meta.dirname, '../wrangler.jsonc'), 'utf8'));
  assert.equal(config.main, undefined);
  assert.equal(config.assets.binding, undefined);
  assert.equal(config.assets.run_worker_first, undefined);
  assert.equal(config.assets.directory, 'public');
  assert.equal(config.assets.not_found_handling, 'none');
  const health = JSON.parse(await readFile(join(publicRoot, 'healthz'), 'utf8'));
  assert.equal(health.staticAsset, true);
  assert.equal(health.dynamicWorker, false);
  assert.equal(health.chatRelay, false);
  assert.equal(health.accountDatabase, false);
  await assert.rejects(access(join(publicRoot, 'api/messages')));
});

test('static document navigation and accessibility targets exist', async () => {
  const html = await readFile(join(publicRoot, 'index.html'), 'utf8');
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map(x => x[1]);
  assert.equal(ids.length, new Set(ids).size);
  for (const match of html.matchAll(/href="#([^"]+)"/g)) assert(ids.includes(match[1]), match[1]);
  assert.equal([...html.matchAll(/<h1\b/g)].length, 1);
  assert.match(html, /lang="zh-CN"/);
  assert.match(html, /Agent 的即时通讯网络/);
  assert.match(html, /assets\/aiconnect\.js/);
  await access(join(publicRoot, 'assets/aiconnect.js'));
  await access(join(publicRoot, 'assets/aiconnect.css'));
  assert(!html.includes('ops.accsci.top'));
});
