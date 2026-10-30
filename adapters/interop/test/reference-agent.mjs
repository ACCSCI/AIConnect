/** Deterministic official-SDK HTTP fixture. Never invokes a model or shell. */
import express from 'express';
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { AgentEvent,DefaultRequestHandler,InMemoryTaskStore } from '@a2a-js/sdk/server';
import { agentCardHandler,jsonRpcHandler,UserBuilder } from '@a2a-js/sdk/server/express';
import { Role,TaskState } from '@a2a-js/sdk';
const text=value=>({content:{$case:'text',value},mediaType:'text/plain',filename:''});
export async function startReferenceAgent({logicalBase,bearer='aiconnect-sdk-fixture-only-bearer-1234567890'}={}){
  const app=express(),server=createServer(app),running=new Map();let postCount=0;
  app.use(express.json({limit:'32kb'}));
  app.use((req,res,next)=>req.headers.authorization===`Bearer ${bearer}`?next():res.status(401).end());
  app.post('/rpc',(req,res,next)=>{if(req.body?.method==='SendMessage')postCount++;next();});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const wireBase=`http://127.0.0.1:${server.address().port}`;
  const provider=logicalBase||wireBase;
  function finish(id,value='deterministic reference result'){
    const r=running.get(id);if(!r)throw new Error('reference_task_missing');
    r.bus.publish(AgentEvent.artifactUpdate({taskId:id,contextId:r.contextId,artifact:{artifactId:'output',name:'output.json',parts:[text(value)]},append:false,lastChunk:true}));
    r.bus.publish(AgentEvent.statusUpdate({taskId:id,contextId:r.contextId,status:{state:TaskState.TASK_STATE_COMPLETED,message:{messageId:randomUUID(),role:Role.ROLE_AGENT,parts:[text(value)]}},final:true}));r.bus.finished();r.resolve?.();
  }
  const executor={
    async execute(context,bus){
      const id=context.taskId;running.set(id,{bus,contextId:context.contextId});
      bus.publish(AgentEvent.task({id,contextId:context.contextId,status:{state:TaskState.TASK_STATE_SUBMITTED},artifacts:[],history:[],metadata:{fixture:true}}));
      bus.publish(AgentEvent.statusUpdate({taskId:id,contextId:context.contextId,status:{state:TaskState.TASK_STATE_WORKING},final:false}));
      const input=context.userMessage.parts.find(p=>p.content?.$case==='data')?.content.value?.input;
      if(input?.hold){await new Promise(resolve=>{running.get(id).resolve=resolve;});return;}
      await new Promise(resolve=>setTimeout(resolve,20));finish(id,input?.largeOutput?'中文'.repeat(12000):undefined);
    },
    async cancelTask(id,bus){bus.publish(AgentEvent.statusUpdate({taskId:id,contextId:running.get(id)?.contextId||'',status:{state:TaskState.TASK_STATE_CANCELED},final:true}));bus.finished();running.get(id)?.resolve?.();},
  };
  const card={name:'AIConnect deterministic SDK fixture',description:'Local test only, no model',version:'1',supportedInterfaces:[{url:`${provider}/rpc`,protocolBinding:'JSONRPC',protocolVersion:'1.0',tenant:''}],capabilities:{streaming:true,pushNotifications:false,extensions:[]},securitySchemes:{},securityRequirements:[],defaultInputModes:['application/json','text/plain'],defaultOutputModes:['text/plain'],skills:[],signatures:[]};
  const handler=new DefaultRequestHandler(card,new InMemoryTaskStore(),executor);
  app.use('/.well-known/agent-card.json',agentCardHandler({agentCardProvider:handler}));
  app.use('/rpc',jsonRpcHandler({requestHandler:handler,userBuilder:UserBuilder.noAuthentication}));
  return {server,wireBase,provider,bearer,finish,get postCount(){return postCount;},close:()=>new Promise(resolve=>{for(const r of running.values()){r.bus.finished();r.resolve?.();}server.closeAllConnections?.();server.close(resolve);})};
}
