#!/usr/bin/env node
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { coreClient } from './core-ipc.mjs';
import { makeA2AClient, processA2AJobs, startA2AServer } from './a2a.mjs';
import { makeMcpServer } from './mcp.mjs';
import { normalizeAgentCard, normalizeRegistryPage } from './normalize.mjs';

const { positionals, values } = parseArgs({ allowPositionals: true, options: {
  data: { type: 'string' }, config: { type: 'string' }, url: { type: 'string' }, once: { type: 'boolean' },
} });
const command = positionals[0];
if (!values.data || !['a2a','mcp','a2a-serve','a2a-inspect','mcp-registry'].includes(command)) {
  process.stderr.write('Usage: node bridge.mjs a2a|mcp|a2a-serve|a2a-inspect|mcp-registry --data DIR [--config FILE] [--once] [--url URL]\n');
  process.exitCode = 2;
} else {
  try {
    const core = coreClient(resolve(values.data));
    const config = values.config ? JSON.parse(await readFile(resolve(values.config),'utf8')) : {};
    if (command === 'mcp') {
      await makeMcpServer(core,{enableInvoke:config.enableInvoke===true}).connect(new StdioServerTransport());
    } else if (command === 'a2a-inspect') {
      if (!values.url) throw new Error('missing_url');
      const provider = config.providers?.[new URL(values.url).origin] || {};
      const { card } = await makeA2AClient(core,values.url,{ ...provider, token: provider.tokenEnv ? process.env[provider.tokenEnv] : undefined });
      process.stdout.write(`${JSON.stringify({ card, normalized:normalizeAgentCard(card,values.url),provenance: 'https_provider_observation', authorSignatureVerified: false, invocationPerformed: false })}\n`);
    } else if (command === 'mcp-registry') {
      const url=values.url || 'https://registry.modelcontextprotocol.io/v0.1/servers?version=latest&limit=32';
      const response=await core({action:'governance.http.get',url,maxBytes:256*1024});
      if (response.status!==200) throw new Error(`registry_http_${response.status}`);
      process.stdout.write(`${JSON.stringify(normalizeRegistryPage(JSON.parse(response.body),url))}\n`);
    } else if (command === 'a2a-serve') {
      if (!values.config) throw new Error('explicit_ingress_config_required');
      const bridge = await startA2AServer(core,config);
      process.stdout.write(`${JSON.stringify({ listening: bridge.baseUrl, scope: 'loopback', centralGateway: false })}\n`);
      for (const signal of ['SIGINT','SIGTERM']) process.once(signal,async () => { await bridge.close(); process.exit(0); });
    } else {
      let stopping = false;
      for (const signal of ['SIGINT','SIGTERM']) process.once(signal,() => { stopping = true; });
      do {
        const report = await processA2AJobs(core,config);
        config.offset = report.nextOffset;
        process.stdout.write(`${JSON.stringify(report)}\n`);
        if (!values.once && !stopping) await new Promise(resolve => setTimeout(resolve,2000));
      } while (!values.once && !stopping);
    }
  } catch (error) {
    // Never print IPC metadata, HTTP bodies, credentials or provider diagnostics.
    process.stderr.write(`AIConnect bridge stopped: ${/^[a-zA-Z0-9_ .:-]{1,140}$/.test(error.message || '') ? error.message : 'bridge_error'}\n`);
    process.exitCode = 1;
  }
}
