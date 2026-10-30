import { mkdir, writeFile, readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { runTask } from './codex-app-server.mjs';
const workspace = resolve('.local/adapter-selftest', randomUUID());
await mkdir(workspace, { recursive: true });
const marker = `fixture-${randomUUID()}`;
await writeFile(join(workspace, 'inventory.txt'), `Synthetic fixture only.\napplication=ExampleEditor\nversion=1.2.3\nmarker=${marker}\n`);
const result = await runTask({ v: 1, taskId: randomUUID(), messageId: randomUUID(), sessionId: randomUUID(), sender: 'local-selftest', workspace, permissions: { wake: true, task: true }, expiresAt: Date.now() + 90000, budget: { timeoutMs: 90000, maxTurns: 1 }, sandbox: 'workspace-write', text: 'Read inventory.txt in the current workspace using a local file-reading tool. Create verification.txt in this workspace containing the application and exact marker found in inventory.txt. Do not read any other files, use the network, or delegate. Reply with the exact marker and application name.' }, { cwd: workspace });
let verifiedFile = false;
try { const text = await readFile(join(workspace, 'verification.txt'), 'utf8'); verifiedFile = text.includes(marker) && text.includes('ExampleEditor'); } catch {}
const evidence = { at: new Date().toISOString(), scope: 'Real Codex app-server local workspace fixture; no P2P involved', passed: result.status === 'completed' && result.text.includes(marker) && verifiedFile, status: result.status, error: result.error, toolCount: result.runtime?.toolCount, replyVerified: result.text.includes(marker), fileVerified: verifiedFile, credentialHandling: 'Existing CLI login only; no credentials read or printed', isolation: 'Codex workspace-write policy and synthetic workspace; not OS user isolation' };
await writeFile(resolve('.local/adapter-selftest-result.json'), JSON.stringify(evidence, null, 2));
console.log(JSON.stringify(evidence, null, 2));
process.exitCode = evidence.passed ? 0 : 1;
