import { createHash, randomUUID, timingSafeEqual } from 'node:crypto';
import express from 'express';
import { AgentEvent, DefaultRequestHandler } from '@a2a-js/sdk/server';
import { agentCardHandler, jsonRpcHandler } from '@a2a-js/sdk/server/express';
import { ClientFactory, DefaultAgentCardResolver, JsonRpcTransportFactory } from '@a2a-js/sdk/client';
import { Role, TaskState } from '@a2a-js/sdk';
import { safeFetchFor } from './safe-http.mjs';

const stateName = new Map([
  [TaskState.TASK_STATE_SUBMITTED, 'submitted'], [TaskState.TASK_STATE_WORKING, 'working'],
  [TaskState.TASK_STATE_COMPLETED, 'completed'], [TaskState.TASK_STATE_FAILED, 'failed'],
  [TaskState.TASK_STATE_CANCELED, 'canceled'], [TaskState.TASK_STATE_REJECTED, 'failed'],
  [TaskState.TASK_STATE_INPUT_REQUIRED, 'input-required'], [TaskState.TASK_STATE_AUTH_REQUIRED, 'input-required'],
]);
const textPart = value => ({ content: { $case: 'text', value }, mediaType: 'text/plain', filename: '' });
const messageText = message => (message?.parts || []).filter(p => p.content?.$case === 'text').map(p => p.content.value).join('\n');
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const terminal = state => ['completed','failed','canceled'].includes(state);
const errorCode = error => /^[a-zA-Z0-9_ .:-]{1,140}$/.test(error.message || '') ? error.message : 'bridge_remote_error';
const utf8Text = (value,max=8000) => {
  const bytes=Buffer.from(value || '', 'utf8');
  if (bytes.length<=max) return bytes.toString('utf8');
  let end=max;
  while(end>0&&(bytes[end]&0xc0)===0x80) end--;
  return bytes.subarray(0,end).toString('utf8');
};
function boundedSnapshot(task) {
  const raw=JSON.stringify(task);
  if(Buffer.byteLength(raw)<=60000)return task;
  return {id:task.id,contextId:task.contextId,status:{state:task.status?.state,timestamp:task.status?.timestamp,
    message:{messageId:task.status?.message?.messageId || randomUUID(),role:Role.ROLE_AGENT,parts:[textPart(utf8Text(messageText(task.status?.message)))],extensions:[],referenceTaskIds:[]}},
    artifacts:[],history:[],metadata:{truncated:true,originalByteLength:Buffer.byteLength(raw),observationSha256:createHash('sha256').update(raw).digest('hex')}};
}

export async function makeA2AClient(core, provider, config = {}) {
  const fetchImpl = safeFetchFor(provider, {
    token: config.token, allowLoopback: config.allowLoopback === true,
    beforeRequest: async url => { await core({ action: 'provider.authorize', url: url.href }); },
    chargeDownload: async bytes => { await core({action:'governance.download.charge',bytes}); },
  });
  const guardedFetch = async (url, init) => {
    const res = await fetchImpl(url, init);
    if ([402,429].includes(res.status)) {
      await core({ action: 'provider.check', url: String(url), statusCode: res.status });
      throw new Error(`provider_disabled_http_${res.status}`);
    }
    return res;
  };
  const factory = new ClientFactory({
    transports: [new JsonRpcTransportFactory({ fetchImpl: guardedFetch })],
    cardResolver: new DefaultAgentCardResolver({ fetchImpl: guardedFetch }),
    clientConfig: { polling: true, acceptedOutputModes: ['text/plain','application/json'] },
  });
  const client = await factory.createFromUrl(provider);
  // A directory cannot redirect a local bearer token to a second service origin.
  const card = await client.getAgentCard();
  const selected = card.supportedInterfaces?.find(i => i.protocolBinding === 'JSONRPC' && i.protocolVersion === '1.0');
  if (!selected || new URL(selected.url).origin !== new URL(provider).origin) throw new Error('provider_origin_mismatch');
  return { client, card };
}

function artifactManifest(task) {
  return (task.artifacts || []).slice(0,32).map(artifact => {
    // Hash only bounded data actually observed. Do not download arbitrary artifact URLs.
    const observed = JSON.stringify(artifact);
    return { name: utf8Text(String(artifact.name || artifact.artifactId || 'artifact'),256),
      sha256: createHash('sha256').update(observed).digest('hex'), size: Buffer.byteLength(observed) };
  });
}

