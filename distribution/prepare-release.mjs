#!/usr/bin/env node
// Prepare immutable release assets from completed, tested CI artifacts.
import {readFile,writeFile,mkdir,copyFile} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {execFileSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
const args=process.argv.slice(2);const arg=name=>{const at=args.indexOf(name);return at<0?undefined:args[at+1]};
const artifacts=arg('--artifacts'),output=arg('--output'),keyFile=arg('--key-file');
if(!artifacts||!output||!keyFile)throw Error('Required: --artifacts DIRECTORY --output NEW_DIRECTORY --key-file PRIVATE_PEM');
const config=JSON.parse(await readFile(new URL('./release.json',import.meta.url),'utf8'));
await mkdir(output,{recursive:true});
const spec={version:config.version,keyId:'aiconnect-release-2026',artifacts:{}};
for(const target of ['windows-x86_64','linux-x86_64','macos-aarch64']){
 const executable=target.startsWith('windows')?'aiconnect.exe':'aiconnect';
 const asset=`aiconnect-${target}${target.startsWith('windows')?'.exe':''}`;
 const source=resolve(artifacts,target,executable),destination=resolve(output,asset);
 await copyFile(source,destination);
 spec.artifacts[target]={path:destination,url:`${config.repository}/releases/download/v${config.version}/${asset}`};
}
const input=resolve(output,'release-input.json');await writeFile(input,JSON.stringify(spec,null,2)+'\n',{flag:'wx'});
const manifest=resolve(output,'stable.json');
const report=JSON.parse(execFileSync(process.execPath,[fileURLToPath(new URL('./build-manifest.mjs',import.meta.url)),'--input',input,'--output',manifest,'--key-file',resolve(keyFile)],{encoding:'utf8',windowsHide:true}));
if(report.publicKey!==config.publicKey)throw Error('Signing key does not match the independently pinned release public key');
const hashes=[];
for(const artifact of Object.values(spec.artifacts)){const bytes=await readFile(artifact.path);hashes.push(`${createHash('sha256').update(bytes).digest('hex')}  ${artifact.path.split(/[\\/]/).pop()}`);}
await writeFile(join(output,'SHA256SUMS'),hashes.join('\n')+'\n');
console.log(JSON.stringify({prepared:true,version:spec.version,targets:Object.keys(spec.artifacts),signed:true,manifest}));
