#!/usr/bin/env node
/** Bootstrap-time Node only. Installed AIConnect is a native executable. */
import {createHash,createPublicKey,verify} from 'node:crypto';
import {mkdir,readFile,writeFile,rename,rm,chmod,mkdtemp} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {homedir} from 'node:os';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
const run=promisify(execFile);
const args=process.argv.slice(2);
function option(flag,fallback){const i=args.indexOf(flag);if(i<0)return fallback;if(!args[i+1]||args[i+1].startsWith('--'))throw Error(`Missing value for ${flag}`);return args[i+1];}
export function canonical(value){if(value===null||typeof value!=='object')return JSON.stringify(value);if(Array.isArray(value))return '['+value.map(canonical).join(',')+']';return '{'+Object.keys(value).sort().map(k=>JSON.stringify(k)+':'+canonical(value[k])).join(',')+'}';}
function validUrl(raw,allowHttp){const url=new URL(raw);if(url.username||url.password)throw Error('URL credentials are forbidden');if(url.protocol==='https:')return url;if(allowHttp&&url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname))return url;throw Error('HTTPS required (HTTP is allowed only for explicit localhost tests)');}
async function download(raw,maxBytes,allowHttp){let url=validUrl(raw,allowHttp);for(let redirects=0;redirects<6;redirects++){const response=await fetch(url,{redirect:'manual',signal:AbortSignal.timeout(60000)});if([301,302,303,307,308].includes(response.status)){url=validUrl(new URL(response.headers.get('location'),url).href,allowHttp);continue;}if(!response.ok)throw Error(`Download failed: HTTP ${response.status}`);const size=Number(response.headers.get('content-length'));if(size>maxBytes)throw Error('Download exceeds size limit');const chunks=[];let total=0;for await(const chunk of response.body){total+=chunk.length;if(total>maxBytes)throw Error('Download exceeds size limit');chunks.push(chunk);}return Buffer.concat(chunks);}throw Error('Too many redirects');}
function target(){const os={win32:'windows',linux:'linux',darwin:'macos'}[process.platform],arch={x64:'x86_64',arm64:'aarch64'}[process.arch];if(!os||!arch)throw Error(`Unsupported platform: ${process.platform}/${process.arch}`);return `${os}-${arch}`;}
async function exists(path){try{await readFile(path);return true;}catch(error){if(error.code==='ENOENT')return false;throw error;}}
async function main(){
 const installDir=resolve(option('--install-dir',process.platform==='win32'?join(process.env.LOCALAPPDATA||join(homedir(),'AppData','Local'),'AIConnect','bin'):join(homedir(),'.local','bin')));
 const fileName=process.platform==='win32'?'aiconnect.exe':'aiconnect';const destination=join(installDir,fileName),receiptPath=join(installDir,'aiconnect-install.json');
 if(args.includes('--uninstall')){await rm(destination,{force:true});await rm(receiptPath,{force:true});console.log(JSON.stringify({uninstalled:true,dataPreserved:true,installDir}));return;}
 const allowHttp=args.includes('--allow-http'),allowUnsigned=args.includes('--allow-unsigned');
 if(allowUnsigned&&!allowHttp)throw Error('Unsigned manifests are restricted to explicit local HTTP tests');
 const manifestUrl=option('--manifest');if(!manifestUrl)throw Error('Required: --manifest HTTPS_URL --public-key BASE64_DER');
 if(allowUnsigned&&!['localhost','127.0.0.1','[::1]'].includes(new URL(manifestUrl).hostname))throw Error('Unsigned manifests require a localhost manifest');
 const raw=await download(manifestUrl,1024*1024,allowHttp),manifest=JSON.parse(raw),payload=manifest.payload;
 if(!payload||payload.schemaVersion!==1)throw Error('Unsupported manifest schema');
 if(!allowUnsigned){const key=option('--public-key');if(!key)throw Error('A pinned release public key is required; no production key has been configured');let publicKey;try{publicKey=createPublicKey({key:Buffer.from(key,'base64'),type:'spki',format:'der'});}catch{throw Error('Invalid pinned release public key');}if(publicKey.asymmetricKeyType!=='ed25519'||manifest.signature?.algorithm!=='ed25519'||!verify(null,Buffer.from(canonical(payload)),publicKey,Buffer.from(manifest.signature.value||'','base64')))throw Error('Manifest signature verification failed');}
 if(!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(payload.version))throw Error('Invalid release version');
 const requested=option('--version');if(requested&&requested!==payload.version)throw Error('Requested version does not match manifest');
 const artifact=payload.artifacts?.[target()];if(!artifact)throw Error(`No artifact for ${target()}`);
 if(artifact.format!=='binary'||artifact.fileName!==fileName||!/^[a-f0-9]{64}$/.test(artifact.sha256)||!Number.isSafeInteger(artifact.size)||artifact.size<=0||artifact.size>100*1024*1024)throw Error('Invalid artifact metadata');
 const artifactUrl=new URL(artifact.url,manifestUrl).href;validUrl(artifactUrl,allowHttp);
 const bytes=await download(artifactUrl,artifact.size,allowHttp);if(bytes.length!==artifact.size||createHash('sha256').update(bytes).digest('hex')!==artifact.sha256)throw Error('Artifact integrity verification failed');
 await mkdir(installDir,{recursive:true});const stageDir=await mkdtemp(join(installDir,'.aiconnect-stage-'));const staged=join(stageDir,fileName);const backup=join(stageDir,'previous');let backedUp=false,installed=false;
 try{await writeFile(staged,bytes,{mode:0o700,flag:'wx'});if(process.platform!=='win32')await chmod(staged,0o700);const smoke=await run(staged,['--version'],{timeout:15000,windowsHide:true});if(smoke.stdout.trim()!==`aiconnect ${payload.version}`)throw Error('Executable version smoke test failed');if(await exists(destination)){await rename(destination,backup);backedUp=true;}await rename(staged,destination);installed=true;const receipt={version:payload.version,target:target(),manifestUrl,sha256:artifact.sha256,signatureVerified:!allowUnsigned,publicKey:allowUnsigned?undefined:option('--public-key'),automaticUpdates:!allowUnsigned,installedAt:new Date().toISOString()};const receiptStage=join(stageDir,'receipt.json');await writeFile(receiptStage,JSON.stringify(receipt,null,2)+'\n',{mode:0o600});await rename(receiptStage,receiptPath);console.log(JSON.stringify({installed:true,version:payload.version,path:destination,dataPreserved:true,pathConfiguration:'Use this absolute path or add the install directory to PATH.'}));}
 catch(error){if(installed)await rm(destination,{force:true});if(backedUp)await rename(backup,destination);throw error;}
 finally{await rm(stageDir,{recursive:true,force:true});}
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){main().catch(error=>{console.error(JSON.stringify({error:error.message}));process.exitCode=1;});}