async function persistTask(core, jobId, task) {
  if (!task?.id || !stateName.has(task.status?.state)) throw new Error('unsupported_a2a_response');
  const text=utf8Text(messageText(task.status.message));
  return core({ action: 'service.update', jobId, externalTaskId: task.id, externalTask: boundedSnapshot(task),
    providerStatus: stateName.get(task.status.state), ...(text.trim() ? {text} : {}),
    artifacts: artifactManifest(task),
  });
}

/** One bounded scan. No model calls. Never retry an ambiguous initial POST. */
export async function processA2AJobs(core, config = {}, dependencies = {}) {
  const pending = await core({ action: 'service.pending', transport: 'a2a', offset: config.offset || 0 });
  const results = [];
  for (const job of (pending.jobs || []).slice(0,32)) {
    const id = job.jobId;
    try {
      const origin = new URL(job.provider).origin;
      const providerConfig = config.providers?.[origin] || {};
      // Credentials are read only from this local configuration/environment, never a peer's input.
      const token = providerConfig.tokenEnv ? process.env[providerConfig.tokenEnv] : undefined;
      const { client } = await (dependencies.makeClient || makeA2AClient)(core, job.provider, { ...providerConfig, token });
      let task;
      if (job.externalTaskId) {
        task = job.cancelRequested
          ? await client.cancelTask({ id: job.externalTaskId }, { signal: AbortSignal.timeout(15000) })
          : await client.getTask({ id: job.externalTaskId, historyLength: 8 }, { signal: AbortSignal.timeout(15000) });
      } else if (job.state === 'queued') {
        // Core atomically claims the job and rechecks its delegation/deadline/budget.
        await core({ action: 'service.dispatch', jobId: id });
        task = await client.sendMessage({ message: {
          messageId: id, role: Role.ROLE_USER,
          parts: [{ content: { $case: 'data', value: { capability: job.capability, input: job.input } }, mediaType: 'application/json', filename: '' }],
          contextId: '', taskId: '', extensions: [], referenceTaskIds: [],
        }, configuration: { returnImmediately: true, historyLength: 8, acceptedOutputModes: ['text/plain','application/json'] },
        metadata: { idempotencyKey: job.idempotencyKey, aiconnectJobId: id } }, { signal: AbortSignal.timeout(15000) });
        if (!task.id && task.messageId) {
          const observed = JSON.stringify(task);
          const text=utf8Text(messageText(task));
          const externalTask=Buffer.byteLength(observed)<=60000?task:{messageId:task.messageId,role:task.role,parts:[textPart(text)],metadata:{truncated:true,originalByteLength:Buffer.byteLength(observed)}};
          await core({ action: 'service.update', jobId: id, providerStatus: 'completed', externalTask,
            ...(text.trim()?{text}:{}),artifacts: [{ name: 'a2a-message.json', sha256: createHash('sha256').update(observed).digest('hex'), size: Buffer.byteLength(observed) }] });
          results.push({ jobId: id, status: 'delivered', requiresLocalReview: true });
          continue;
        }
      } else {
        // Dispatching with no provider task ID is ambiguous after a crash/timeout.
        // A provider may already have performed the operation: no automatic second send.
        results.push({ jobId: id, status: 'reconciliation_required', error: 'dispatch_without_external_task_id' });
        continue;
      }
      await persistTask(core, id, task);
      const state = stateName.get(task.status.state);
      results.push({ jobId: id, status: state, externalTaskId: task.id, requiresLocalReview: state === 'completed' });
    } catch (error) {
      results.push({ jobId: id, status: 'deferred', error: errorCode(error) });
    }
  }
  return { results, hasMore: pending.hasMore === true, nextOffset: pending.hasMore ? pending.nextOffset : 0 };
}

