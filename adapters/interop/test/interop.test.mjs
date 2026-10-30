import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import express from 'express';
import { AgentEvent, DefaultRequestHandler, InMemoryTaskStore } from '@a2a-js/sdk/server';
import { agentCardHandler, jsonRpcHandler, UserBuilder } from '@a2a-js/sdk/server/express';
import { TaskState, Role } from '@a2a-js/sdk';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { safeFetchFor, publicAddress } from '../safe-http.mjs';
import { CoreTaskStore, makeA2AClient, processA2AJobs, startA2AServer } from '../a2a.mjs';
import { makeMcpServer } from '../mcp.mjs';
import { normalizeAgentCard,normalizeRegistryPage } from '../normalize.mjs';

const close = server => new Promise(resolve => { server.closeAllConnections?.(); server.close(resolve); });
const listen = server => new Promise(resolve => server.listen(0,'127.0.0.1',() => resolve(`http://127.0.0.1:${server.address().port}`)));
const token = 'test-only-not-a-real-credential-1234567890';
const text = value => ({ content: { $case: 'text', value }, mediaType: 'text/plain', filename: '' });

// This fixture is deliberately separate from the Rust daemon: it tests official SDK
// wire behavior + the adapter/Core contract, not production grant or SQLite policy.
function fixtureCore(initial = []) {
  const jobs = new Map(initial.map(j => [j.jobId,structuredClone(j)]));
  const calls = [];
  const core = async q => {
    calls.push(structuredClone(q));
    if (q.action === 'provider.authorize') return { free: true, status: 'active' };
    if (q.action === 'provider.check') return { status: 'disabled' };
    if (q.action === 'governance.download.charge') return {chargedBytes:q.bytes};
    if (q.action === 'status') return { ready: true };
    if (q.action === 'discover') return { agents: [] };
    if (q.action === 'service.pending' || q.action === 'service.list') {
      const all = [...jobs.values()].filter(j => j.transport === q.transport && (q.action === 'service.list' || !['delivered','failed','canceled'].includes(j.state)));
      return { jobs: structuredClone(all), hasMore: false };
    }
    if (q.action === 'service.invoke') {
      const job = { ...q, action: undefined, jobId: randomUUID(), state: 'queued' }; jobs.set(job.jobId,job); return { job: structuredClone(job) };
    }
    let job = q.jobId ? jobs.get(q.jobId) : [...jobs.values()].find(j => j.externalTaskId === q.externalTaskId);
    if (!job) throw new Error('service_job_not_found');
    if (q.action === 'service.dispatch') {
      if (job.state !== 'queued') throw new Error('job_not_dispatchable');
      job.state = 'dispatching';
    } else if (q.action === 'service.update') {
      Object.assign(job,Object.fromEntries(Object.entries(q).filter(([k]) => !['action','jobId'].includes(k))));
      job.state = q.providerStatus === 'completed' ? 'delivered' : q.providerStatus;
      if (q.providerStatus === 'completed') job.localReviewStatus = 'awaiting_review';
    } else if (q.action === 'service.snapshot') job.externalTask = structuredClone(q.externalTask);
    else if (q.action === 'service.cancel') job.cancelRequested = true;
    return { job: structuredClone(job) };
  };
  return { core, jobs, calls };
}

