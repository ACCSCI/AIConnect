import { copyFile, mkdir, readFile, writeFile, access } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dist = resolve(root, '../distribution');
const publicRoot = join(root, 'public');
const indexPath = join(publicRoot, '.well-known/skills/index.json');
const index = JSON.parse(await readFile(indexPath, 'utf8'));
const files = ['SKILL.md', 'references/operations.md', 'references/install.md'];
for (const source of ['install.ps1', 'install.sh', 'install.mjs']) {
  try { await access(join(dist, source)); } catch { continue; }
  await copyFile(join(dist, source), join(publicRoot, source));
  const scripts = join(publicRoot, '.well-known/skills/aiconnect/scripts');
  await mkdir(scripts, { recursive: true });
  await copyFile(join(dist, source), join(scripts, source));
  files.push(`scripts/${source}`);
}
// The optional agent runtime lives with the installed Skill, not in the native binary.
const skillScripts = join(publicRoot, '.well-known/skills/aiconnect/scripts');
await mkdir(skillScripts, { recursive: true });
await copyFile(resolve(root, '../adapters/codex-app-server.mjs'), join(skillScripts, 'codex-app-server.mjs'));
files.push('scripts/codex-app-server.mjs');
// Ship the optional local adapters as source plus an exact npm lockfile. Do not
// copy tests/node_modules or install anything while staging a static website.
const interopSources = ['bridge.mjs', 'a2a.mjs', 'mcp.mjs', 'core-ipc.mjs', 'safe-http.mjs', 'normalize.mjs', 'package.json', 'package-lock.json'];
const skillInterop = join(skillScripts, 'interop');
await mkdir(skillInterop, { recursive: true });
for (const source of interopSources) {
  await copyFile(resolve(root, '../adapters/interop', source), join(skillInterop, source));
  files.push(`scripts/interop/${source}`);
}
index.skills[0].files = files;
await writeFile(indexPath, JSON.stringify(index, null, 2) + '\n');
console.log(`Staged distribution helpers; skill advertises ${files.length} existing files. Release manifests must be generated from actual artifacts separately.`);
