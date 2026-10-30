import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { option, decodeJson, createLab, runFile, shellQuote } from './lib.mjs';
test('separate fixture workspaces contain a B-only synthetic marker', async () => {
  const root = await mkdtemp(join(tmpdir(), 'aiconnect-fixture-'));
  const lab = await createLab(root);
  assert.equal(new Set(Object.values(lab.devices).map(d => d.data)).size, 3);
  const a = await readFile(join(lab.devices.a.workspace, 'target.json'), 'utf8');
  const b = await readFile(join(lab.devices.b.workspace, 'settings.json'), 'utf8');
  assert(!a.includes(lab.source.fixtureMarker));
  assert(b.includes(lab.source.fixtureMarker));
});
test('runner sends exact JSON input without shell interpretation', async () => {
  const payload = { text: '`$() & dangerous-looking literal' };
  const result = await runFile(process.execPath, ['-e', 'process.stdin.pipe(process.stdout)'], { stdin: JSON.stringify(payload) });
  assert.equal(result.code, 0); assert.deepEqual(decodeJson(result.stdout), payload);
  assert.equal(shellQuote("a'b", 'win32'), "'a''b'");
});
test('missing option values and missing JSON are rejected', () => {
  assert.throws(() => option(['--binary'], '--binary'), /missing/);
  assert.throws(() => decodeJson('diagnostic only'), /JSON/);
});
test('completed command is observed even if a descendant retains output pipes', async () => {
  const code = `const {spawn}=require('node:child_process'); const p=spawn(process.execPath,['-e','setTimeout(()=>{},1800)'],{stdio:['ignore',process.stdout,process.stderr],windowsHide:true}); p.unref(); console.log('{"completed":true}');`;
  const result = await runFile(process.execPath, ['-e', code], { timeoutMs: 700 });
  assert.equal(result.code, 0);
  assert.equal(result.timedOut, false);
  assert.equal(decodeJson(result.stdout).completed, true);
});
