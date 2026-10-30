#!/usr/bin/env node
/** Actual Core grants/SQLite Jobs + official SDK wire. Test-only logical HTTPS -> loopback mapping. */
import assert from 'node:assert/strict';
import { resolve,join } from 'node:path';
import { mkdir,readFile,writeFile } from 'node:fs/promises';
import { createHash,randomUUID } from 'node:crypto';
import { ClientFactory,DefaultAgentCardResolver,JsonRpcTransportFactory } from '../adapters/interop/node_modules/@a2a-js/sdk/dist/client/index.js';
import { coreClient } from '../adapters/interop/core-ipc.mjs';
import { processA2AJobs } from '../adapters/interop/a2a.mjs';
import { safeFetchFor } from '../adapters/interop/safe-http.mjs';
import { startReferenceAgent } from '../adapters/interop/test/reference-agent.mjs';
import { runFile,decodeJson } from './lib.mjs';

const binary=resolve(process.env.AICONNECT_TEST_BINARY||`target/debug/aiconnect${process.platform==='win32'?'.exe':''}`);
const root=resolve('.local/interop-a2a',randomUUID()),data=join(root,'profile');await mkdir(data,{recursive:true});
const provider='https://aiconnect-sdk-fixture.example';
const report={startedAt:new Date().toISOString(),passed:false,binarySha256:createHash('sha256').update(await readFile(binary)).digest('hex'),
  scope:'Actual independent native daemon, OwnerRoot grants, free-only registry, durable Core SQLite Jobs, official SDK client/server over real local HTTP. Test-only fetch dependency maps a reserved logical HTTPS origin to loopback; does NOT prove public HTTPS/TLS, production SSRF bypass, WAN or model execution.',checks:[]};