async function referenceAgent() {
  const app = express(); app.use(express.json({ limit: '32kb' }));
  app.use((req,res,next) => req.headers.authorization === `Bearer ${token}` ? next() : res.status(401).end());
  const server = createServer(app); const base = await listen(server);
  const running = new Map();
  const card = { name: 'deterministic reference', description: 'No model, SDK reference fixture', version: '1',
    supportedInterfaces: [{ url: `${base}/rpc`, protocolBinding: 'JSONRPC', protocolVersion: '1.0', tenant: '' }],
    capabilities: { streaming: true, pushNotifications: false, extensions: [] }, securitySchemes: {}, securityRequirements: [],
    defaultInputModes: ['application/json','text/plain'], defaultOutputModes: ['text/plain'], skills: [], signatures: [] };
  const executor = {
    async execute(ctx,bus) {
      const task = { id: ctx.taskId, contextId: ctx.contextId, status: { state: TaskState.TASK_STATE_SUBMITTED }, artifacts: [], history: [] };
      running.set(task.id,{ bus, task }); bus.publish(AgentEvent.task(task));
      bus.publish(AgentEvent.statusUpdate({ taskId:task.id,contextId:task.contextId,status:{state:TaskState.TASK_STATE_WORKING},final:false }));
      const input = ctx.userMessage.parts.find(p => p.content?.$case === 'data')?.content.value;
      if (input?.input?.hold) return;
      await new Promise(resolve => setTimeout(resolve,30));
      bus.publish(AgentEvent.artifactUpdate({ taskId:task.id,contextId:task.contextId,artifact:{artifactId:'result',name:'result',parts:[text('deterministic result')]},append:false,lastChunk:true }));
      bus.publish(AgentEvent.statusUpdate({ taskId:task.id,contextId:task.contextId,status:{state:TaskState.TASK_STATE_COMPLETED,message:{messageId:randomUUID(),role:Role.ROLE_AGENT,parts:[text('delivered')]}},final:true }));
      bus.finished();
    },
    async cancelTask(id,bus) {
      bus.publish(AgentEvent.statusUpdate({ taskId:id,contextId:running.get(id).task.contextId,status:{state:TaskState.TASK_STATE_CANCELED},final:true })); bus.finished();
    },
  };
  const handler = new DefaultRequestHandler(card,new InMemoryTaskStore(),executor);
  app.use('/.well-known/agent-card.json',agentCardHandler({agentCardProvider:handler}));
  app.use('/rpc',jsonRpcHandler({ requestHandler:handler,userBuilder:UserBuilder.noAuthentication }));
  return { server,base };
}

test('HTTP boundary rejects private, redirect, oversized body and changed origin', async t => {
  for (const ip of ['127.0.0.1','10.0.0.1','169.254.169.254','100.64.0.1','::1','::ffff:127.0.0.1','2001:db8::1']) assert.equal(publicAddress(ip),false,ip);
  assert.equal(publicAddress('8.8.8.8'),true);
  assert.equal(publicAddress('2606:4700:4700::1111'),true);
  await assert.rejects(safeFetchFor('https://example.com',{resolve:async()=>[{address:'127.0.0.1',family:4}]})('https://example.com'),/private_address/);
  await assert.rejects(safeFetchFor('https://example.com')('https://different.example'),/origin_mismatch/);
  const server = createServer((req,res) => {
    if (req.url === '/redirect') { res.writeHead(302,{location:'http://169.254.169.254'}); res.end(); }
    else res.end('x'.repeat(1025));
  });
  const base = await listen(server); t.after(()=>close(server));
  const fetch = safeFetchFor(base,{allowLoopback:true,maxBytes:1024});
  await assert.rejects(fetch(`${base}/redirect`),/redirect_blocked/);
  await assert.rejects(fetch(base),/response_too_large/);
});

test('official A2A discovery/auth, submission, progress, reconnect, delivery awaits review', async t => {
  const ref = await referenceAgent(); t.after(()=>close(ref.server));
  const f = fixtureCore([{jobId:'job-1',state:'queued',transport:'a2a',provider:ref.base,capability:'test',input:{},idempotencyKey:'stable-1'}]);
  await assert.rejects(makeA2AClient(f.core,ref.base,{allowLoopback:true}),/401|fetch|HTTP/i);
  process.env.AICONNECT_INTEROP_TEST_TOKEN = token;
  t.after(()=>delete process.env.AICONNECT_INTEROP_TEST_TOKEN);
  const config={providers:{[ref.base]:{allowLoopback:true,tokenEnv:'AICONNECT_INTEROP_TEST_TOKEN'}}};
  let report=await processA2AJobs(f.core,config);
  assert.equal(report.results[0].error,undefined,JSON.stringify(report));
  assert.ok(f.jobs.get('job-1').externalTaskId);
  await new Promise(resolve=>setTimeout(resolve,70));
  // A new worker/client instance reuses persisted provider TaskId, not a second POST.
  report=await processA2AJobs(f.core,config);
  assert.equal(f.jobs.get('job-1').state,'delivered',JSON.stringify(report));
  assert.equal(f.jobs.get('job-1').localReviewStatus,'awaiting_review');
  assert.equal(f.calls.filter(c=>c.action==='service.dispatch').length,1);
  assert.ok(f.calls.filter(c=>c.action==='provider.authorize').length>=3);
});

