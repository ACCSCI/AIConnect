#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { resolve,join } from 'node:path';
import { homedir } from 'node:os';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const args=process.argv.slice(2);const option=(flag,def)=>{const i=args.indexOf(flag);return i<0?def:args[i+1];};
const directory=resolve(option('--data',process.env.AICONNECT_HOME||join(homedir(),'.aiconnect')));
const command=args[0]||'help';
const emit=value=>console.log(JSON.stringify(value,null,args.includes('--json')?0:2));
async function call(path,method='GET',body){const info=JSON.parse(await readFile(join(directory,'daemon.json'),'utf8'));const res=await fetch(`http://127.0.0.1:${info.port}${path}`,{method,headers:{Authorization:`Bearer ${info.token}`,'Content-Type':'application/json'},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(20000)});const data=await res.json();if(!res.ok)throw new Error(data.error);return data;}
try{
  if(command==='daemon'){
    const {startDaemon}=await import('../runtime/daemon.mjs');
    const daemon=await startDaemon({directory,port:Number(option('--port','18871')),directOnly:args.includes('--direct-only'),relayUrls:option('--relay')?[option('--relay')]:undefined});
    emit({running:true,port:daemon.info.port,id:daemon.info.id,localOnly:true});
    for(const sig of ['SIGINT','SIGTERM'])process.once(sig,async()=>{await daemon.close();process.exit(0);});
  }else if(command==='start'){
    try{emit(await call('/api/status'));process.exit(0);}catch{}
    const child=spawn(process.execPath,[fileURLToPath(import.meta.url),'daemon','--data',directory,'--port',option('--port','18871'),...(args.includes('--direct-only')?['--direct-only']:[]),...(option('--relay')?['--relay',option('--relay')]:[])],{detached:true,windowsHide:true,stdio:'ignore'});child.unref();
    let started=false;for(let i=0;i<60;i++){await new Promise(r=>setTimeout(r,200));try{emit(await call('/api/status'));started=true;break;}catch{}}if(!started)throw new Error('daemon_start_failed: run daemon in foreground for details');
  }else if(command==='status')emit(await call('/api/status'));
  else if(command==='sessions')emit(await call('/api/sessions'));
  else if(command==='temp')emit(await call('/api/sessions','POST',{label:option('--name','临时协作'),ttlSeconds:Number(option('--ttl','1800'))}));
  else if(command==='join'){if(!args[1])throw new Error('usage: join <invite>');emit(await call('/api/join','POST',{invite:args[1]}));}
  else if(command==='send'){if(!args[1]||!args[2])throw new Error('usage: send <session-id> <text>');emit(await call(`/api/sessions/${encodeURIComponent(args[1])}/messages`,'POST',{text:args[2]}));}
  else if(command==='messages'){if(!args[1])throw new Error('usage: messages <session-id>');emit(await call(`/api/sessions/${encodeURIComponent(args[1])}/messages`));}
  else if(command==='end'){if(!args[1])throw new Error('usage: end <session-id>');emit(await call(`/api/sessions/${encodeURIComponent(args[1])}/end`,'POST',{}));}
  else if(command==='ui'){const info=JSON.parse(await readFile(join(directory,'daemon.json'),'utf8'));console.log(`http://127.0.0.1:${info.port}/#token=${info.token}`);}
  else console.log(`AIConnect 0.2.0 — local-first temporary P2P connections\n\nstart [--port 18871] [--data DIRECTORY]\ndaemon [--direct-only] [--relay HTTPS_URL]\nstatus | sessions\ntemp [--name NAME] [--ttl SECONDS]\njoin INVITE\nsend SESSION_ID "message"\nmessages SESSION_ID\nend SESSION_ID\nui\n\nAll commands support --data DIRECTORY and --json.\nTemporary invites expire, admit one peer, create no contacts, and grant no execution rights.\nThis CLI is local source, not a published npm package.`);
}catch(error){console.error(JSON.stringify({error:error.message}));process.exitCode=1;}
