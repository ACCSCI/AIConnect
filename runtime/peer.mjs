import { Endpoint, EndpointTicket, SecretKey, Signature, RelayMode, presetMinimal } from '@number0/iroh';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { LocalStore } from './store.mjs';

const ALPN=[...Buffer.from('aiconnect/message/1')];
const LIMIT=32768;
const bytes=value=>[...Buffer.from(JSON.stringify(value))];
const fail=(message)=>{throw new Error(message);};
const equal=(a,b)=>{const x=Buffer.from(String(a)),y=Buffer.from(String(b));return x.length===y.length&&timingSafeEqual(x,y);};
export async function bounded(promise,ms,label='timeout') { let timer;try{return await Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error(label)),ms);})]);}finally{clearTimeout(timer);} }
export function publicSession(s) { const { capability, invite, peerTicket, ...safe }=s;return safe; }

export class Peer extends EventEmitter {
  static async open({directory,directOnly=false,bindAddr,relayUrls,now=Date.now}={}) {
    const peer=new Peer();peer.now=now;peer.directory=directory;peer.directOnly=directOnly;peer.closed=false;peer.inFlight=0;
    await mkdir(directory,{recursive:true,mode:0o700});
    const keyPath=join(directory,'identity.key');let key;
    try{key=SecretKey.fromBytes([...await readFile(keyPath)]);}catch(error){if(error.code!=='ENOENT')throw error;key=SecretKey.generate();await writeFile(keyPath,Buffer.from(key.toBytes()),{mode:0o600,flag:'wx'});}
    peer.key=key;peer.store=new LocalStore(directory);
    const builder=Endpoint.builder();
    if(directOnly)presetMinimal(builder);else builder.applyN0();
    if(relayUrls?.length)builder.relayMode(RelayMode.customFromUrls(relayUrls));
    if(bindAddr)builder.bindAddr(bindAddr);
    builder.secretKey(key.toBytes());builder.alpns([ALPN]);
    peer.endpoint=await builder.bind();peer.id=peer.endpoint.id().toString();
    peer.acceptTask=peer.acceptLoop();
    peer.timer=setInterval(()=>{peer.expire();peer.retry().catch(()=>{});},2000);peer.timer.unref();
    return peer;
  }
  async ticket() {
    if(!this.directOnly)await bounded(this.endpoint.online(),6000,'relay_not_ready').catch(()=>{});
    return EndpointTicket.fromAddr(this.endpoint.addr()).toString();
  }
  async create({label='临时协作',ttlSeconds=1800}={}) {
    if(!Number.isInteger(ttlSeconds)||ttlSeconds<1||ttlSeconds>86400)fail('invalid_ttl');
    if(typeof label!=='string'||label.length>80)fail('invalid_label');
    const body={v:1,kind:'temporary',id:randomUUID(),label,host:this.id,ticket:await this.ticket(),capability:randomBytes(32).toString('base64url'),created:this.now(),expires:this.now()+ttlSeconds*1000};
    const serialized=Buffer.from(JSON.stringify(body));
    const invite=Buffer.from(JSON.stringify({body:serialized.toString('base64url'),signature:Buffer.from(this.key.sign([...serialized]).toBytes()).toString('base64url')})).toString('base64url');
    this.store.putSession({...body,role:'host',status:'waiting',peerId:null,peerTicket:null,invite});
    this.emit('change');return {session:publicSession(this.store.session(body.id)),invite};
  }
  decodeInvite(invite) {
    if(typeof invite!=='string'||invite.length>12000)fail('invalid_invite');
    let envelope,raw,body,addr;
    try {
      envelope=JSON.parse(Buffer.from(invite,'base64url'));raw=Buffer.from(envelope.body,'base64url');body=JSON.parse(raw);
      addr=EndpointTicket.fromString(body.ticket).endpointAddr();
      if(addr.id().toString()!==body.host)fail('invalid_invite_identity');
      addr.id().verify([...raw],Signature.fromBytes([...Buffer.from(envelope.signature,'base64url')]));
    }catch{fail('invalid_invite_signature');}
    if(body.v!==1||body.kind!=='temporary'||typeof body.id!=='string'||typeof body.capability!=='string'||!Number.isSafeInteger(body.expires)||body.expires<=this.now())fail('invite_expired_or_invalid');
    if(body.host===this.id)fail('cannot_join_self');
    return body;
  }
  async join(invite) {
    const body=this.decodeInvite(invite);const old=this.store.session(body.id);
    if(old&&old.status!=='open')fail('session_ended');
    const temporary={...body,role:'guest',peerId:body.host,peerTicket:body.ticket,status:'open',invite};
    const ack=await this.rpc(body.ticket,{kind:'join',sessionId:body.id,capability:body.capability,ticket:await this.ticket()},body.host);
    if(ack.sessionId!==body.id||ack.status!=='open')fail('join_rejected');
    temporary.expires=Math.min(body.expires,ack.expires);this.store.putSession(temporary);this.emit('change');return publicSession(temporary);
  }
  active(id) {
    const session=this.store.session(id);if(!session)fail('session_not_found');
    if(session.expires<=this.now()){this.store.end(session,'expired');fail('session_expired');}
    if(!['waiting','open'].includes(session.status))fail('session_ended');return session;
  }
  expire() { for(const s of this.store.sessions())if(['open','waiting'].includes(s.status)&&s.expires<=this.now()){this.store.end(s,'expired');this.emit('change');} }
  async send(sessionId,text,{messageId=randomUUID()}={}) {
    const s=this.active(sessionId);if(s.status!=='open'||!s.peerTicket)fail('waiting_for_peer');
    if(typeof text!=='string'||!text.trim()||Buffer.byteLength(text)>16000)fail('invalid_message');
    this.store.insertMessage({id:messageId,sessionId,sender:this.id,text,created:this.now(),status:'pending'});
    const message=this.store.message(messageId);if(message.status==='acked')return {id:messageId,status:'acked',duplicate:true};
    await this.deliver(s,message);this.emit('change');return {id:messageId,status:'acked'};
  }
  async deliver(session,msg) {
    this.active(session.id);
    const ack=await this.rpc(session.peerTicket,{kind:'message',sessionId:session.id,capability:session.capability,id:msg.id,text:msg.text,created:msg.created},session.peerId);
    if(ack.ack!==msg.id||ack.persisted!==true)fail('invalid_ack');
    this.store.mark(msg.id,'acked');
  }
  async retry() {
    if(this.retrying||this.closed)return;this.retrying=true;
    try{for(const msg of this.store.pending()){try{await this.deliver(this.active(msg.session_id),msg);this.emit('change');}catch{}}}finally{this.retrying=false;}
  }
  async end(id) {
    const s=this.active(id);this.store.end(s,'ended');this.emit('change');
    if(s.peerTicket)await this.rpc(s.peerTicket,{kind:'end',sessionId:id,capability:s.capability},s.peerId).catch(()=>{});
    return {ended:true};
  }
  async rpc(ticket,packet,expectedId) {
    let conn;
    try {
      const addr=EndpointTicket.fromString(ticket).endpointAddr();
      if(addr.id().toString()!==expectedId)fail('peer_identity_mismatch');
      conn=await bounded(this.endpoint.connect(addr,ALPN),7000,'peer_unreachable');
      if(conn.remoteId().toString()!==expectedId)fail('peer_identity_mismatch');
      // Allow a short direct-path preference window before sending application data.
      const until=Date.now()+300;
      while(!this.directOnly&&!conn.paths().some(p=>p.isSelected&&p.isIp)&&Date.now()<until)await new Promise(r=>setTimeout(r,30));
      this.emit('path',{peerId:expectedId,direct:conn.paths().some(p=>p.isSelected&&p.isIp),relay:conn.paths().some(p=>p.isSelected&&p.isRelay)});
      const stream=await conn.openBi();await stream.send.writeAll(bytes(packet));await stream.send.finish();
      const response=JSON.parse(Buffer.from(await bounded(stream.recv.readToEnd(LIMIT),5000,'ack_timeout')).toString());
      if(response.error)fail(response.error);return response;
    }finally{conn?.close(0n,[]);}
  }
  async acceptLoop() {
    while(!this.closed){try{const incoming=await this.endpoint.acceptNext();if(!incoming)break;if(this.inFlight>=32){await incoming.refuse();continue;}this.inFlight++;this.handle(incoming).catch(()=>{}).finally(()=>this.inFlight--);}catch{if(!this.closed)await new Promise(r=>setTimeout(r,50));}}
  }
  async handle(incoming) {
    let conn;
    try {
      conn=await bounded((await incoming.accept()).connect(),5000);
      const remote=conn.remoteId().toString();const stream=await bounded(conn.acceptBi(),5000);
      let response;
      try{
        const packet=JSON.parse(Buffer.from(await bounded(stream.recv.readToEnd(LIMIT),5000)).toString());
        const s=this.active(packet.sessionId);
        if(!equal(s.capability,packet.capability))fail('invalid_capability');
        if(packet.kind==='join'){
          if(s.role!=='host'||(s.peerId&&s.peerId!==remote))fail('invite_already_used');
          const addr=EndpointTicket.fromString(packet.ticket).endpointAddr();if(addr.id().toString()!==remote)fail('peer_identity_mismatch');
          s.peerId=remote;s.peerTicket=packet.ticket;s.status='open';this.store.putSession(s);
          response={sessionId:s.id,status:'open',expires:s.expires};
        }else{
          if(s.status!=='open'||s.peerId!==remote)fail('peer_not_allowed');
          if(packet.kind==='message'){
            if(typeof packet.id!=='string'||packet.id.length>80||typeof packet.text!=='string'||!packet.text.trim()||Buffer.byteLength(packet.text)>16000||!Number.isSafeInteger(packet.created))fail('invalid_message');
            const inserted=this.store.insertMessage({id:packet.id,sessionId:s.id,sender:remote,text:packet.text,created:packet.created,status:'received'});
            response={ack:packet.id,persisted:true};if(inserted)this.emit('message',{sessionId:s.id,id:packet.id,text:packet.text,sender:remote});
          }else if(packet.kind==='end'){this.store.end(s,'ended');response={ended:true};}else fail('unsupported_message_kind');
        }
        this.emit('change');
      }catch(error){response={error:error.message};}
      await stream.send.writeAll(bytes(response));await stream.send.finish();
      await bounded(stream.send.stopped(),1500).catch(()=>{});
    }finally{conn?.close(0n,[]);}
  }
  async close(){if(this.closed)return;this.closed=true;clearInterval(this.timer);await this.endpoint.close();await this.acceptTask;while(this.inFlight)await new Promise(r=>setTimeout(r,10));this.store.close();}
}
