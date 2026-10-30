#!/usr/bin/env node
/** Local Codex configuration preflight only. Deliberately never calls turn/start. */
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { mkdir,writeFile } from 'node:fs/promises';
import { resolve,join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { locateCodex,notificationOverrides } from '../adapters/codex-app-server.mjs';

const directory=resolve('.local/notification-preflight',randomUUID());await mkdir(directory,{recursive:true});
const report={startedAt:new Date().toISOString(),passed:false,scope:'Actual installed Codex app-server initialize/config/thread/MCP preflight only; NO turn/start, model generation, channel publication or message sending.',checks:[]};
const target=await locateCodex();
const features=['shell_tool','apps','plugins','memories','request_permissions_tool','multi_agent'];
const proc=spawn(target.command,[...target.args,...features.flatMap(f=>['-c',`features.${f}=false`]),'app-server','--stdio'],{cwd:directory,windowsHide:true,shell:false,stdio:['pipe','pipe','pipe']});
const pending=new Map();let next=0,received=0;
const lines=createInterface({input:proc.stdout});
proc.stderr.on('data',()=>{});
proc.stdin.on('error',()=>{});
const fail=()=>{for(const p of pending.values())p.reject(new Error('runtime_preflight_exited'));pending.clear();};
proc.on('error',fail);proc.on('exit',fail);
lines.on('line',line=>{
  received+=Buffer.byteLength(line);if(received>4*1024*1024){fail();return;}
  let p;try{p=JSON.parse(line);}catch{fail();return;}
  if(p.id!=null&&!p.method){const waiter=pending.get(p.id);if(!waiter)return;pending.delete(p.id);if(p.error){report.rejectionHint=String(p.error.message||'').replace(/[A-Za-z0-9_-]{32,}/g,'[long-value]').slice(0,400);waiter.reject(new Error(`runtime_rpc_error_${p.error.code}`));}else waiter.resolve(p.result);}
  // No server-originated approval/tool request is ever answered with permission.
  if(p.id!=null&&p.method){proc.stdin.write(`${JSON.stringify({id:p.id,error:{code:-32601,message:'Preflight does not grant approvals or execute tools'}})}\n`);}
});
const send=packet=>proc.stdin.write(`${JSON.stringify(packet)}\n`);
const rpc=(method,params)=>new Promise((resolve,reject)=>{assert.notEqual(method,'turn/start');const id=++next;pending.set(id,{resolve,reject});send({id,method,params});});
const timeout=setTimeout(()=>{fail();proc.kill();},40000);
try{
  await rpc('initialize',{clientInfo:{name:'aiconnect_notification_preflight',version:'0.1.0'}});send({method:'initialized',params:{}});
  report.checks.push('Installed runtime accepts stdio initialization without a model turn');
  const cfg=await rpc('config/read',{cwd:directory,includeLayers:true});const overrides=notificationOverrides(cfg);
  report.disabledMcpOverrides=Object.keys(overrides.mcp_servers||{}).length;
  const start=await rpc('thread/start',{cwd:directory,ephemeral:true,sandbox:'read-only',approvalPolicy:'never',config:overrides,developerInstructions:'Local configuration preflight only. No model turn or task is authorized.'});
  assert.ok(start.thread?.id);report.checks.push('Actual runtime accepts ephemeral read-only thread and disabled notification tool configuration');
  const inventory=await rpc('mcpServerStatus/list',{threadId:start.thread.id,limit:100,detail:'toolsAndAuthOnly'});
  assert.ok(Array.isArray(inventory.data));assert.ok(!inventory.nextCursor);
  assert.ok(inventory.data.every(s=>s.tools&&typeof s.tools==='object'&&Object.keys(s.tools).length===0&&!s.toolsError));
  report.checks.push('No MCP tools are exposed before any generation');
  report.modelTurns=0;report.passed=true;
}catch(error){report.error=/^[a-zA-Z0-9_ .:-]{1,140}$/.test(error.message||'')?error.message:'runtime_preflight_rejected';process.exitCode=1;}
finally{
  clearTimeout(timeout);proc.stdin.end();
  if(proc.exitCode===null)await Promise.race([new Promise(resolve=>proc.once('exit',resolve)),new Promise(resolve=>setTimeout(resolve,1500))]);
  if(proc.exitCode===null&&proc.pid){
    if(process.platform==='win32')await new Promise(resolve=>{const kill=spawn('taskkill.exe',['/PID',String(proc.pid),'/T','/F'],{windowsHide:true,stdio:'ignore'});kill.once('exit',resolve);kill.once('error',resolve);});
    proc.kill();
  }
  lines.close();report.finishedAt=new Date().toISOString();const reportPath=join(directory,'report.json');await writeFile(reportPath,JSON.stringify(report,null,2));process.stdout.write(`${JSON.stringify({...report,reportPath})}\n`);
}