test('official A2A cancellation and ambiguous POST is never automatically resent', async t => {
  const ref=await referenceAgent(); t.after(()=>close(ref.server));
  process.env.AICONNECT_INTEROP_TEST_TOKEN=token; t.after(()=>delete process.env.AICONNECT_INTEROP_TEST_TOKEN);
  const config={providers:{[ref.base]:{allowLoopback:true,tokenEnv:'AICONNECT_INTEROP_TEST_TOKEN'}}};
  const f=fixtureCore([{jobId:'hold',state:'queued',transport:'a2a',provider:ref.base,capability:'test',input:{hold:true},idempotencyKey:'hold'},
    {jobId:'ambiguous',state:'dispatching',transport:'a2a',provider:ref.base,capability:'test',input:{}}]);
  let r=await processA2AJobs(f.core,config);
  assert.equal(r.results.find(r=>r.jobId==='ambiguous').status,'reconciliation_required');
  f.jobs.get('hold').cancelRequested=true;
  r=await processA2AJobs(f.core,config);
  assert.equal(f.jobs.get('hold').state,'canceled',JSON.stringify(r));
  assert.equal(f.calls.filter(c=>c.action==='service.dispatch').length,1);
});

test('402 and 429 disable free provider, never invoke paid fallback', async t => {
  for (const code of [402,429]) {
    const server=createServer((req,res)=>{res.writeHead(code);res.end('{}');});
    const base=await listen(server); t.after(()=>close(server));
    const f=fixtureCore();
    await assert.rejects(makeA2AClient(f.core,base,{allowLoopback:true}),new RegExp(`provider_disabled_http_${code}`));
    assert.ok(f.calls.some(c=>c.action==='provider.check'&&c.statusCode===code));
    assert.equal(f.calls.some(c=>c.action==='service.dispatch'),false);
  }
});

test('official MCP client sees only bounded read tools; cannot invoke or approve', async () => {
  const f=fixtureCore([{jobId:'read',transport:'native',state:'delivered'}]);
  const server=makeMcpServer(f.core);
  const client=new Client({name:'test-client',version:'1'});
  const [a,b]=InMemoryTransport.createLinkedPair();
  await server.connect(a); await client.connect(b);
  try {
    const {tools}=await client.listTools();
    assert.equal(tools.length,4); assert.ok(tools.every(t=>t.annotations.readOnlyHint));
    const r=await client.callTool({name:'aiconnect_status',arguments:{}});
    assert.deepEqual(JSON.parse(r.content[0].text),{ready:true});
    const rejected=await client.callTool({name:'service_invoke',arguments:{}});
    assert.equal(rejected.isError,true);
    assert.equal(f.calls.some(c=>c.action==='service.invoke'),false);
  } finally {await client.close();await server.close();}
});

test('loopback ingress requires local bearer and fixed native route, persists SDK TaskStore', async t => {
  const f=fixtureCore();
  const bridge=await startA2AServer(f.core,{provider:'local-session',sessionId:'session-1',capability:'readonly',grantId:'grant',audience:'local-agent',token,maxPolls:1});
  t.after(()=>bridge.close());
  const denied=await fetch(`${bridge.baseUrl}/.well-known/agent-card.json`);
  assert.equal(denied.status,401);
  const browser=await fetch(`${bridge.baseUrl}/.well-known/agent-card.json`,{headers:{authorization:`Bearer ${token}`,origin:'https://evil.example'}});
  assert.equal(browser.status,403);
  const {client}=await makeA2AClient(f.core,bridge.baseUrl,{allowLoopback:true,token});
  const task=await client.sendMessage({message:{messageId:'remote-1',role:Role.ROLE_USER,parts:[text('hello')],extensions:[],referenceTaskIds:[]},configuration:{returnImmediately:true}});
  await new Promise(resolve=>setTimeout(resolve,40));
  const invoke=f.calls.find(c=>c.action==='service.invoke');
  assert.equal(invoke.provider,'local-session'); assert.equal(invoke.grantId,'grant');
  assert.ok(task.id);
  const store=new CoreTaskStore(f.core);
  const snapshot=await store.load(task.id);
  assert.equal(snapshot.id,task.id);
  const job=[...f.jobs.values()][0]; job.state='delivered';job.localReviewStatus='awaiting_review';
  const reloaded=await store.load(task.id);
  assert.equal(reloaded.status.state,TaskState.TASK_STATE_COMPLETED);
  assert.equal(reloaded.metadata.localReviewStatus,'awaiting_review');
  const fetched = await client.getTask({id:task.id});
  assert.equal(fetched.status.state,TaskState.TASK_STATE_COMPLETED);
  const wrongScope = new CoreTaskStore(f.core,'another-local-route');
  assert.equal(await wrongScope.load(task.id),undefined);
});