/** The SDK's wire snapshots live in the same Core SQLite Job, never another database. */
export class CoreTaskStore {
  constructor(core, scope) { this.core = core; this.scope = scope; }
  async load(id) {
    try {
      const r = await this.core({ action: 'service.lookup', externalTaskId: id });
      if (!r.job?.externalTask || (this.scope && r.job.externalTask.metadata?.aiconnectBridgeScope !== this.scope)) return undefined;
      if (r.job.transport !== 'native') return r.job.externalTask;
      const { job } = await this.core({ action: 'service.status', jobId: r.job.jobId });
      const latest = jobTask(job,id,r.job.externalTask.contextId);
      return { ...r.job.externalTask, ...latest, metadata: { ...r.job.externalTask.metadata, ...latest.metadata } };
    }
    catch (error) { if (/not_found/.test(error.message)) return undefined; throw error; }
  }
  async save(task) {
    const { job } = await this.core({ action: 'service.lookup', externalTaskId: task.id });
    if (!job) throw new Error('bridge_task_not_found');
    if (this.scope && task.metadata?.aiconnectBridgeScope !== this.scope) throw new Error('bridge_task_scope_mismatch');
    await this.core({ action: 'service.snapshot', jobId: job.jobId, externalTask: task });
  }
  async list(params = {}) {
    const offset = params.pageToken ? Number(params.pageToken) : 0;
    if (!Number.isSafeInteger(offset) || offset < 0) throw new Error('invalid_page_token');
    const { jobs = [], hasMore, nextOffset } = await this.core({ action: 'service.list', transport: 'native', offset, limit: Math.min(params.pageSize || 32,100) });
    const tasks=[];
    for(const job of jobs.filter(j => j.externalTask && (!this.scope || j.externalTask.metadata?.aiconnectBridgeScope === this.scope))){
      const latest=await this.load(job.externalTaskId);
      if(latest && (!params.contextId || latest.contextId===params.contextId) && (!params.status || latest.status?.state===params.status))tasks.push(latest);
    }
    return { tasks, nextPageToken: hasMore ? String(nextOffset) : '', pageSize: tasks.length, totalSize: tasks.length };
  }
}

function jobTask(job, taskId, contextId) {
  const state = job.state === 'delivered' || job.localReviewStatus === 'awaiting_review' || job.state === 'approved' || job.state === 'completed'
    ? TaskState.TASK_STATE_COMPLETED : job.state === 'failed' ? TaskState.TASK_STATE_FAILED
    : job.state === 'canceled' ? TaskState.TASK_STATE_CANCELED : job.state === 'expired' ? TaskState.TASK_STATE_FAILED : TaskState.TASK_STATE_WORKING;
  return { id: taskId, contextId, status: { state, timestamp: new Date().toISOString(),
    message: { messageId: randomUUID(), role: Role.ROLE_AGENT, taskId, contextId, parts: [textPart(job.text || job.result?.text || job.state)], extensions: [], referenceTaskIds: [] } },
    artifacts: [], history: [], metadata: { aiconnectJobId: job.jobId, localReviewStatus: job.localReviewStatus || 'pending' } };
}

