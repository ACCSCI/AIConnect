#!/usr/bin/env node
import assert from 'node:assert/strict';
import {writeFile,readFile,access} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {randomUUID} from 'node:crypto';
import {createLab,runFile,decodeJson,saveReport,shellQuote} from './lib.mjs';

const binary=resolve(process.env.AICONNECT_TEST_BINARY||`target/debug/aiconnect${process.platform==='win32'?'.exe':''}`);
const realAgents=process.argv.includes('--agents');
const lab=await createLab(resolve('.local/task-flow'));
const {a,b}=lab.devices;
const active=new Set();
const report={startedAt:new Date().toISOString(),passed:false,realAgents,checks:[],isolation:'Two local processes/profiles/workspaces under one OS user, not an OS security boundary',network:'Actual native iroh direct communication; no supervisor forwarding'};
async function cli(device,args,failure=false){
 const out=await runFile(binary,['--data',device.data,'--json',...args],{timeoutMs:35000});
 assert(!out.timedOut,'CLI timed out');const value=decodeJson(out.stdout||out.stderr);
 if(failure){assert.notEqual(out.code,0);return value;}
 assert.equal(out.code,0,value.error);return value;
}
async function check(name,fn){await fn();report.checks.push(name);}
function decodeCard(card){return JSON.parse(Buffer.from(card.body,'base64url'));}
const wait=ms=>new Promise(r=>setTimeout(r,ms));
try{
 for(const d of [a,b]){await cli(d,['setup','--direct-only']);active.add(d);}
 const ac=await cli(a,['identity']),bc=await cli(b,['identity']);
 for(const [d,card] of [[a,bc],[b,ac]]){const file=join(d.workspace,'contact.json');await writeFile(file,JSON.stringify(card));await cli(d,['contacts','import',file]);}
 const sid=(await cli(a,['connect',decodeCard(bc).id])).session.id;
 let taskId;
 await check('native CLI proposal/accept/deliver/request-changes/redeliver/review',async()=>{
   const proposed=await cli(a,['task','propose',sid,'--title','CLI task confirmation','--text','Read synthetic inventory','--ttl','2m','--contract-id','reserved-only']);
   taskId=proposed.task.taskId;assert.equal(proposed.task.state,'proposed');assert.equal(proposed.delivery.status,'acked');
   assert.equal((await cli(b,['task','show',taskId])).task.contractId,'reserved-only');
   await cli(a,['task','accept',taskId],true);
   const event=randomUUID();await cli(b,['task','accept',taskId,'--event-id',event]);await cli(b,['task','accept',taskId,'--event-id',event]);
   await cli(a,['task','review',taskId,'--approve','--note','too early'],true);
   await cli(b,['task','deliver',taskId,'--text','first result']);
   await cli(a,['task','review',taskId,'--reject','--note','include version']);
   await cli(b,['task','deliver',taskId,'--text','corrected result v2','--sha256','b'.repeat(64)]);
   await cli(a,['task','review',taskId,'--approve','--note','verified']);
   assert.equal((await cli(b,['task','show',taskId])).task.state,'completed');
   assert.equal((await cli(a,['task','history',taskId])).messages.length,6);
   await cli(b,['task','review',taskId,'--approve','--note','self approve'],true);
   await cli(b,['task','deliver',taskId,'--text','late overwrite'],true);
 });
 await check('proposal receipt and explicit acceptance do not auto-execute an attached runtime',async()=>{
   await cli(b,['contacts','policy',decodeCard(ac).id,'--wake','true','--task','true']);
   await cli(b,['agent','attach','--command',process.execPath,'--arg',resolve('audit/fixtures/runtime-marker-adapter.mjs'),'--workspace',b.workspace]);
   const p=await cli(a,['task','propose',sid,'--text','Must wait for explicit agent work','--ttl','2m']);
   await cli(b,['task','accept',p.task.taskId]);await wait(900);
   await assert.rejects(access(join(b.workspace,'executions.jsonl')));
   assert.equal(Object.keys((await cli(b,['agent','status'])).state.claims).length,0);
   await cli(b,['agent','detach']);
 });
 await check('task state and event history survive daemon restart',async()=>{
   await cli(b,['daemon','stop']);active.delete(b);await cli(b,['daemon','start','--direct-only']);active.add(b);
   assert.equal((await cli(b,['task','show',taskId])).task.state,'completed');
   assert.equal((await cli(b,['task','history',taskId])).messages.length,6);
 });
 if(realAgents){
   await check('two real Agents negotiate confirmation and review a fixture delivery',async()=>{
     const {runTask}=await import(pathToFileURL(resolve('adapters/codex-app-server.mjs')));
     const prefix=process.platform==='win32'?'& ':'';
     const command=d=>`${prefix}${shellQuote(binary)} --data ${shellQuote(d.data)} --json`;
     const aCmd=command(a),bCmd=command(b);
     const title=`Real-confirmation-${randomUUID().slice(0,8)}`;
     const aPrompt=`You are Agent A, requester, in an authorized local AIConnect task confirmation test. Only use your workspace and the explicitly allowed AIConnect executable/profile. Never read B's files, credentials, or user files. Do not delegate. Commands use ${process.platform==='win32'?'PowerShell':'POSIX shell'} syntax. CLI base: ${aCmd}.\nFirst run: ${aCmd} task propose ${shellQuote(sid)} --title ${shellQuote(title)} --text ${shellQuote('Read only settings.json in your own configured workspace and deliver its exact JSON contents. Do not migrate or change anything.')} --ttl 5m\nCapture task.taskId. Poll 'task show <ID>' up to 40 times, waiting 2 seconds each time. Delivery is not completion. Once state is delivered, parse task.result.text as JSON, check it includes device, fixtureMarker, editor, tools, then write confirmation-plan.json in your own workspace with {taskId:<ID>,sourceDevice:<device>,fixtureMarker:<marker>,editor:<editor>,tools:<tools>}. Explicitly approve using 'task review <ID> --approve --note "Checked required fixture fields"'. Never approve before delivered. Return after completed. No other networking or system changes.`;
     const bPrompt=`You are Agent B, assignee, in an authorized local AIConnect task confirmation test. Only use your workspace and the explicitly allowed AIConnect executable/profile. Never read credentials or other files; do not delegate. CLI base (${process.platform==='win32'?'PowerShell':'POSIX shell'}): ${bCmd}.\nPoll 'task list' up to 40 times waiting 2 seconds between polls for title ${title}. Ignore all other tasks. When that task appears in proposed state, explicitly call 'task accept <taskId>'. Only then read settings.json in your own workspace using a tool. Deliver its exact JSON contents using 'task deliver <ID> --text <JSON-string>'; use correct shell argument quoting. Do not approve your own delivery. Poll 'task show <ID>' until state completed, then stop. Do not modify source files, migrate software, access other workspaces or send legacy --kind task messages.`;
     const run=(device,prompt)=>runTask({v:1,taskId:randomUUID(),messageId:randomUUID(),sessionId:sid,sender:'local-authorized-user',text:prompt,workspace:device.workspace,expiresAt:Date.now()+200000,budget:{timeoutMs:180000,maxTurns:1},permissions:{wake:true,task:true},sandbox:'workspace-write'},{cwd:device.workspace,role:'coordinator',networkAccess:true});
     const outcomes=await Promise.allSettled([run(a,aPrompt),run(b,bPrompt)]);
     const values=outcomes.map(o=>{assert.equal(o.status,'fulfilled');assert.equal(o.value.status,'completed',o.value.error);assert(o.value.runtime.toolCount>0);return o.value;});
     const plan=JSON.parse(await readFile(join(a.workspace,'confirmation-plan.json'),'utf8'));
     assert.equal(plan.sourceDevice,lab.source.device);assert.equal(plan.fixtureMarker,lab.source.fixtureMarker);assert.deepEqual(plan.editor,lab.source.editor);assert.deepEqual(plan.tools,lab.source.tools);
     assert.equal((await cli(a,['task','show',plan.taskId])).task.state,'completed');
     const events=(await cli(b,['task','history',plan.taskId])).messages.map(m=>JSON.parse(m.text).type);
     assert.deepEqual(events,['proposed','accepted','delivered','approved']);
     report.agents=values.map(v=>({status:v.status,tools:v.runtime.toolCount}));report.realTaskId=plan.taskId;report.realEvents=events;
     assert.deepEqual(JSON.parse(await readFile(join(b.workspace,'settings.json'),'utf8')),lab.source);
   });
 }
 report.passed=true;
}catch(e){report.error=e.message;process.exitCode=1;}
finally{
 for(const d of active){try{await cli(d,['daemon','stop']);}catch{report.cleanupIncomplete=true;report.passed=false;process.exitCode=1;}}
 report.finishedAt=new Date().toISOString();const reportPath=await saveReport(lab.runDirectory,report);console.log(JSON.stringify({...report,reportPath}));
}
