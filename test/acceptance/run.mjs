import assert from 'node:assert/strict';
import {execFile,spawn} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdir,readFile,writeFile,access} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {connect} from 'node:net';

const exec=promisify(execFile);
const argv=process.argv.slice(2);
const flag=(name,fallback)=>{const n=argv.indexOf(name);return n<0?fallback:argv[n+1]};
const binary=resolve(flag('--binary',`target/debug/aiconnect${process.platform==='win32'?'.exe':''}`));
const root=resolve('.local/acceptance',new Date().toISOString().replace(/[:.]/g,'-'));
await mkdir(root,{recursive:true});
const results=[];const running=[];
const profile=name=>join(root,name);
function native(args){return new Promise((resolve,reject)=>{
  const child=spawn(binary,args,{windowsHide:true,stdio:['ignore','pipe','pipe']});let stdout='',stderr='';let settled=false;
  child.stdout.on('data',b=>stdout+=b);child.stderr.on('data',b=>stderr+=b);
  const timeout=setTimeout(()=>{child.kill();finish(Error('native_command_timeout'))},45000);
  function finish(error){if(settled)return;settled=true;clearTimeout(timeout);child.stdout.destroy();child.stderr.destroy();if(error){error.stderr=stderr;reject(error)}else resolve({stdout,stderr})}
  child.once('error',finish);child.once('exit',code=>setTimeout(()=>finish(code===0?null:Object.assign(Error('native_command_failed'),{code})),50));
});}
async function cli(name,args,{failure=false}={}){
  try{const {stdout}=await native(['--data',profile(name),'--json',...args]);
    if(failure)throw Error('expected_command_rejection');
    return JSON.parse(stdout.trim());
  }catch(e){
    let detail;try{detail=JSON.parse(e.stderr?.trim()||'{}').error}catch{}
    // execFile's default error message includes the complete command, including invitation arguments.
    const safeError=detail||`command_failed:${args[0]}:${e.code??'unknown'}`;
    if(!failure||e.message==='expected_command_rejection')throw Error(e.message==='expected_command_rejection'?e.message:safeError);
    return {rejected:true,error:safeError};
  }
}
async function check(name,fn){const start=Date.now();try{await fn();results.push({name,status:'passed',durationMs:Date.now()-start});console.log(`PASS ${name}`);}catch(e){results.push({name,status:'failed',error:e.message,durationMs:Date.now()-start});console.error(`FAIL ${name}: ${e.message}`);}}
async function wire(meta,payload){return new Promise((resolve,reject)=>{const [host,port]=meta.address.split(':');const socket=connect({host,port:Number(port)});let text='';socket.setTimeout(25000);socket.on('connect',()=>socket.end(payload+'\n'));socket.on('data',chunk=>text+=chunk);socket.on('end',()=>{try{resolve(text.trim()?JSON.parse(text.trim()):null)}catch(e){reject(e)}});socket.on('error',reject);socket.on('timeout',()=>socket.destroy(Error('ipc_timeout')));});}
const cardBody=card=>JSON.parse(Buffer.from(card.body,'base64url').toString('utf8'));
let a,b,aCard,bCard,session;
try{
  await access(binary);
  await check('three independent native daemons and persisted identities',async()=>{
    for(const name of ['a','b','stranger']){running.push(name);await cli(name,['setup','--name',`acceptance-${name}`,'--direct-only']);}
    aCard=await cli('a',['identity']);bCard=await cli('b',['identity']);a=cardBody(aCard);b=cardBody(bCard);assert.notEqual(a.id,b.id);
    const ids=await Promise.all(['a','b','stranger'].map(n=>cli(n,['status']).then(v=>v.id)));assert.equal(new Set(ids).size,3);
  });
  if(results.at(-1).status!=='passed')throw Error('native_daemon_prerequisite_failed');
  await check('local IPC rejects absent/wrong credentials and malformed requests',async()=>{
    const meta=JSON.parse(await readFile(join(profile('a'),'daemon.json'),'utf8'));
    for(const payload of [JSON.stringify({request:{action:'identity',name:'intrusion'}}),JSON.stringify({token:'wrong',request:{action:'identity',name:'intrusion'}}),'not json']){
      const response=await wire(meta,payload);assert(!response||response.ok===false,'unauthorized or malformed IPC accepted');
    }
    const identity=cardBody(await cli('a',['identity']));assert.equal(identity.id,a.id);assert.notEqual(identity.name,'intrusion');
    assert.equal((await cli('a',['status'])).id,a.id,'daemon must survive rejected ingress');
  });
  await check('self-signed contacts round trip and tampered cards rejected',async()=>{
    await writeFile(join(root,'a-card.json'),JSON.stringify(aCard));await writeFile(join(root,'b-card.json'),JSON.stringify(bCard));
    await cli('a',['contacts','import',join(root,'b-card.json'),'--name','Other owner B','--group','test']);
    await cli('b',['contacts','import',join(root,'a-card.json')]);
    assert.equal((await cli('a',['contacts','list'])).contacts.length,1);
    await cli('a',['contacts','group',b.id,'verified-group']);
    assert.equal((await cli('a',['contacts','list'])).contacts[0].group,'verified-group');
    const exported=await cli('a',['contacts','export',b.id]);assert.deepEqual(exported,bCard);
    const tampered=structuredClone(bCard);tampered.body=Buffer.from(JSON.stringify({...b,name:'tampered'})).toString('base64url');await writeFile(join(root,'bad-card.json'),JSON.stringify(tampered));
    await cli('stranger',['contacts','import',join(root,'bad-card.json')],{failure:true});
    assert.equal((await cli('stranger',['contacts','list'])).contacts.length,0);
  });
  await check('temporary invite admits only one distinct peer',async()=>{
    const created=await cli('a',['temp','create','--ttl','60s']);session=created.session.id;
    const joined=await cli('b',['temp','join',created.invite]);assert.equal(joined.session.id,session);
    await cli('stranger',['temp','join',created.invite],{failure:true});
    const sessions=await cli('a',['sessions']);assert(!JSON.stringify(sessions).includes(created.invite));assert(!JSON.stringify(sessions).includes('capability'));
  });
  await check('core rejects invalid TTL and oversized text beyond CLI parsing',async()=>{
    for(const ttlSeconds of [0,86401]){
      const rejected=await cli('a',['request',JSON.stringify({action:'temp.create',ttlSeconds})],{failure:true});assert.match(rejected.error,/invalid_ttl/);
    }
    const rejected=await cli('a',['request',JSON.stringify({action:'send',sessionId:session,text:'x'.repeat(16001)})],{failure:true});assert.match(rejected.error,/invalid_message/);
    assert.equal((await cli('b',['messages',session])).messages.length,0,'invalid requests cannot create receiver records');
  });
  await check('durable delivery acknowledgement and stable-ID duplicate suppression',async()=>{
    const messageId='acceptance-stable-id';const sent=await cli('a',['send',session,'--text','Unique acceptance message','--id',messageId]);assert.equal(sent.status,'acked');
    const path=(await cli('a',['status'])).lastPath;assert.equal(path.direct,true,'direct-only native delivery must report an observed selected direct path');assert.equal(path.relay,false);
    await cli('a',['send',session,'--text','Unique acceptance message','--id',messageId]);
    const messages=(await cli('b',['messages',session])).messages;assert.equal(messages.filter(m=>m.id===messageId).length,1);
    await cli('a',['send',session,'--text','Conflicting duplicate','--id',messageId],{failure:true});
    await cli('b',['send',session,'--text','Reply via native transport']);
    assert.equal((await cli('a',['messages',session])).messages.length,2);
  });
  await check('temporary sessions never grant task execution',async()=>{
    await cli('a',['send',session,'--text','execute test task','--kind','task'],{failure:true});
    const inbox=(await cli('b',['inbox'])).messages;
    for(const message of inbox.filter(m=>m.sessionId===session)){assert.equal(message.taskAllowed,false);assert.equal(message.wakeAllowed,false);}
  });
  await check('concurrent invitation claims bind exactly one distinct endpoint',async()=>{
    for(let iteration=0;iteration<4;iteration++){
      const created=await cli('a',['temp','create','--ttl','60s']);
      const attempts=await Promise.allSettled(['b','stranger'].map(name=>cli(name,['temp','join',created.invite])));
      assert.equal(attempts.filter(v=>v.status==='fulfilled').length,1,'simultaneous different peers must not both join');
      await cli('a',['session','close',created.session.id]);
    }
  });
  await check('closed sessions reject further delivery while retaining history',async()=>{
    await cli('a',['session','close',session]);
    await cli('b',['send',session,'--text','must not arrive'],{failure:true});
    assert.equal((await cli('b',['messages',session])).messages.length,2);
  });
  await check('expiry is enforced for a previously issued invitation',async()=>{
    const created=await cli('a',['temp','create','--ttl','1s']);await new Promise(r=>setTimeout(r,1250));
    await cli('stranger',['temp','join',created.invite],{failure:true});
  });
  await check('restart preserves identity contacts and message history',async()=>{
    await cli('a',['daemon','stop']);await cli('a',['daemon','start','--direct-only']);
    assert.equal(cardBody(await cli('a',['identity'])).id,a.id);assert.equal((await cli('a',['contacts','list'])).contacts.length,1);
    assert.equal((await cli('a',['messages',session])).messages.length,2);
  });
  await check('offline outbox survives sender restart and retries after receiver recovery',async()=>{
    const connection=await cli('a',['connect',b.id]);const sid=connection.session.id;
    await cli('b',['daemon','stop']);
    const queued=await cli('a',['send',sid,'--id','offline-restart-message','--text','Persist me while receiver is offline']);assert.equal(queued.status,'pending');
    await cli('a',['daemon','stop']);await cli('a',['daemon','start','--direct-only']);
    await cli('b',['daemon','start','--direct-only']);
    let delivered=false;for(let i=0;i<40;i++){const messages=(await cli('b',['messages',sid])).messages;if(messages.some(m=>m.id==='offline-restart-message')){delivered=true;break}await new Promise(r=>setTimeout(r,500));}
    assert(delivered,'durable sender outbox did not retry after both daemon restarts');
    const sender=(await cli('a',['messages',sid])).messages.find(m=>m.id==='offline-restart-message');assert.equal(sender.status,'acked');
  });
  await check('blocking survives restart and prevents contact reconnection',async()=>{
    await cli('b',['contacts','block',a.id]);await cli('b',['daemon','stop']);await cli('b',['daemon','start','--direct-only']);
    // Refresh the restarted endpoint ticket so a stale UDP port cannot masquerade as a successful policy rejection.
    await writeFile(join(root,'b-card-refreshed.json'),JSON.stringify(await cli('b',['identity'])));
    await cli('a',['contacts','import',join(root,'b-card-refreshed.json')]);
    const denied=await cli('a',['connect',b.id],{failure:true});assert.match(denied.error,/contact_blocked/);
    await cli('b',['contacts','unblock',a.id]);
    const connection=await cli('a',['connect',b.id]);assert(connection.session.id);
    await cli('a',['send',connection.session.id,'--text','contact message']);
  });
  await check('contact task and wake policy remain distinct and independently enforced',async()=>{
    const connection=await cli('a',['connect',b.id]);const sid=connection.session.id;
    const denied=await cli('a',['send',sid,'--kind','task','--id','policy-denied-task','--text','A task without task authorization'],{failure:true});
    assert.match(denied.error,/permission_denied/);
    assert.equal((await cli('a',['messages',sid])).messages.find(m=>m.id==='policy-denied-task').status,'rejected');
    assert(!(await cli('b',['inbox'])).messages.some(m=>m.id==='policy-denied-task'),'denied task must not enter receiver inbox');
    await cli('b',['contacts','policy',a.id,'--task','true','--wake','false']);
    await cli('a',['retry']);assert(!(await cli('b',['inbox'])).messages.some(m=>m.id==='policy-denied-task'),'a terminally denied task must not revive after a policy grant');
    const allowed=await cli('a',['send',sid,'--kind','task','--id','policy-allowed-task','--text','Authorized task remains dormant without wake permission']);
    assert.equal(allowed.status,'acked');
    const message=(await cli('b',['inbox'])).messages.find(m=>m.id==='policy-allowed-task');assert(message);assert.equal(message.taskAllowed,true);assert.equal(message.wakeAllowed,false);
    await cli('b',['contacts','policy',a.id,'--task','false']);
    const revoked=(await cli('b',['inbox'])).messages.find(m=>m.id==='policy-allowed-task');assert.equal(revoked.taskAllowed,false);
  });
  await check('contact removal ends its existing sessions',async()=>{
    const connection=await cli('a',['connect',b.id]);await cli('b',['contacts','remove',a.id]);
    assert.equal((await cli('b',['contacts','list'])).contacts.length,0);
    const rejected=await cli('a',['send',connection.session.id,'--text','Must not revive a removed contact session'],{failure:true});assert.match(rejected.error,/session_ended/);
    await cli('a',['connect',b.id],{failure:true});
  });
}catch(e){results.push({name:'acceptance harness prerequisites',status:'failed',error:e.message});}
finally{for(const name of running){try{await cli(name,['daemon','stop'])}catch{}}}
if(argv.includes('--full'))await check('real dual-Agent local collaboration',async()=>{await exec(process.execPath,['lab/run.mjs','--agents','--binary',binary],{timeout:600000,maxBuffer:2**20,windowsHide:true});});
const report={version:1,createdAt:new Date().toISOString(),binary,scope:'native-local-cli-blackbox',limitations:['Same-host independent processes are logical isolation, not OS security isolation.','This gate does not prove WAN NAT traversal, forced relay, platform matrix, or release readiness.'],results,passed:results.length>0&&results.every(v=>v.status==='passed')};
await writeFile(join(root,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({report:join(root,'report.json'),passed:report.passed}));if(!report.passed)process.exitCode=1;
