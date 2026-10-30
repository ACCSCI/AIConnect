#!/usr/bin/env node
// Real native processes, loopback IPC load generation; no model calls or message forwarding.
import {spawn,execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {createConnection} from 'node:net';
import {mkdir,mkdtemp,readFile,writeFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {randomUUID,createHash} from 'node:crypto';
import {performance} from 'node:perf_hooks';
import {DatabaseSync} from 'node:sqlite';
import {ownerOfflineDelivery} from './six-layer-pressure-metrics.mjs';
const exec=promisify(execFile),argv=process.argv.slice(2);
const opt=(k,d)=>{const i=argv.indexOf(k);return i<0?d:argv[i+1]};
const binary=resolve(opt('--binary',`target/release/aiconnect${process.platform==='win32'?'.exe':''}`));
// Preserve the original backpressure benchmark's protocol; opt into replica measurements explicitly.
const ownerOnline=!argv.includes('--replicated');
const sizes=opt('--sizes','16,32,64').split(',').map(Number);
const steadyCount=Number(opt('--steady','16')),burstCount=Number(opt('--burst','8')),width=Number(opt('--concurrency','4'));
if(!sizes.every(n=>Number.isInteger(n)&&n>=2&&n<=64)||![steadyCount,burstCount,width].every(n=>Number.isInteger(n)&&n>0&&n<=64))throw Error('invalid workload');
const root=resolve('.local/group-pressure');await mkdir(root,{recursive:true});const runDir=await mkdtemp(join(root,'run-'));
const delay=ms=>new Promise(r=>setTimeout(r,ms));
async function pool(items,n,fn){const results=new Array(items.length);let next=0;await Promise.all(Array.from({length:Math.min(n,items.length)},async()=>{while(next<items.length){const i=next++;results[i]=await fn(items[i],i);}}));return results;}
function stats(values){const a=values.filter(Number.isFinite).sort((a,b)=>a-b);const p=q=>a.length?a[Math.min(a.length-1,Math.ceil(q*a.length)-1)]:null;return{samples:a.length,minMs:a[0]??null,p50Ms:p(.5),p95Ms:p(.95),maxMs:a.at(-1)??null};}
function rpc(node,request,timeoutMs=45000){return new Promise((yes,no)=>{
 if(!node.info||node.exited)return no(Error('node_not_running'));
 const [host,port]=node.info.address.split(':');const socket=createConnection({host,port:Number(port)});let buffer='',done=false;
 const finish=(e,v)=>{if(done)return;done=true;socket.destroy();e?no(e):yes(v);};
 socket.setTimeout(timeoutMs,()=>finish(Error('ipc_timeout')));socket.once('error',e=>finish(Error(`ipc_${e.code||'error'}`)));socket.once('end',()=>{if(!done)finish(Error('ipc_closed'))});
 socket.once('connect',()=>socket.write(JSON.stringify({token:node.info.token,request})+'\n'));
 socket.on('data',chunk=>{buffer+=chunk;if(buffer.length>2_000_000)return finish(Error('ipc_response_limit'));if(buffer.includes('\n')){try{const v=JSON.parse(buffer.slice(0,buffer.indexOf('\n')));v.ok?finish(null,v.result):finish(Error(v.error||'ipc_error'));}catch{finish(Error('ipc_invalid_response'));}}});
});}
async function launch(node){
 await mkdir(node.data,{recursive:true});node.exited=false;node.stderr='';
 node.child=spawn(binary,['--data',node.data,'--json','daemon','run','--direct-only'],{windowsHide:true,stdio:['ignore','ignore','pipe'],env:{...process.env,TOKIO_WORKER_THREADS:'2'}});
 node.child.stderr.on('data',b=>{node.stderr=(node.stderr+b).slice(-2000)});node.child.once('error',e=>{node.error=e.code;node.exited=true});node.child.once('exit',()=>{node.exited=true});
 const deadline=Date.now()+20000;
 while(Date.now()<deadline){if(node.exited)throw Error(`node_start_failed:${node.index}:${node.error||node.stderr}`);try{node.info=JSON.parse(await readFile(join(node.data,'daemon.json'),'utf8'));const s=await rpc(node,{action:'status'},1000);node.id=s.id;return;}catch{}await delay(50);}
 throw Error('node_start_deadline');
}
async function stop(node){if(!node.child||node.exited)return;try{await rpc(node,{action:'daemon.stop'},12000);}catch{}const end=Date.now()+12000;while(!node.exited&&Date.now()<end)await delay(50);if(!node.exited){node.child.kill();const end=Date.now()+3000;while(!node.exited&&Date.now()<end)await delay(50);}if(!node.exited)throw Error('cleanup_incomplete');}
async function memory(nodes){
 const ids=nodes.filter(n=>!n.exited&&n.child?.pid).map(n=>n.child.pid);
 if(process.platform!=='win32')return{unavailable:'native process memory sampler implemented for Windows only'};
 const script=`$p=@(); ${ids.length?`$p=@(Get-Process -Id @(${ids.join(',')}) -ErrorAction SilentlyContinue | ForEach-Object { [pscustomobject]@{pid=$_.Id;workingSet=$_.WorkingSet64;privateBytes=$_.PrivateMemorySize64;cpuSeconds=$_.CPU} });`:''} $o=Get-CimInstance Win32_OperatingSystem; [pscustomobject]@{freeBytes=([long]$o.FreePhysicalMemory*1024);processes=$p}|ConvertTo-Json -Compress -Depth 4`;
 const {stdout}=await exec('powershell.exe',['-NoProfile','-Command',script],{windowsHide:true,timeout:10000});const m=JSON.parse(stdout.trim());
 return{at:new Date().toISOString(),freeBytes:m.freeBytes,totalWorkingSetBytes:m.processes.reduce((s,p)=>s+p.workingSet,0),totalPrivateBytes:m.processes.reduce((s,p)=>s+p.privateBytes,0),ownerPrivateBytes:m.processes.find(p=>p.pid===nodes[0]?.child?.pid)?.privateBytes??null,maxNodePrivateBytes:Math.max(0,...m.processes.map(p=>p.privateBytes)),processes:m.processes,harnessRss:process.memoryUsage().rss};
}
async function histories(nodes,gid,deadline=Infinity){return pool(nodes,8,async n=>{const rows=[];let after=0;while(true){const remaining=deadline-performance.now();if(remaining<=0)throw Error('history_poll_deadline');const page=await rpc(n,{action:'groups.messages',groupId:gid,after},Math.max(1,Math.min(45000,remaining)));rows.push(...page.messages);if(!page.hasMore)break;after=page.nextAfter;}return rows;});}
async function settleOwnerOffline(nodes,gid,attempts,maxMs=45000){
 const began=performance.now(),deadline=began+maxMs,online=nodes.filter(n=>n.index!==0);let final=ownerOfflineDelivery(nodes,attempts,new Map()),pollError;
 while(final.receivedDeliveries<final.expectedDeliveries&&performance.now()<deadline){
  try{const rows=await histories(online,gid,deadline);final=ownerOfflineDelivery(nodes,attempts,new Map(online.map((n,i)=>[n.index,rows[i]])));}
  catch(error){pollError=error.message;break;}
  if(final.receivedDeliveries===final.expectedDeliveries)break;
  const remaining=deadline-performance.now();if(remaining>0)await delay(Math.min(200,remaining));
 }
 return{...final,waitBudgetMs:maxMs,drainMs:performance.now()-began,timedOut:final.receivedDeliveries<final.expectedDeliveries,pollError};
}
async function settle(nodes,gid,sends,maxMs=45000){const began=performance.now();let final;
 do{const rows=await histories(nodes,gid);let received=0,acceptedReceived=0;for(const sent of sends){sent.receivedRecipients=0;for(let i=0;i<nodes.length;i++){if(i===sent.sender)continue;if(rows[i].some(m=>m.id===sent.id&&m.sender===nodes[sent.sender].id)){received++;sent.receivedRecipients++;if(sent.accepted)acceptedReceived++;}}}
 const expected=sends.length*(nodes.length-1),acceptedMessages=sends.filter(s=>s.accepted).length,acceptedExpected=acceptedMessages*(nodes.length-1);final={attemptedMessages:sends.length,acceptedMessages,expectedDeliveries:expected,receivedDeliveries:received,deliveryRate:expected?received/expected:1,acceptedDeliveryRate:acceptedExpected?acceptedReceived/acceptedExpected:1,completionLatency:stats(sends.filter(s=>s.accepted).map(s=>s.allAckMs)),fullAckPending:sends.filter(s=>s.accepted&&!Number.isFinite(s.allAckMs)).length,drainMs:performance.now()-began};if(acceptedReceived===acceptedExpected&&final.fullAckPending===0)break;await delay(200);
 }while(performance.now()-began<maxMs);return final;}
async function runSize(count){
 const directory=join(runDir,`nodes-${count}`);await mkdir(directory,{recursive:true});
 const nodes=Array.from({length:count},(_,index)=>({index,data:join(directory,`node-${index}`)}));
 const result={nodes:count,startedAt:new Date().toISOString(),passed:false,protocol:ownerOnline?'owner-online-v1':'members-v1',workload:{steady:steadyCount,burst:burstCount,concurrentSenders:width,payloadBytes:256,tokioWorkersPerNode:2},memorySamples:[],messages:[]};
 let sampler, ackTimer, sampling=false,resourceError;
 const readers=new Map(),observed=new Map();
 function reader(sender){if(!readers.has(sender)){const db=new DatabaseSync(join(nodes[sender].data,'local.sqlite'),{readOnly:true});db.exec('PRAGMA busy_timeout=100');readers.set(sender,{db,query:db.prepare("SELECT COUNT(*) n,SUM(status='acked') acked,SUM(status='rejected') rejected FROM group_outbox WHERE message_id=?")});}return readers.get(sender);}
 function observeAcks(){for(const [id,item]of observed){try{const q=reader(item.entry.sender).query.get(id);if(q.n===count-1){item.entry.accepted=true;if(q.acked===count-1){item.entry.allAckMs=performance.now()-item.start;observed.delete(id);}else if(q.rejected>0){item.entry.rejectedAfterQueue=q.rejected;}}else if(item.entry.returned){observed.delete(id);}}catch(e){result.ackObservationError=e.message;}}}
 const save=()=>writeFile(join(directory,'report.json'),JSON.stringify(result,null,2)+'\n');
 async function sample(){if(sampling)return;sampling=true;try{const m=await memory(nodes);result.memorySamples.push(m);if(m.freeBytes<1.5*1024**3)resourceError='resource_guard_less_than_1_5_GiB_free';}catch(e){result.memorySamplingError=e.message;}finally{sampling=false;}}
 try{
  notify(`Starting ${count} native nodes`);const before=await memory([]);result.memoryBefore=before;
  await pool(nodes,4,launch);if(new Set(nodes.map(n=>n.id)).size!==count)throw Error('identities_not_distinct');
  await sample();sampler=setInterval(()=>sample(),2500);
  if(resourceError)throw Error(resourceError);
  const created=await rpc(nodes[0],{action:'groups.create',name:`Pressure ${count}`,maxMembers:count,ownerOnline});const gid=created.group.groupId;
  const joinTimes=[];for(const node of nodes.slice(1)){if(resourceError)throw Error(resourceError);const t=performance.now();const invitation=await rpc(nodes[0],{action:'groups.invite',groupId:gid,peer:node.id});await rpc(node,{action:'groups.join',invite:invitation.invite});joinTimes.push(performance.now()-t);}
  const admitted=(await rpc(nodes[0],{action:'groups.show',groupId:gid})).group;
  if(admitted.maxMembers!==count)throw Error('configured_member_limit_not_applied');
  result.admission={members:Object.keys(admitted.members).length,joinLatency:stats(joinTimes),snapshotRawBytes:Buffer.byteLength(JSON.stringify(admitted))};
  notify(`${count}: all members admitted, snapshot ${result.admission.snapshotRawBytes} bytes`);await sample();
  ackTimer=setInterval(observeAcks,100);
  async function send(sender,phase,index){if(resourceError)throw Error(resourceError);const id=randomUUID(),text=`bench:${count}:${phase}:${index}:`.padEnd(256,'x');const t=performance.now();let response,error;
   const entry={id,sender,phase};reader(sender);observed.set(id,{entry,start:t});result.messages.push(entry);
   try{response=await rpc(nodes[sender],{action:'groups.send',groupId:gid,id,text});}catch(e){error=e.message;}
   Object.assign(entry,{returned:true,requestMs:performance.now()-t,error,acked:response?.deliveries?.filter(d=>d.status==='acked').length??0,pending:response?.deliveries?.filter(d=>d.status==='pending').length??0,rejected:response?.deliveries?.filter(d=>d.status==='rejected').length??0});if(entry.acked===count-1){entry.accepted=true;entry.allAckMs??=entry.requestMs;observed.delete(id);}observeAcks();return entry;
  }
  const warmup=[];for(let i=0;i<2;i++)warmup.push(await send(1+(i%(count-1)),'warmup',i));result.warmup=await settle(nodes,gid,warmup);
  const steady=[];for(let i=0;i<steadyCount;i++){steady.push(await send(1+i%(count-1),'steady',i));if(i%4===3)notify(`${count}: steady ${i+1}/${steadyCount}`);}
  const steadyDelivery=await settle(nodes,gid,steady);
  result.steady={requestLatency:stats(steady.map(x=>x.requestMs)),allAckLatency:stats(steady.filter(x=>x.acked===count-1).map(x=>x.requestMs)),initialAckRate:steady.reduce((s,x)=>s+x.acked,0)/(steady.length*(count-1)),requestErrors:steady.filter(x=>x.error).length,...steadyDelivery};await save();
  notify(`${count}: steady received ${(steadyDelivery.deliveryRate*100).toFixed(2)}%`);
  const burst=await pool(Array.from({length:burstCount},(_,i)=>i),width,i=>send(1+i%(count-1),'burst',i));
  result.burst={requestLatency:stats(burst.map(x=>x.requestMs)),allAckLatency:stats(burst.filter(x=>x.acked===count-1).map(x=>x.requestMs)),initialAckRate:burst.reduce((s,x)=>s+x.acked,0)/(burst.length*(count-1)),requestErrors:burst.filter(x=>x.error).length,...await settle(nodes,gid,burst)};await save();
  notify(`${count}: burst received ${(result.burst.deliveryRate*100).toFixed(2)}%`);
  await stop(nodes[0]);const offline=[];
  await pool([1,2,3].filter(i=>i<count),3,async sender=>{const id=randomUUID(),t=performance.now();let error;try{await rpc(nodes[sender],{action:'groups.send',groupId:gid,id,text:ownerOnline?'owner offline must fail closed':'online members continue while owner is offline'});}catch(e){error=e.message;}offline.push({sender,id,ms:performance.now()-t,error});});
  result.ownerOffline={attempts:offline,allRejected:offline.every(x=>!!x.error),allAccepted:offline.every(x=>!x.error)};
  if(ownerOnline){const offRows=await histories(nodes.slice(1),gid);result.ownerOffline.persistedOfflineAttempts=offRows.reduce((sum,rows)=>sum+rows.filter(r=>offline.some(x=>x.id===r.id)).length,0);}
  else{result.ownerOffline.delivery=await settleOwnerOffline(nodes,gid,offline);result.ownerOffline.persistedOfflineAttempts=result.ownerOffline.delivery.receivedDeliveries;result.ownerOffline.persistedOfflineAttemptsDefinition='Online non-sender recipient copies only; offline owner and sender copies excluded';}
  await launch(nodes[0]);const recovery=await send(1,'owner-recovery',0);result.ownerRecovery={...await settle(nodes,gid,[recovery]),requestMs:recovery.requestMs,restoredMembers:Object.keys((await rpc(nodes[0],{action:'groups.show',groupId:gid})).group.members).length};
  await sample();result.memory={peakPrivateBytes:Math.max(...result.memorySamples.map(m=>m.totalPrivateBytes||0)),peakWorkingSetBytes:Math.max(...result.memorySamples.map(m=>m.totalWorkingSetBytes||0)),peakOwnerPrivateBytes:Math.max(...result.memorySamples.map(m=>m.ownerPrivateBytes||0)),maxNodePrivateBytes:Math.max(...result.memorySamples.map(m=>m.maxNodePrivateBytes||0)),minFreeBytes:Math.min(...result.memorySamples.filter(m=>m.freeBytes).map(m=>m.freeBytes))};
  const offlineExpected=ownerOnline?(result.ownerOffline.allRejected&&result.ownerOffline.persistedOfflineAttempts===0):(result.ownerOffline.allAccepted&&result.ownerOffline.delivery.deliveryRate===1&&!result.ownerOffline.delivery.pollError);
  result.passed=result.admission.members===count&&result.steady.deliveryRate===1&&result.burst.deliveryRate===1&&result.steady.requestErrors===0&&result.burst.requestErrors===0&&offlineExpected&&result.ownerRecovery.deliveryRate===1;
 }catch(e){result.error=e.message;notify(`${count}: case error ${e.message}`);}
 finally{clearInterval(sampler);clearInterval(ackTimer);for(const r of readers.values())r.db.close();while(sampling)await delay(50);await pool(nodes,8,async n=>{try{await stop(n);}catch{result.cleanupIncomplete=true;result.passed=false;}});result.finishedAt=new Date().toISOString();await save();}
 return result;
}
function notify(message){console.log(JSON.stringify({at:new Date().toISOString(),progress:message}));}
const summary={startedAt:new Date().toISOString(),scope:'One Windows host, real independent release Rust daemons, direct-only iroh; no models, no WAN/relay capacity inference',binary:{path:binary,sha256:createHash('sha256').update(await readFile(binary)).digest('hex')},definitions:{latency:'requestLatency=IPC send wall time; completionLatency=all-recipient durable ACK, including retries, observed from sender outbox read-only at 100ms resolution; warmup drained first',deliveryRate:'Recipient histories read independently; sender local copy excluded. deliveryRate includes failed intents; acceptedDeliveryRate includes only messages with a real persisted fanout outbox',memory:'Sampled every 2.5s: sum native private committed bytes plus working sets; summed working sets may double-count shared pages; harness memory separate'},cases:[]};
for(const size of sizes){const result=await runSize(size);summary.cases.push(result);await writeFile(join(runDir,'summary.json'),JSON.stringify(summary,null,2)+'\n');if(result.cleanupIncomplete||result.error?.includes('resource_guard'))break;}
summary.finishedAt=new Date().toISOString();await writeFile(join(runDir,'summary.json'),JSON.stringify(summary,null,2)+'\n');
console.log(JSON.stringify({report:join(runDir,'summary.json'),results:summary.cases.map(c=>({nodes:c.nodes,passed:c.passed,error:c.error,steady:c.steady,burst:c.burst,memory:c.memory,ownerOffline:c.ownerOffline,ownerRecovery:c.ownerRecovery}))}));
if(summary.cases.length!==sizes.length||summary.cases.some(c=>!c.passed||c.error||c.cleanupIncomplete))process.exitCode=1;
