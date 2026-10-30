import { createServer } from 'node:http';
import { readFile,writeFile,mkdir,unlink } from 'node:fs/promises';
import { randomBytes,timingSafeEqual } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { join,resolve } from 'node:path';
import { Peer,publicSession } from './peer.mjs';

const root=fileURLToPath(new URL('../',import.meta.url));
export async function startDaemon({directory,port=18871,directOnly=false,bindAddr,relayUrls}={}){
  await mkdir(directory,{recursive:true,mode:0o700});
  const peer=await Peer.open({directory,directOnly,bindAddr,relayUrls});
  const token=randomBytes(32).toString('base64url');const paths=new Map();
  peer.on('path',path=>paths.set(path.peerId,path));
  const authenticate=req=>{
    const expected=Buffer.from(`Bearer ${token}`),actual=Buffer.from(req.headers.authorization||'');
    return actual.length===expected.length&&timingSafeEqual(actual,expected);
  };
  async function jsonBody(req){let body='';for await(const chunk of req){body+=chunk;if(Buffer.byteLength(body)>40000)throw new Error('body_too_large');}return body?JSON.parse(body):{};}
  const server=createServer(async(req,res)=>{
    const address=server.address();const allowed=new Set([`127.0.0.1:${address.port}`,`localhost:${address.port}`]);
    const send=(status,body)=>{res.writeHead(status,{'Content-Type':'application/json; charset=utf-8','Cache-Control':'no-store'});res.end(JSON.stringify(body));};
    if(!allowed.has(req.headers.host)){send(403,{error:'invalid_host'});return;}
    const url=new URL(req.url,`http://127.0.0.1:${address.port}`);
    if(req.headers.origin&&!new Set([`http://127.0.0.1:${address.port}`,`http://localhost:${address.port}`]).has(req.headers.origin)){send(403,{error:'invalid_origin'});return;}
    if(!url.pathname.startsWith('/api/')){
      if(!['/','/local.html','/local.js','/styles.css'].includes(url.pathname)||req.method!=='GET'){send(404,{error:'not_found'});return;}
      const filename=url.pathname==='/'?'local.html':url.pathname.slice(1);
      try{const content=await readFile(resolve(root,filename));res.writeHead(200,{'Content-Type':filename.endsWith('.html')?'text/html; charset=utf-8':filename.endsWith('.js')?'text/javascript; charset=utf-8':'text/css; charset=utf-8','Cache-Control':'no-store','Referrer-Policy':'no-referrer','Content-Security-Policy':"default-src 'self'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src https://fonts.gstatic.com; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"});res.end(content);}catch{send(404,{error:'not_found'});}return;
    }
    if(!authenticate(req)){send(401,{error:'local_authorization_required'});return;}
    try{
      peer.expire();
      if(url.pathname==='/api/status'&&req.method==='GET'){send(200,{version:'0.2.0',id:peer.id,p2p:true,directOnly,contactsCreated:0,paths:[...paths.values()]});return;}
      if(url.pathname==='/api/sessions'&&req.method==='GET'){send(200,{sessions:peer.store.sessions().map(publicSession)});return;}
      if(url.pathname==='/api/sessions'&&req.method==='POST'){send(201,await peer.create(await jsonBody(req)));return;}
      if(url.pathname==='/api/join'&&req.method==='POST'){send(200,{session:await peer.join((await jsonBody(req)).invite)});return;}
      const m=url.pathname.match(/^\/api\/sessions\/([a-zA-Z0-9-]+)\/(messages|end)$/);
      if(m){const id=m[1];if(!peer.store.session(id)){send(404,{error:'session_not_found'});return;}
        if(m[2]==='messages'&&req.method==='GET'){send(200,{messages:peer.store.messages(id)});return;}
        if(m[2]==='messages'&&req.method==='POST'){const body=await jsonBody(req);send(200,await peer.send(id,body.text));return;}
        if(m[2]==='end'&&req.method==='POST'){send(200,await peer.end(id));return;}
      }
      send(404,{error:'not_found'});
    }catch(error){const known=['session_ended','session_expired','invite_already_used','invite_expired_or_invalid'];send(known.includes(error.message)?410:400,{error:error.message});}
  });
  try{await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(port,'127.0.0.1',resolve);});}catch(error){await peer.close();throw error;}
  const info={port:server.address().port,token,id:peer.id,pid:process.pid};
  await writeFile(join(directory,'daemon.json'),JSON.stringify(info),{mode:0o600});
  let closed=false;
  return {peer,server,info,async close(){if(closed)return;closed=true;await new Promise(resolve=>server.close(resolve));await peer.close();await unlink(join(directory,'daemon.json')).catch(()=>{});}};
}
