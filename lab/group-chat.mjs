#!/usr/bin/env node
import assert from 'node:assert/strict';
import {access,readFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {pathToFileURL} from 'node:url';
import {randomUUID} from 'node:crypto';
import {spawn} from 'node:child_process';
import {createLab,runFile,decodeJson,saveReport,shellQuote,testTransportArgs} from './lib.mjs';
const binary=resolve(process.env.AICONNECT_TEST_BINARY||`target/debug/aiconnect${process.platform==='win32'?'.exe':''}`);
const real=process.argv.includes('--agents'),relay=process.argv.includes('--relay-only');
const lab=await createLab(resolve('.local/group-chat'));
const peers=Object.values(lab.devices);const {a,b,c}=lab.devices;const started=new Set();
const report={startedAt:new Date().toISOString(),passed:false,realAgents:real,relayOnly:relay,checks:[],scope:'Same-host independent native profiles and workspaces; not an OS sandbox or WAN NAT test',routing:'Signed member replicas with local epoch validation; owner signs management changes, website not involved'};
async function cli(d,args,fail=false){const out=await runFile(binary,['--data',d.data,'--json',...args],{timeoutMs:45000});assert(!out.timedOut,'CLI timeout');const value=decodeJson(out.stdout||out.stderr);if(fail){assert.notEqual(out.code,0);return value;}assert.equal(out.code,0,value.error);return value;}
async function check(name,fn){await fn();report.checks.push(name);}
const id=d=>d.endpointId;
const delay=ms=>new Promise(r=>setTimeout(r,ms));
let gid,ib,ic;
try{
 for(const d of peers){await cli(d,['setup',...testTransportArgs(relay)]);started.add(d);d.endpointId=(await cli(d,['status'])).id;}
 await check('three native nodes explicitly join bound group invitations',async()=>{
   gid=(await cli(a,['group','create','--name','Multi-agent collaboration'])).group.groupId;
   ib=(await cli(a,['group','invite',gid,id(b)])).invite;
   await cli(c,['group','join',ib],true);await cli(b,['group','join',ib]);
   ic=(await cli(a,['group','invite',gid,id(c)])).invite;await cli(c,['group','join',ic]);
   assert.equal(Object.keys((await cli(b,['group','sync',gid])).group.members).length,3);
 });
 await check('member fanout is durable, deduplicated and never grants execution',async()=>{
   const mid=randomUUID();let send=await cli(b,['group','send',gid,'--text','native group B announcement','--id',mid]);
   const deadline=Date.now()+45000;while(send.deliveries.some(x=>x.status==='pending')&&Date.now()<deadline){await delay(300);send=await cli(b,['group','send',gid,'--text','native group B announcement','--id',mid]);}
   assert.equal(send.deliveries.length,2);assert(send.deliveries.every(x=>x.status==='acked'),JSON.stringify(send));
   await cli(b,['group','send',gid,'--text','native group B announcement','--id',mid]);
   for(const d of peers){const rows=(await cli(d,['group','messages',gid])).messages;assert.equal(rows.length,1);assert.equal(rows[0].sender,id(b));assert.equal(rows[0].executionAllowed,false);}
   await cli(b,['contacts','import',await exportCard(a,b)]);
   await cli(b,['contacts','policy',id(a),'--wake','true','--task','true']);
   await cli(b,['agent','attach','--command',process.execPath,'--arg',resolve('audit/fixtures/runtime-marker-adapter.mjs'),'--workspace',b.workspace]);
   await cli(a,['group','send',gid,'--text','A group announcement must not execute']);await delay(800);
   await assert.rejects(access(join(b.workspace,'executions.jsonl')));await cli(b,['agent','detach']);
   const path=(await cli(b,['status'])).lastPath;assert(relay?path.relay&&!path.direct:path.direct,'selected transport not observed');report.observedPath=path;
 });
 await check('group inbox watch resumes a cursor and streams new messages',async()=>{
   const cursor=(await cli(a,['group','inbox'])).nextAfter;
   const watching=spawn(binary,['--data',a.data,'--json','group','inbox','--after',String(cursor),'--watch'],{windowsHide:true,stdio:['ignore','pipe','pipe']});
   try{
     const message=new Promise((yes,no)=>{let buffer='';const timer=setTimeout(()=>no(Error('group watch deadline')),8000);watching.once('error',no);watching.stdout.on('data',chunk=>{buffer+=chunk;const lines=buffer.split('\n');buffer=lines.pop();for(const line of lines){if(!line.trim())continue;const event=JSON.parse(line);if(event.text==='live group watch'){clearTimeout(timer);yes(event);}}});});
     await delay(200);await cli(c,['group','send',gid,'--text','live group watch']);
     const event=await message;assert.equal(event.groupId,gid);assert.equal(event.sender,id(c));assert(event.sequence>cursor);
   }finally{watching.kill();}
 });
 await check('group roles, mute and protected owner enforced remotely',async()=>{
   await cli(b,['group','role',gid,id(c),'admin'],true);
   await cli(a,['group','role',gid,id(b),'admin']);
   await cli(b,['group','policy',gid,id(c),'--can-send','false']);
   await cli(c,['group','send',gid,'--text','muted attempt'],true);
   await cli(b,['group','policy',gid,id(c),'--can-send','true']);
   await cli(b,['group','remove',gid,id(a)],true);await cli(b,['group','dissolve',gid],true);
 });
 if(real){await check('three real Agents announce and read each other through the group',async()=>{
   const {runTask}=await import(pathToFileURL(resolve('adapters/codex-app-server.mjs')));const markers=peers.map(()=>`GROUP-${randomUUID()}`);
   const outcomes=await Promise.allSettled(peers.map((d,i)=>{
     const prefix=process.platform==='win32'?'& ':'';const cmd=`${prefix}${shellQuote(binary)} --data ${shellQuote(d.data)} --json`;
     const prompt=`You are Agent ${d.name} in an authorized three-Agent AIConnect group test. Only use your own workspace and the allowed native CLI/profile. Do not delegate or read other workspaces/credentials. Group ID ${gid}. CLI syntax ${process.platform==='win32'?'PowerShell':'POSIX'}. First send exactly one announcement with: ${cmd} group send ${shellQuote(gid)} --text ${shellQuote(markers[i])}. Then poll ${cmd} group messages ${shellQuote(gid)} until you see three different messages starting GROUP- from three different senders (up to 40 polls, wait 2 seconds each). Do not reply or trigger any tasks. Save group-observed.json in your own workspace with {groupId,announcements:[{sender,text}]} containing only these three GROUP- messages. Stop after the file is written. All participants are already authorized members.`;
     return runTask({v:1,taskId:randomUUID(),messageId:randomUUID(),sessionId:gid,sender:'local-authorized-user',text:prompt,workspace:d.workspace,expiresAt:Date.now()+200000,budget:{timeoutMs:180000,maxTurns:1},permissions:{wake:true,task:true},sandbox:'workspace-write'},{cwd:d.workspace,role:'coordinator',networkAccess:true});
   }));
   for(let i=0;i<peers.length;i++){const result=outcomes[i];assert.equal(result.status,'fulfilled');assert.equal(result.value.status,'completed',result.value.error);assert(result.value.runtime.toolCount>0);const observed=JSON.parse(await readFile(join(peers[i].workspace,'group-observed.json'),'utf8'));assert.equal(observed.groupId,gid);assert.equal(observed.announcements.length,3);for(let j=0;j<peers.length;j++)assert(observed.announcements.some(x=>x.sender===id(peers[j])&&x.text===markers[j]));}
   report.agents=outcomes.map(x=>({status:x.value.status,tools:x.value.runtime.toolCount}));
 });}
 await check('removed member cannot use old invitation; restart retains group history',async()=>{
   await cli(b,['group','remove',gid,id(c)]);await cli(c,['group','send',gid,'--text','removed attempt'],true);await cli(c,['group','join',ic],true);
   const prior=(await cli(b,['group','messages',gid])).messages.length;
   await cli(b,['daemon','stop']);started.delete(b);await cli(b,['daemon','start',...testTransportArgs(relay)]);started.add(b);
   assert.equal((await cli(b,['group','messages',gid])).messages.length,prior);assert.equal((await cli(b,['group','sync',gid])).group.members[id(b)].role,'admin');
   await cli(a,['group','dissolve',gid]);await cli(b,['group','send',gid,'--text','dissolved attempt'],true);
 });
 report.passed=true;
}catch(e){report.error=e.message;process.exitCode=1;}
finally{for(const d of started){try{await cli(d,['daemon','stop']);}catch{report.cleanupIncomplete=true;report.passed=false;process.exitCode=1;}}report.finishedAt=new Date().toISOString();console.log(JSON.stringify({...report,reportPath:await saveReport(lab.runDirectory,report)}));}

async function exportCard(from,to){const {writeFile}=await import('node:fs/promises');const file=join(to.workspace,'contact-card.json');await writeFile(file,JSON.stringify(await cli(from,['identity'])));return file;}