/** Explicit opt-in loopback ingress. Fixed local grant/capability; no arbitrary Core proxy. */
export async function startA2AServer(core, config) {
  for (const key of ['provider','sessionId','capability','grantId','audience']) if (typeof config[key] !== 'string' || !config[key]) throw new Error(`missing_${key}`);
  const bearer = config.token ?? (config.tokenEnv && process.env[config.tokenEnv]);
  if (typeof bearer !== 'string' || Buffer.byteLength(bearer) < 32) throw new Error('strong_local_bearer_required');
  const app = express();
  app.disable('x-powered-by');
  app.use((req,res,next) => {
    // A browser cannot use a localhost bridge as a cross-origin confused deputy.
    if (req.headers.origin || !/^(127\.0\.0\.1|\[::1\])(?::\d+)?$/.test(req.headers.host || '')) return res.status(403).json({ error: 'bridge_origin_denied' });
    const actual = Buffer.from(req.headers.authorization || ''); const expected = Buffer.from(`Bearer ${bearer}`);
    if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) return res.status(401).json({ error: 'authentication_required' });
    next();
  });
  app.use(express.json({ limit: '32kb', strict: true }));
  let card;
  const scope = createHash('sha256').update(JSON.stringify([bearer,config.provider,config.sessionId,config.capability,config.audience])).digest('hex');
  const store = new CoreTaskStore(core,scope);
  let stopping = false;
  const executor = {
    async execute(context,bus) {
      if(context.task) throw new Error('bridge_followup_requires_new_job');
      // Fixed route configured by local owner. Remote metadata cannot supply a grant, provider or price.
      const { job } = await core({ action: 'service.invoke', transport: 'native', provider: config.provider,
        capability: config.capability, sessionId: config.sessionId, grantId: config.grantId, audience: config.audience,
        ...(config.descriptor ? {descriptor:config.descriptor} : {}),
        input: { parts: context.userMessage.parts }, externalTaskId: context.taskId,
        idempotencyKey: `a2a:${scope}:${createHash('sha256').update(context.userMessage.messageId).digest('hex')}`, ttlSeconds: Math.min(config.ttlSeconds || 300,3600),
        declaredPrice: { amount: 0, currency: 'USD' } });
      let latest = jobTask(job,context.taskId,context.contextId);
      latest.metadata.aiconnectBridgeScope = scope;
      bus.publish(AgentEvent.task(latest));
      for (let n = 0; n < Math.min(config.maxPolls || 60,180); n++) {
        if (stopping) { bus.finished(); return; }
        const { job: fresh } = await core({ action: 'service.status', jobId: job.jobId });
        latest = jobTask(fresh,context.taskId,context.contextId);
        const done = terminal(stateName.get(latest.status.state));
        bus.publish(AgentEvent.statusUpdate({ taskId: context.taskId, contextId: context.contextId, status: latest.status, final: done, metadata: latest.metadata }));
        if (done) { bus.finished(); return; }
        await pause(1000);
      }
      if (stopping) { bus.finished(); return; }
      bus.publish(AgentEvent.statusUpdate({ taskId: context.taskId, contextId: context.contextId,
        status: { state: TaskState.TASK_STATE_INPUT_REQUIRED, message: { messageId: randomUUID(), role: Role.ROLE_AGENT, parts: [textPart('Local job still pending; query task again or inspect AIConnect Job.')], taskId: context.taskId, contextId: context.contextId, extensions: [], referenceTaskIds: [] } }, final: true }));
      bus.finished();
    },
    async cancelTask(taskId,bus) {
      const { job } = await core({ action: 'service.lookup', externalTaskId: taskId });
      await core({ action: 'service.cancel', jobId: job.jobId });
      // Do not assert that an external side effect was undone.
      bus.publish(AgentEvent.statusUpdate({ taskId, contextId: job.externalTask?.contextId || '',
        status: { state: TaskState.TASK_STATE_INPUT_REQUIRED, message: { messageId: randomUUID(), role: Role.ROLE_AGENT, parts: [textPart('Cancellation requested; confirmation is pending.')], taskId, contextId: job.externalTask?.contextId || '', extensions: [], referenceTaskIds: [] } }, final: true }));
      bus.finished();
    },
  };
  const userBuilder = async () => ({ userName: 'local-authorized-bridge', isAuthenticated: true });
  const server = await new Promise(resolve => { const s = app.listen(config.port || 0,'127.0.0.1',() => resolve(s)); });
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  card = { name: config.name || 'AIConnect local gateway', description: 'Locally authorized native service; no central gateway.',
    supportedInterfaces: [{ url: `${baseUrl}/a2a`, protocolBinding: 'JSONRPC', protocolVersion: '1.0', tenant: '' }],
    version: '0.1.0', capabilities: { streaming: true, pushNotifications: false, extensions: [] },
    securitySchemes: { bearer: { scheme: { $case: 'httpAuthSecurityScheme', value: { scheme: 'bearer', bearerFormat: '' } } } },
    securityRequirements: [{ schemes: { bearer: { list: [] } } }], defaultInputModes: ['text/plain','application/json'], defaultOutputModes: ['text/plain'],
    skills: [{ id: config.capability, name: config.capability, description: 'Fixed local authorized service', tags: [], examples: [], inputModes: [], outputModes: [] }], signatures: [] };
  const handler = new DefaultRequestHandler(card,store,executor);
  app.use('/.well-known/agent-card.json',agentCardHandler({ agentCardProvider: handler, cache: { maxAge: 0 } }));
  app.use('/a2a',jsonRpcHandler({ requestHandler: handler, userBuilder }));
  return { server, baseUrl, card, close: () => { stopping = true; return new Promise(resolve => { server.closeAllConnections?.(); server.close(resolve); }); } };
}
