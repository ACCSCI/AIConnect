#!/usr/bin/env node
import {readFile,writeFile} from 'node:fs/promises';
import {createHash,createPrivateKey,createPublicKey,sign} from 'node:crypto';
import {canonical} from './install.mjs';
const args=process.argv.slice(2);const option=(flag)=>{const i=args.indexOf(flag);return i<0?undefined:args[i+1];};
try{
 const input=option('--input'),output=option('--output'),keyFile=option('--key-file');
 if(!input||!output)throw Error('Usage: node build-manifest.mjs --input release-input.json --output manifest.json [--key-file PRIVATE_KEY_PEM]');
 const spec=JSON.parse(await readFile(input,'utf8'));if(!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(spec.version))throw Error('Invalid release version');
 const artifacts={};for(const [target,artifact] of Object.entries(spec.artifacts||{})){if(!/^(windows|linux|macos)-(x86_64|aarch64)$/.test(target))throw Error('Unsupported target');const bytes=await readFile(artifact.path);if(bytes.length<1||bytes.length>104857600)throw Error('Artifact must be 1..100 MiB');const chunkSize=262144;const chunks={size:chunkSize,sha256:[]};for(let i=0;i<bytes.length;i+=chunkSize)chunks.sha256.push(createHash('sha256').update(bytes.subarray(i,i+chunkSize)).digest('hex'));artifacts[target]={url:artifact.url,sha256:createHash('sha256').update(bytes).digest('hex'),size:bytes.length,format:'binary',fileName:target.startsWith('windows-')?'aiconnect.exe':'aiconnect',chunks};}
 if(!Object.keys(artifacts).length)throw Error('No artifacts');const payload={schemaVersion:1,version:spec.version,artifacts};const manifest={payload};
 let result={signed:false,warning:'Unsigned manifests are local-test only; production installer refuses them.'};
 if(keyFile){const key=createPrivateKey(await readFile(keyFile));if(key.asymmetricKeyType!=='ed25519')throw Error('Release signing requires an Ed25519 key');manifest.signature={algorithm:'ed25519',keyId:spec.keyId||'release',value:sign(null,Buffer.from(canonical(payload)),key).toString('base64')};const publicKey=createPublicKey(key).export({type:'spki',format:'der'}).toString('base64');result={signed:true,publicKey};}
 await writeFile(output,JSON.stringify(manifest,null,2)+'\n',{flag:'wx'});
 console.log(JSON.stringify(result));
}catch(error){console.error(JSON.stringify({error:error.message}));process.exitCode=1;}
