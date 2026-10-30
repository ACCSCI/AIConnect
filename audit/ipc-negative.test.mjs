import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { spawn } from 'node:child_process';
import { createConnection } from 'node:net';

const executable = resolve(process.env.AICONNECT_NATIVE_BINARY || (process.platform === 'win32' ? 'target/debug/aiconnect.exe' : 'target/debug/aiconnect'));
const base = resolve('.local/audit-ipc');
await mkdir(base, { recursive: true });
// Observe process exit rather than Windows inherited-pipe close events.
function native(args) {
  return new Promise((res, rej) => {
    const child = spawn(executable, args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = '', settled = false;
    child.stdout.on('data', data => stdout += data);
    child.stderr.on('data', data => stderr += data);
    const timer = setTimeout(() => { child.kill(); finish(Object.assign(new Error('native_command_timeout'), { killed: true })); }, 45000);
    function finish(error) {
      if (settled) return;
      settled = true; clearTimeout(timer); child.stdout.destroy(); child.stderr.destroy();
      if (error) { error.stderr = stderr; rej(error); } else res({ stdout, stderr });
    }
    child.once('error', finish);
    child.once('exit', code => setTimeout(() => finish(code === 0 ? null : Object.assign(new Error('native_command_rejected'), { code, killed: false })), 50));
  });
}

test('native daemon singleton, oversized frames and incomplete clients preserve service availability', async t => {
  const directory = await mkdtemp(join(base, 'profile-'));
  const cli = (...args) => native(['--data', directory, '--json', ...args]);
  const sockets = [];
  let running = false;
  let phaseStarted = Date.now();
  const phase = name => { t.diagnostic(`${name}: ${Date.now() - phaseStarted}ms`); phaseStarted = Date.now(); };
  try {
    await cli('daemon', 'start', '--direct-only');
    phase('startup');
    running = true;
    const before = JSON.parse((await cli('status')).stdout);
    assert.equal(typeof before.id, 'string');
    phase('initial status');

    await assert.rejects(cli('daemon', 'run', '--direct-only'), error => {
      assert.equal(error.code, 1, 'second daemon must exit itself, not be killed by the harness');
      assert.equal(error.killed, false);
      assert.match(error.stderr, /daemon_already_running_for_data_directory/);
      return true;
    });
    phase('singleton rejection');

    const info = JSON.parse(await readFile(join(directory, 'daemon.json'), 'utf8'));
    const oversized = await new Promise((resolveResponse, reject) => {
      const socket = createConnection(Number(info.address.split(':').at(-1)), '127.0.0.1');
      sockets.push(socket);
      let response = '';
      socket.setTimeout(5000, () => reject(new Error('oversized_frame_response_timeout')));
      socket.on('connect', () => socket.write('x'.repeat(1024 * 1024 + 1) + '\n'));
      socket.on('data', data => { response += data.toString(); if (response.includes('\n')) { socket.destroy(); resolveResponse(JSON.parse(response.trim())); } });
      socket.on('error', error => { if (!response) reject(error); });
      socket.on('end', () => { if (!response) reject(new Error('oversized_frame_no_response')); });
    });
    assert.equal(oversized.ok, false);
    assert.equal(oversized.error, 'request_too_large');
    phase('oversized frame');

    // Partial clients occupy bounded handlers, but should not block healthy requests.
    for (let i = 0; i < 4; i++) {
      const socket = createConnection(Number(info.address.split(':').at(-1)), '127.0.0.1');
      sockets.push(socket);
      socket.on('error', () => {});
      await new Promise((res, rej) => { socket.once('connect', res); socket.once('error', rej); });
      socket.write('{');
    }
    const after = JSON.parse((await cli('status')).stdout);
    assert.equal(after.id, before.id);
    phase('partial clients with healthy status');
  } finally {
    for (const socket of sockets) socket.destroy();
    if (running) await cli('daemon', 'stop');
    phase('shutdown');
  }
});
