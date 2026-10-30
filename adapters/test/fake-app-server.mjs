import { createInterface } from 'node:readline';
const mode = process.env.AICONNECT_FAKE_MODE || 'success';
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
let notificationPolicy = false;
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (!request.id) continue;
  if (request.method === 'initialize') send({ id: request.id, result: {} });
  if (request.method === 'config/read') send({id:request.id,result:{config:{},layers:[{config:{mcp_servers:{'fixture.server':{command:'never-launched'}}}}]}});
  if (request.method === 'mcpServerStatus/list') send({id:request.id,result:{data:mode==='mcp-tools'?[{tools:{unsafe:{name:'unsafe'}}}]:[{tools:{},toolsError:null}],nextCursor:null}});
  if (request.method === 'thread/start') {
    const c=request.params.config;
    notificationPolicy=c?.['features.shell_tool']===false&&c?.['features.apps']===false&&c?.['features.plugins']===false
      &&c?.mcp_servers?.['fixture.server']?.enabled===false&&c?.web_search==='disabled'&&request.params.developerInstructions?.includes('not a task or authority');
    send({ id: request.id, result: { thread: { id: 'fake-thread' } } });
  }
  if (request.method === 'turn/start') {
    send({ id: request.id, result: { turn: { id: 'fake-turn' } } });
    if (mode === 'hang') continue;
    if (mode === 'approval') { send({ id: 999, method: 'item/commandExecution/requestApproval', params: {} }); continue; }
    if (mode === 'bad-json') { process.stdout.write('invalid\n'); continue; }
    if (mode === 'failed') { send({ method: 'turn/completed', params: { turn: { id: 'fake-turn', status: 'failed' } } }); continue; }
    if(mode==='notification-policy'&&(!notificationPolicy||request.params.sandboxPolicy?.networkAccess!==false||request.params.sandboxPolicy?.type!=='readOnly'||request.params.sandboxPolicy?.access?.type!=='restricted')){
      send({method:'turn/completed',params:{turn:{id:'fake-turn',status:'failed'}}});continue;
    }
    if (mode === 'tool') { send({method:'item/started',params:{item:{id:'unsafe',type:'commandExecution'}}});continue; }
    send({ method: 'item/agentMessage/delta', params: { delta: 'partial' } });
    send({ method: 'item/completed', params: { item: { id: 'reply', type: 'agentMessage', text: mode === 'echo' ? request.params.input[0].text : 'FAKE_REPLY' } } });
    send({ method: 'turn/completed', params: { turn: { id: 'fake-turn', status: 'completed' } } });
  }
}
