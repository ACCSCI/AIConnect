import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp,mkdir } from 'node:fs/promises';
import { join,resolve } from 'node:path';
import { Peer } from '../runtime/peer.mjs';
import { startDaemon } from '../runtime/daemon.mjs';
import { request } from 'node:http';
const root=resolve('.local/test-state');await mkdir(root,{recursive:true});
async function makePeer(name,now){return Peer.open({directory:await mkdtemp(join(root,name)),directOnly:true,bindAddr:'127.0.0.1:0',now});}

test('signed one-peer temporary invitation, actual P2P messages, deduplication and revocation',async()=>{
  const a=await makePeer('a-'),b=await makePeer('b-'),c=await makePeer('c-');
  try{
    const created=await a.create({label:'一次临时合作',ttlSeconds:60});
    assert.equal(created.session.capability,undefined);
    const changed=JSON.parse(Buffer.from(created.invite,'base64url'));
    const body=JSON.parse(Buffer.from(changed.body,'base64url'));body.expires+=86400000;
    changed.body=Buffer.from(JSON.stringify(body)).toString('base64url');
    await assert.rejects(b.join(Buffer.from(JSON.stringify(changed)).toString('base64url')),/invalid_invite_signature/);
    const session=await b.join(created.invite);assert.equal(session.peerId,a.id);
    await assert.rejects(c.join(created.invite),/invite_already_used/);
    const paths=[];b.on('path',p=>paths.push(p));
    assert.equal((await b.send(session.id,'你好，临时连接。',{messageId:'stable-message-id'})).status,'acked');
    await b.send(session.id,'你好，临时连接。',{messageId:'stable-message-id'});
    assert.equal(a.store.messages(session.id).length,1);assert(paths.some(p=>p.direct&&!p.relay));
    await a.send(session.id,'收到，消息只保存在本地。');assert.equal(b.store.messages(session.id).length,2);
    await a.end(session.id);assert.equal(b.store.session(session.id).status,'ended');
    await assert.rejects(b.send(session.id,'不能再发送'),/session_ended/);
    await assert.rejects(c.join(created.invite),/session_ended/);
    assert.equal(a.store.messages(session.id).length,2);
  }finally{await Promise.all([a.close(),b.close(),c.close()]);}
});
test('expiry rejects existing traffic and reused invitation',async()=>{
  let clock=Date.now();const now=()=>clock;
  const a=await makePeer('expiry-a-',now),b=await makePeer('expiry-b-',now);
  try{const created=await a.create({ttlSeconds:2});await b.join(created.invite);clock+=3000;a.expire();b.expire();
    await assert.rejects(b.send(created.session.id,'too late'),/session_expired/);
    await assert.rejects(b.join(created.invite),/invite_expired_or_invalid/);
    assert.equal(a.store.session(created.session.id).status,'expired');assert.equal(b.store.session(created.session.id).status,'expired');
  }finally{await a.close();await b.close();}
});
test('endpoint identity and session restore from SQLite',async()=>{
  const directory=await mkdtemp(join(root,'restart-'));let peer=await Peer.open({directory,directOnly:true,bindAddr:'127.0.0.1:0'});
  const id=peer.id,created=await peer.create({ttlSeconds:60});await peer.close();
  peer=await Peer.open({directory,directOnly:true,bindAddr:'127.0.0.1:0'});
  try{assert.equal(peer.id,id);assert.equal(peer.store.session(created.session.id).status,'waiting');}finally{await peer.close();}
});
test('local API enforces bearer authorization and origin/host restrictions',async()=>{
  const daemon=await startDaemon({directory:await mkdtemp(join(root,'api-')),port:0,directOnly:true,bindAddr:'127.0.0.1:0'});
  try{const base=`http://127.0.0.1:${daemon.info.port}`;const headers={Authorization:`Bearer ${daemon.info.token}`};
    assert.equal((await fetch(`${base}/api/status`)).status,401);
    assert.equal((await fetch(`${base}/api/status`,{headers:{...headers,Origin:'https://evil.example'}})).status,403);
    const badHost=await new Promise((resolve,reject)=>{const req=request(`${base}/api/status`,{headers:{...headers,Host:'evil.example'}},res=>{res.resume();resolve(res.statusCode);});req.on('error',reject);req.end();});
    assert.equal(badHost,403);
    assert.equal((await(await fetch(`${base}/api/status`,{headers})).json()).contactsCreated,0);
    assert.equal((await fetch(`${base}/api/sessions`,{method:'POST',headers:{...headers,'Content-Type':'application/json'},body:JSON.stringify({ttlSeconds:60})})).status,201);
    const list=await(await fetch(`${base}/api/sessions`,{headers})).json();assert.equal(list.sessions[0].capability,undefined);
  }finally{await daemon.close();}
});
