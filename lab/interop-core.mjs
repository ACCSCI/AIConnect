#!/usr/bin/env node
/** Actual Rust daemon + authenticated IPC + official MCP stdio client. No model/network calls. */
import assert from 'node:assert/strict';
import { resolve, join } from 'node:path';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import { Client } from '../adapters/interop/node_modules/@modelcontextprotocol/sdk/dist/esm/client/index.js';
import { StdioClientTransport } from '../adapters/interop/node_modules/@modelcontextprotocol/sdk/dist/esm/client/stdio.js';
import { runFile, decodeJson } from './lib.mjs';
import { coreClient } from '../adapters/interop/core-ipc.mjs';
import { processA2AJobs } from '../adapters/interop/a2a.mjs';

const binary = resolve(process.env.AICONNECT_TEST_BINARY || `target/debug/aiconnect${process.platform === 'win32' ? '.exe' : ''}`);
const root = resolve('.local/interop',randomUUID());
const data = join(root,'profile');
await mkdir(data,{recursive:true});
const report = { startedAt: new Date().toISOString(), passed:false, scope:'Single-host real native daemon, authenticated IPC and official MCP SDK stdio. A2A wire/reference tests are separate contract fixtures; no WAN, paid APIs or models.', checks:[], binarySha256:createHash('sha256').update(await readFile(binary)).digest('hex') };
let started = false, client, transport;
async function cli(args) {
  const r = await runFile(binary,['--data',data,'--json',...args],{timeoutMs:45000});
  assert.equal(r.timedOut,false);
  const result = decodeJson(r.stdout || r.stderr);
  assert.equal(r.code,0,JSON.stringify(result));
  return result;
}
try {
  await cli(['setup','--direct-only']); started = true;
  const core = coreClient(data);
  const status = await core({action:'status'});
  assert.ok(status.id); report.checks.push('Authenticated Node IPC reads real Rust identity without printing credentials');
  assert.deepEqual((await processA2AJobs(core)).results,[]);
  report.checks.push('Outbound official A2A worker queries real Core pending Job store without external calls');
  await assert.rejects(core({action:'provider.authorize',url:'https://unregistered.invalid'}));
  report.checks.push('Unregistered external provider is rejected by real Core free-only policy');
  client = new Client({name:'aiconnect-native-lab',version:'1'});
  transport = new StdioClientTransport({command:process.execPath,args:[resolve('adapters/interop/bridge.mjs'),'mcp','--data',data],stderr:'pipe',maxBufferSize:1024*1024});
  await client.connect(transport);
  const {tools} = await client.listTools();
  assert.equal(tools.length,4); assert.ok(tools.every(t=>t.annotations?.readOnlyHint===true));
  const result = await client.callTool({name:'aiconnect_status',arguments:{}});
  assert.equal(JSON.parse(result.content[0].text).id,status.id);
  const discovered = await client.callTool({name:'aiconnect_discover',arguments:{scope:'trusted'}});
  assert.notEqual(discovered.isError,true,JSON.stringify(discovered));
  report.checks.push('Official MCP client initializes real stdio bridge; read-only status and cached discovery reach Rust daemon');
  const refused = await client.callTool({name:'service_invoke',arguments:{}});
  assert.equal(refused.isError,true);
  report.checks.push('MCP has no execution, arbitrary Core proxy or approval tool');
  await client.close(); client = undefined;
  const localConfig=join(root,'mcp-opt-in.json');
  await writeFile(localConfig,JSON.stringify({enableInvoke:true}));
  client=new Client({name:'aiconnect-native-lab-opt-in',version:'1'});
  transport=new StdioClientTransport({command:process.execPath,args:[resolve('adapters/interop/bridge.mjs'),'mcp','--data',data,'--config',localConfig],stderr:'pipe',maxBufferSize:1024*1024});
  await client.connect(transport);
  const optInTools=await client.listTools();assert.equal(optInTools.tools.length,5);
  const invalidGrant=await client.callTool({name:'aiconnect_service_invoke',arguments:{transport:'a2a',provider:'https://unregistered.invalid',audience:'https://unregistered.invalid',capability:'test',grantId:'not-a-grant',input:{test:true},idempotencyKey:'must-deny'}});
  assert.equal(invalidGrant.isError,true);
  report.checks.push('Explicit opt-in MCP invoke still fails real Core provider/grant authorization; tool cannot create grants');
  await client.close();client=undefined;
  await cli(['daemon','stop']); started = false;
  await cli(['daemon','start','--direct-only']); started = true;
  assert.equal((await core({action:'status'})).id,status.id);
  report.checks.push('Restart retains native profile identity and IPC reconnect reloads new daemon credentials');
  report.passed=true;
} catch(error) { report.error=error.message;process.exitCode=1; }
finally {
  await client?.close().catch(()=>{});
  await transport?.close().catch(()=>{});
  if (started) try { await cli(['daemon','stop']); } catch {report.cleanupIncomplete=true;report.passed=false;process.exitCode=1;}
  report.finishedAt=new Date().toISOString();
  const reportPath=join(root,'report.json');
  await writeFile(reportPath,JSON.stringify(report,null,2));
  process.stdout.write(`${JSON.stringify({...report,reportPath})}\n`);
}