let started=false,reference;
async function cli(args){const out=await runFile(binary,['--data',data,'--json',...args],{timeoutMs:45000});assert.equal(out.timedOut,false);const v=decodeJson(out.stdout||out.stderr);assert.equal(out.code,0,JSON.stringify(v));return v;}
const core=coreClient(data);
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function check(label,fn){await fn();report.checks.push(label);}
let grant,firstId,cancelId;
try{
  reference=await startReferenceAgent({logicalBase:provider});
  // This dependency injection lives in the test harness only. CLI/config JSON cannot supply executable fetch callbacks.
  const wireFetch=safeFetchFor(reference.wireBase,{allowLoopback:true,token:reference.bearer,chargeDownload:bytes=>core({action:'governance.download.charge',bytes})});
  const mappedFetch=async(input,init)=>{
    const url=new URL(String(input));assert.equal(url.origin,provider);
    await core({action:'provider.authorize',url:url.href});
    return wireFetch(`${reference.wireBase}${url.pathname}${url.search}`,init);
  };
  const dependencies={makeClient:async(_core,url)=>{
    assert.equal(url,provider);
    const factory=new ClientFactory({transports:[new JsonRpcTransportFactory({fetchImpl:mappedFetch})],cardResolver:new DefaultAgentCardResolver({fetchImpl:mappedFetch}),clientConfig:{polling:true}});
    const client=await factory.createFromUrl(url);return{client,card:await client.getAgentCard()};
  }};
  const tick=()=>processA2AJobs(core,{},dependencies);
  const job=async id=>(await core({action:'service.status',jobId:id})).job;
  const invoke=async(key,input={hold:true})=>core({action:'service.invoke',transport:'a2a',provider,audience:provider,capability:'fixture.echo',input,inputSchema:{type:'object'},declaredPrice:{amount:0,currency:'USD'},grantId:grant.id,idempotencyKey:key,ttlSeconds:120});
  await check('Real Core initializes owner, explicitly attests local free fixture provider and grants scoped service permission',async()=>{
    await cli(['setup','--direct-only']);started=true;
    await core({action:'owner.init'});
    const identity=await core({action:'status'});
    await core({action:'provider.add',url:provider,free:true,termsAllowed:true,termsUrl:`${provider}/test-fixture-terms`,purpose:'Self-controlled deterministic local fixture; no external service, subscription, model or relay'});
    grant=await core({action:'owner.grant',agentId:identity.id,audience:provider,scopes:['service:fixture.echo'],maxCalls:8,ttlSeconds:180});
    assert.ok(grant.id);
  });
  await check('Core persists proposed Job; official SDK discovers authenticated card, atomically claims and observes working TaskId',async()=>{
    const created=await invoke('first');firstId=created.job.jobId;assert.equal(created.job.state,'queued');
    const r=await tick();assert.equal(r.results[0].error,undefined,JSON.stringify(r));
    await sleep(50);await tick();
    const saved=await job(firstId);assert.equal(saved.state,'working');assert.ok(saved.externalTaskId);assert.equal(reference.postCount,1);
    report.firstExternalTaskId=saved.externalTaskId;
  });
  await check('Daemon restart reloads SQLite TaskId and official SDK re-queries without duplicate send',async()=>{
    await cli(['daemon','stop']);started=false;
    await cli(['daemon','start','--direct-only']);started=true;
    await tick();const resumed=await job(firstId);assert.equal(resumed.externalTaskId,report.firstExternalTaskId);assert.equal(resumed.state,'working');assert.equal(reference.postCount,1);
    const retry=await invoke('first');assert.equal(retry.idempotent,true);assert.equal(retry.job.jobId,firstId);
  });
  await check('Provider completed becomes delivered awaiting local review, then explicit review approves',async()=>{
    reference.finish(report.firstExternalTaskId,'real Core observed official SDK result');await sleep(50);
    const r=await tick();assert.equal(r.results[0].error,undefined,JSON.stringify(r));
    const delivered=await job(firstId);assert.equal(delivered.state,'delivered');assert.equal(delivered.localReviewStatus,'awaiting_review');assert.equal(delivered.result.text,'real Core observed official SDK result');
    const reviewed=await core({action:'service.review',jobId:firstId,approve:true,text:'Locally verified deterministic fixture'});assert.equal(reviewed.job.state,'completed');assert.equal(reviewed.job.localReviewStatus,'approved');
  });
  await check('Working Job cancellation reaches official SDK cancellation and Core stores canceled without rollback promise',async()=>{
    cancelId=(await invoke('cancel')).job.jobId;
    await tick();await sleep(30);await tick();assert.equal((await job(cancelId)).state,'working');
    const cancelled=await core({action:'service.cancel',jobId:cancelId});assert.equal(cancelled.guaranteedRollback,false);
    const r=await tick();assert.equal(r.results[0].error,undefined,JSON.stringify(r));assert.equal((await job(cancelId)).state,'canceled');assert.equal(reference.postCount,2);
  });
  await check('Large Chinese provider reply is UTF-8 bounded, snapshot truncation explicit and TaskId retained',async()=>{
    const id=(await invoke('large-chinese',{largeOutput:true})).job.jobId;
    await tick();await sleep(60);const r=await tick();assert.equal(r.results[0].error,undefined,JSON.stringify(r));
    const delivered=await job(id);assert.equal(delivered.state,'delivered');assert.ok(Buffer.byteLength(delivered.result.text)<=8000);assert.ok(!delivered.result.text.includes('\ufffd'));
    assert.equal(delivered.externalTask.metadata.truncated,true);assert.ok(delivered.externalTaskId);assert.equal(reference.postCount,3);
  });
  await check('Revocation between queueing and dispatch rejects real Core claim; no new remote execution occurs',async()=>{
    const id=(await invoke('revoked')).job.jobId;
    await core({action:'owner.revoke',grantId:grant.id});const r=await tick();
    assert.ok(r.results.find(x=>x.jobId===id)?.error);assert.equal(reference.postCount,3);
  });
  const budget=await core({action:'budget.inspect'});assert.ok(budget.dailyDownloadedBytes>0);report.observedDownloadBytes=budget.dailyDownloadedBytes;
  const audit=await core({action:'audit.export',limit:1000});assert.ok(audit.events.length>0);report.auditEvents=audit.events.length;report.auditEventTypes=[...new Set(audit.events.map(e=>e.event))];
  report.passed=true;
}catch(error){report.error=error.message;process.exitCode=1;}
finally{
  if(started)try{await cli(['daemon','stop']);}catch{report.cleanupIncomplete=true;report.passed=false;process.exitCode=1;}
  await reference?.close().catch(()=>{});report.finishedAt=new Date().toISOString();
  const reportPath=join(root,'report.json');await writeFile(reportPath,JSON.stringify(report,null,2));process.stdout.write(`${JSON.stringify({...report,reportPath})}\n`);
}
