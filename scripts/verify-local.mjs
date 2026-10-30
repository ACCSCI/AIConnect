#!/usr/bin/env node
/** Local verification only: this script never deploys or publishes. */
import {spawn} from 'node:child_process';
import {access,writeFile,mkdir} from 'node:fs/promises';
import {resolve,dirname} from 'node:path';
import {fileURLToPath} from 'node:url';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const full=process.argv.includes('--agents');
const network=process.argv.includes('--network');
const checks=[
  ['Rust workspace tests','cargo',['test','--workspace','-j','2'],'Cargo.toml'],
  ['Native CLI build','cargo',['build','-p','aiconnect-cli','-j','2'],'Cargo.toml'],
  ['Agent adapter tests',process.execPath,['--test','adapters/test/adapter.test.mjs','adapters/test/bridge.test.mjs','adapters/test/notification.test.mjs'],'adapters/test/adapter.test.mjs'],
  ['Mini-environment harness tests',process.execPath,['--test','lab/lib.test.mjs','lab/six-layer-pressure-metrics.test.mjs'],'lab/lib.test.mjs'],
  ['Official A2A and MCP SDK tests',process.execPath,['--test','adapters/interop/test/interop.test.mjs'],'adapters/interop/package-lock.json'],
  ['Six-layer native blackbox acceptance',process.execPath,['lab/six-layer-acceptance.mjs'],'lab/six-layer-acceptance.mjs'],
  ['Real Core and MCP SDK integration',process.execPath,['lab/interop-core.mjs'],'lab/interop-core.mjs'],
  ['Real Core and A2A task lifecycle',process.execPath,['lab/interop-a2a-core.mjs'],'lab/interop-a2a-core.mjs'],
  ['Signed installer tests',process.execPath,['distribution/test-install.mjs'],'distribution/test-install.mjs'],
  ['Native blackbox acceptance',process.execPath,['test/acceptance/run.mjs'],'test/acceptance/run.mjs'],
  ['Native IPC adversarial tests',process.execPath,['--test','audit/ipc-negative.test.mjs'],'audit/ipc-negative.test.mjs'],
  ['Native runtime policy tests',process.execPath,['--test','audit/runtime-policy.test.mjs'],'audit/runtime-policy.test.mjs'],
  ['Task confirmation native CLI',process.execPath,['lab/task-flow.mjs'],'lab/task-flow.mjs'],
  ['Group chat native nodes',process.execPath,['lab/group-chat.mjs'],'lab/group-chat.mjs'],
  ['Group chat relay transport',process.execPath,['lab/group-chat.mjs','--relay-only'],'lab/group-chat.mjs'],
  ['Replicated group owner and sender offline',process.execPath,['lab/group-replication.mjs'],'lab/group-replication.mjs'],
  ['Replicated group forced relay',process.execPath,['lab/group-replication.mjs','--relay-only'],'lab/group-replication.mjs'],
  ['Native forced relay transport',process.execPath,['lab/run.mjs','--relay-only'],'lab/run.mjs'],
  ['Workers local route tests',process.execPath,['--run','test','--prefix','site'],'site/package.json'],
  ['Workers real local HTTP',process.execPath,['--run','test:local'],'site/package.json'],
];
// Run model-backed collaboration last, after deterministic and network gates pass.
if(full)checks.push(['Real dual-Agent collaboration',process.execPath,['lab/run.mjs','--agents'],'lab/run.mjs']);
if(full)checks.push(['Real task confirmation collaboration',process.execPath,['lab/task-flow.mjs','--agents'],'lab/task-flow.mjs']);
if(full)checks.push(['Real three-Agent group chat',process.execPath,['lab/group-chat.mjs','--agents'],'lab/group-chat.mjs']);
const result={startedAt:new Date().toISOString(),scope:'Local verification only; no deployment or publication',realAgents:full,checks:[]};
const selectedChecks=checks.filter(([name])=>network||!(/relay transport|forced relay|forced relay transport/i.test(name)));
for(const [name,command,args,required]of selectedChecks){
  console.log(`\n${name}`);const started=Date.now();
  try{await access(resolve(root,required));}catch{result.checks.push({name,status:'missing',required});process.exitCode=1;break;}
  // --run executes a package script in the selected directory, without npm publication.
  const cwd=name.startsWith('Workers ')?resolve(root,'site'):root;
  const actualArgs=name==='Workers local route tests'?['--run','test']:args;
  const env={...process.env};
  if(!network)env.AICONNECT_TEST_LOOPBACK='1';
  if(name==='Signed installer tests')env.AICONNECT_NATIVE_BINARY=resolve(root,'target/debug',process.platform==='win32'?'aiconnect.exe':'aiconnect');
  const code=await new Promise((done,reject)=>{const child=spawn(command,actualArgs,{cwd,env,stdio:'inherit',windowsHide:true});child.once('error',reject);child.once('exit',c=>done(c??1));}).catch(error=>{console.error(error.message);return 1;});
  result.checks.push({name,status:code===0?'passed':'failed',elapsedMs:Date.now()-started});
  if(code!==0){process.exitCode=1;break;}
}
result.finishedAt=new Date().toISOString();result.passed=result.checks.length===selectedChecks.length&&result.checks.every(c=>c.status==='passed');
result.networkTests=network;result.localLoopbackOnly=!network;
await mkdir(resolve(root,'.local'),{recursive:true});await writeFile(resolve(root,'.local/verification.json'),JSON.stringify(result,null,2)+'\n');
console.log(JSON.stringify(result,null,2));