test('A2A/MCP normalization keeps external provenance and declares lossy unsigned mappings', () => {
  const card={name:'External agent',description:'untrusted content',supportedInterfaces:[{url:'https://agent.example/rpc',protocolBinding:'JSONRPC',protocolVersion:'1.0'}],skills:[{id:'lookup',name:'Lookup',description:'read data',tags:['read']}],defaultInputModes:['text/plain'],defaultOutputModes:['text/plain']};
  const n=normalizeAgentCard(card,'https://agent.example');
  assert.equal(n.kind,'aiconnect.external-agent');assert.equal(n.skills[0].skillId,'lookup');
  assert.equal(n.provenance.nativeAuthorSignatureVerified,false);assert.equal(n.invocationAuthorized,false);assert.equal(n.signature,undefined);
  assert.ok(n.losses.some(s=>s.includes('schema')));
  const registry=normalizeRegistryPage({servers:[{server:{name:'io.example/tool',version:'1',description:'tool metadata',packages:[{registryType:'npm',identifier:'unknown-server',version:'1'}],remotes:[{type:'streamable-http',url:'https://tool.example/mcp'}]}}],metadata:{nextCursor:'cursor'}},'https://registry.example/v0.1/servers');
  assert.equal(registry.entries[0].kind,'aiconnect.external-tool-server');assert.equal(registry.entries[0].packages[0].notInstalled,true);
  assert.deepEqual(registry.entries[0].skills,[]);assert.equal(registry.nextCursor,'cursor');assert.equal(registry.automaticNextPage,false);
  assert.throws(()=>normalizeRegistryPage({servers:[{server:{name:'invalid'}}]},'https://registry.example'),/invalid_mcp/);
  assert.throws(()=>normalizeAgentCard(card,'file:///private/path'),/invalid_metadata_source/);
});

test('MCP invocation is local opt-in and delegates authorization to Core without granting permission', async () => {
  const calls=[];
  const core=async q=>{calls.push(q);if(q.grantId!=='allowed')throw new Error('delegation_required');return{job:{jobId:'authorized-job',state:'queued',executionStarted:false}};};
  const server=makeMcpServer(core,{enableInvoke:true});
  const client=new Client({name:'opt-in-local',version:'1'});
  const [a,b]=InMemoryTransport.createLinkedPair();await server.connect(a);await client.connect(b);
  try {
    const {tools}=await client.listTools();assert.equal(tools.length,5);
    const input={transport:'a2a',provider:'https://agent.example',capability:'lookup',audience:'https://agent.example',grantId:'denied',input:{question:'hello'},idempotencyKey:'invoke-test'};
    const denied=await client.callTool({name:'aiconnect_service_invoke',arguments:input});assert.equal(denied.isError,true);
    const permitted=await client.callTool({name:'aiconnect_service_invoke',arguments:{...input,grantId:'allowed'}});
    assert.equal(JSON.parse(permitted.content[0].text).job.state,'queued');
    assert.ok(calls.every(c=>c.action==='service.invoke'&&c.declaredPrice.amount===0));
    assert.equal(calls.some(c=>c.action==='owner.grant'||c.action==='service.review'),false);
  } finally {await client.close();await server.close();}
});

test('HTTP download budget is checked before network and charged with backpressure', async t => {
  const server=createServer((req,res)=>res.end('observed-body'));
  const base=await listen(server);t.after(()=>close(server));
  const charged=[];
  const fetch=safeFetchFor(base,{allowLoopback:true,chargeDownload:async n=>charged.push(n)});
  assert.equal(await (await fetch(base)).text(),'observed-body');
  assert.equal(charged[0],0);assert.equal(charged.reduce((a,b)=>a+b,0),Buffer.byteLength('observed-body'));
  await assert.rejects(safeFetchFor(base,{allowLoopback:true,chargeDownload:async()=>{throw new Error('download_budget_exhausted');}})(base),/download_budget_exhausted/);
});
