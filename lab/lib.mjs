import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';

export function option(args, flag, fallback) {
  const at = args.indexOf(flag);
  if (at < 0) return fallback;
  if (!args[at + 1] || args[at + 1].startsWith('--')) throw new Error(`missing ${flag}`);
  return args[at + 1];
}
/** Public relays are development/test infrastructure, never implicit production fallback. */
export function testTransportArgs(relayOnly = false) {
  return relayOnly ? ['--relay-only', '--dev-public-relay'] : ['--direct-only'];
}
export function runFile(file, args, { cwd, timeoutMs = 30000, stdin, env } = {}) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(file, args, { cwd, env: env || process.env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    let stdout = '', stderr = '', timedOut = false;
    const timeout = setTimeout(() => { timedOut = true; child.kill(); }, timeoutMs);
    child.stdout.on('data', data => { stdout += data; if (stdout.length > 2_000_000) child.kill(); });
    child.stderr.on('data', data => { stderr += data; if (stderr.length > 2_000_000) child.kill(); });
    child.on('error', error => { clearTimeout(timeout); reject(error); });
    // A detached Windows descendant may retain an inherited pipe handle. The
    // command process's exit is authoritative; do not wait forever for all
    // descendants to close inherited pipes before observing that completion.
    child.on('exit', code => {
      clearTimeout(timeout);
      setTimeout(() => {
        child.stdout.destroy(); child.stderr.destroy(); child.stdin.destroy();
        resolveRun({ code, stdout, stderr, timedOut });
      }, 25);
    });
    child.stdin.on('error', () => {});
    child.stdin.end(stdin);
  });
}
export function decodeJson(output) {
  const text = output.trim();
  try { return JSON.parse(text); } catch {}
  const rows = text.split(/\r?\n/).filter(Boolean);
  for (let i = rows.length - 1; i >= 0; i--) {
    try { return JSON.parse(rows[i]); } catch {}
  }
  throw new Error('CLI did not return JSON');
}
export async function createLab(root) {
  const marker = randomBytes(12).toString('hex');
  const runDirectory = resolve(root, `${new Date().toISOString().replace(/[:.]/g, '-')}-${marker.slice(0, 6)}`);
  const devices = {};
  for (const name of ['a', 'b', 'c']) {
    devices[name] = { name, data: join(runDirectory, name, 'data'), workspace: join(runDirectory, name, 'workspace') };
    await mkdir(devices[name].data, { recursive: true });
    await mkdir(devices[name].workspace, { recursive: true });
  }
  const source = { device: 'old-computer-b', fixtureMarker: marker, editor: { theme: 'sage', fontSize: 15 }, tools: ['git', 'node', 'rust'] };
  await writeFile(join(devices.b.workspace, 'settings.json'), JSON.stringify(source, null, 2));
  await writeFile(join(devices.a.workspace, 'target.json'), JSON.stringify({ device: 'new-computer-a', existingFiles: [] }, null, 2));
  return { runDirectory, devices, source };
}
export async function saveReport(runDirectory, report) {
  const reportPath = join(runDirectory, 'report.json');
  await writeFile(reportPath, JSON.stringify(report, null, 2));
  return reportPath;
}
export async function loadPlan(device) {
  return JSON.parse(await readFile(join(device.workspace, 'migration-plan.json'), 'utf8'));
}
export function shellQuote(value, platform = process.platform) {
  if (platform === 'win32') return `'${String(value).replaceAll("'", "''")}'`;
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}
