const MAX = 256 * 1024;
function bounded(value) {
  if (!value || typeof value !== 'object' || Buffer.byteLength(JSON.stringify(value)) > MAX) throw new Error('metadata_too_large_or_invalid');
}
function string(value,max = 4000) { return typeof value === 'string' ? value.slice(0,max) : ''; }
function origin(source) {
  const url = new URL(source);
  if (!['https:','http:'].includes(url.protocol) || url.username || url.password) throw new Error('invalid_metadata_source');
  return url.origin;
}
/** Unsigned metadata adapter; this cannot be fed to the NativeDescriptor signature verifier. */
export function normalizeAgentCard(card,source) {
  bounded(card);
  const sourceOrigin=origin(source);
  if (!Array.isArray(card.supportedInterfaces) || !card.supportedInterfaces.length || !Array.isArray(card.skills)) throw new Error('invalid_a2a_card');
  const transports=card.supportedInterfaces.slice(0,16).map(i=>({ protocol:'a2a',binding:string(i.protocolBinding,40),version:string(i.protocolVersion,40),url:string(i.url,2048),requiresSeparateOriginAuthorization:origin(i.url)!==sourceOrigin }));
  return { schemaVersion:1,kind:'aiconnect.external-agent',externalId:sourceOrigin,name:string(card.name,160),description:string(card.description),
    skills:card.skills.slice(0,64).map(s=>({skillId:string(s.id,256),name:string(s.name,160),description:string(s.description),tags:(s.tags||[]).slice(0,32).map(t=>string(t,80)),inputModes:s.inputModes||card.defaultInputModes||[],outputModes:s.outputModes||card.defaultOutputModes||[],schemaAvailable:false})),
    transports,authRequirements:{securitySchemes:card.securitySchemes||{},securityRequirements:card.securityRequirements||[]},
    provenance:{source,observedAt:Date.now(),protocol:'a2a',sourceType:sourceOrigin.startsWith('https:')?'https-provider':'local-http-test',nativeAuthorSignatureVerified:false,ownerDelegationVerified:false,cardSignatureVerified:false},
    losses:['A2A skill descriptions generally have no callable JSON input/output schema.','HTTPS origin is not an AIConnect AgentKey or verified OwnerRoot.','Provider capability, price and quality remain unverified claims.','JWS signatures are preserved only in the original card; this adapter does not verify them.'],
    invocationAuthorized:false,original:card };
}

export function normalizeRegistryEntry(entry,source) {
  bounded(entry);
  const server=entry.server || entry;
  if (typeof server.name!=='string' || !server.name || typeof server.version!=='string') throw new Error('invalid_mcp_registry_entry');
  return {schemaVersion:1,kind:'aiconnect.external-tool-server',externalId:string(server.name,256),name:string(server.title||server.name,160),version:string(server.version,80),description:string(server.description),
    skills:[],transports:(server.remotes||[]).slice(0,16).map(r=>({protocol:'mcp',binding:string(r.type,40),url:string(r.url,2048),notConnected:true})),
    packages:(server.packages||[]).slice(0,16).map(p=>({registryType:string(p.registryType,40),identifier:string(p.identifier,512),version:string(p.version,80),transport:p.transport,notInstalled:true})),
    provenance:{source,observedAt:Date.now(),protocol:'mcp-registry',sourceType:origin(source).startsWith('https:')?'https-registry':'local-http-test',nativeAuthorSignatureVerified:false,ownerDelegationVerified:false,registryNamespaceIsNotOwnerIdentity:true},
    losses:['MCP registry describes tool servers, not Agent identities.','Registry metadata does not enumerate live tool schemas or prove tool safety.','Packages and remotes are metadata only: nothing is installed, connected or run.'],
    invocationAuthorized:false,original:entry};
}

export function normalizeRegistryPage(page,source) {
  bounded(page);
  if (!Array.isArray(page.servers) || page.servers.length>100) throw new Error('invalid_mcp_registry_page');
  return {entries:page.servers.map(e=>normalizeRegistryEntry(e,source)),nextCursor:typeof page.metadata?.nextCursor==='string'?page.metadata.nextCursor:null,source,automaticNextPage:false};
}
