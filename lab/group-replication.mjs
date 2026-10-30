#!/usr/bin/env node
import assert from 'node:assert/strict';
import {mkdir,readFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {createHash} from 'node:crypto';
import {createLab,runFile,decodeJson,saveReport,testTransportArgs} from './lib.mjs';
const binary=resolve(process.env.AICONNECT_TEST_BINARY||`target/debug/aiconnect${process.platform==='win32'?'.exe':''}`);
const relay=process.argv.includes('--relay-only'),lab=await createLab(resolve('.local/group-replication'));
const {a,b,c}=lab.devices,d={name:'d',data:join(lab.runDirectory,'d','data')};await mkdir(d.data,{recursive:true});
const devices=[a,b,c,d],started=new Set(),report={startedAt:new Date().toISOString(),passed:false,relayOnly:relay,checks:[],scope:'Four independent native processes and SQLite profiles on one host; no models, WAN claim or deployment',binarySha256:createHash('sha256').update(await readFile(binary)).digest('hex')};
async function cli(p,args,fail=false){const out=await runFile(binary,['--data',p.data,'--json',...args],{timeoutMs:45000});assert(!out.timedOut,'CLI timeout');const v=decodeJson(out.stdout||out.stderr);assert.equal(out.code===0,!fail,v.error);return v;}
async function start(p){await cli(p,['setup',...testTransportArgs(relay)]);started.add(p);const id=(await cli(p,['status'])).id;if(p.id)assert.equal(id,p.id);p.id=id;}
async function stop(p){await cli(p,['daemon','stop']);started.delete(p);}
async function check(name,fn){await fn();report.checks.push(name);}
async function messages(p,gid){return(await cli(p,['group','messages',gid])).messages;}
async function waitFor(p,gid,text){const until=Date.now()+45000;while(Date.now()<until){const rows=await messages(p,gid);if(rows.some(r=>r.text===text))return rows;await cli(p,['group','sync',gid]);await new Promise(r=>setTimeout(r,200));}throw Error('replica convergence deadline');}
let gid;
try{
 await check('new groups default to signed member replication; four identities explicitly join',async()=>{for(const p of devices)await start(p);const created=await cli(a,['group','create','--name','Replicated group']);gid=created.group.groupId;assert.equal(created.group.replication,'members-v1');for(const p of [b,c,d]){const invite=await cli(a,['group','invite',gid,p.id]);await cli(p,['group','join',invite.invite]);}});
 await check('owner offline: sender persists and another member receives signed message',async()=>{await stop(d);await stop(a);await cli(b,['group','send',gid,'--text','replica survives original sender']);const rows=await waitFor(c,gid,'replica survives original sender');assert.equal(rows[0].signatureVerified,true);assert.equal(rows[0].executionAllowed,false);});
 await check('owner and original sender offline: restarted member pulls the surviving replica',async()=>{await stop(b);await start(d);const rows=await waitFor(d,gid,'replica survives original sender');assert.equal(rows.filter(r=>r.text==='replica survives original sender').length,1);assert(!started.has(a)&&!started.has(b));});
 await check('remaining members chat without owner; repeat sync is idempotent and restart retains log',async()=>{await cli(d,['group','send',gid,'--text','members continue independently']);await waitFor(c,gid,'members continue independently');for(let n=0;n<2;n++)await cli(d,['group','sync',gid]);const count=(await messages(d,gid)).length;await stop(d);await start(d);assert.equal((await messages(d,gid)).length,count);report.observedPath=(await cli(c,['status'])).lastPath;assert.equal(report.observedPath.relay,relay);});
 await check('owner returns: policy updates and removal deny further current group traffic',async()=>{await start(a);await cli(a,['group','policy',gid,c.id,'--can-send','false']);await cli(c,['group','send',gid,'--text','must reject muted sender'],true);await cli(a,['group','remove',gid,d.id]);await cli(d,['group','sync',gid],true);});
 report.passed=true;
}catch(e){report.error=e.message;process.exitCode=1;}
finally{for(const p of started){try{await stop(p);}catch{report.cleanupIncomplete=true;report.passed=false;process.exitCode=1;}}report.finishedAt=new Date().toISOString();console.log(JSON.stringify({...report,reportPath:await saveReport(lab.runDirectory,report)}));}
