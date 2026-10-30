import { copyFile, mkdir, readFile, writeFile, access } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve, join } from 'node:path';
import {createHash} from 'node:crypto';
import {zipSync} from 'fflate';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const dist = resolve(root, '../distribution');
const publicRoot = join(root, 'public');
const indexPath = join(publicRoot, '.well-known/skills/index.json');
const index = {skills:[{name:'aiconnect',description:'Install and operate AIConnect for local-first P2P agent messaging, temporary connections, contacts, groups and collaboration.'}]};
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
// One complete artifact, instead of silently accepting failed per-file fetches.
const entries={};
for(const file of files)entries[file]=[new Uint8Array(await readFile(join(publicRoot,'.well-known/skills/aiconnect',file))),{mtime:new Date('2026-01-01T00:00:00Z')}];
const repositorySkill=resolve(root,'../skills/aiconnect');
for(const file of files){await mkdir(dirname(join(repositorySkill,file)),{recursive:true});await copyFile(join(publicRoot,'.well-known/skills/aiconnect',file),join(repositorySkill,file));}
const archive=zipSync(entries,{level:6});
const digest=createHash('sha256').update(archive).digest('hex');
const archiveName=`aiconnect-${digest.slice(0,16)}.zip`;
await writeFile(join(publicRoot,archiveName),archive);
// Keep the inspectable resource list separate from the discovery protocol.
await writeFile(join(publicRoot,'.well-known/skills/files.json'),JSON.stringify(index,null,2)+'\n');
const discovery={$schema:'https://schemas.agentskills.io/discovery/0.2.0/schema.json',skills:[{name:'aiconnect',description:index.skills[0].description,type:'archive',url:`/${archiveName}`,digest:`sha256:${digest}`}]};
await writeFile(indexPath,JSON.stringify(discovery,null,2)+'\n');
const preferred=join(publicRoot,'.well-known/agent-skills');
await mkdir(preferred,{recursive:true});
await writeFile(join(preferred,'index.json'),JSON.stringify(discovery,null,2)+'\n');
console.log(`Staged distribution helpers; skill advertises ${files.length} existing files. Release manifests must be generated from actual artifacts separately.`);
