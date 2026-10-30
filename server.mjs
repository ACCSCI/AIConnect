import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve, extname, sep } from 'node:path';
const root = fileURLToPath(new URL('.', import.meta.url));
const mime = { '.html':'text/html; charset=utf-8', '.css':'text/css; charset=utf-8', '.js':'text/javascript; charset=utf-8' };
createServer(async (req, res) => {
  try {
    const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    if(pathname==='/healthz'){res.writeHead(200,{'Content-Type':'application/json','Cache-Control':'no-store'});return res.end(JSON.stringify({ok:true,version:'0.2.0',mode:'public-entry',chatStorage:'client-local',workers:false}));}
    const path = resolve(root, '.' + (pathname === '/' ? '/index.html' : pathname === '/temporary' ? '/local.html' : pathname));
    if (!path.startsWith(root.endsWith(sep) ? root : root + sep)) { res.writeHead(403); return res.end(); }
    const file = extname(path) ? path : resolve(root, 'index.html');
    if (!['index.html','styles.css','app.js','local.html','local.js'].includes(file.slice(root.length))) { res.writeHead(404); return res.end(); }
    const body = await readFile(file);
    res.writeHead(200, { 'Content-Type': mime[extname(file)], 'Cache-Control':'no-store', 'Referrer-Policy':'no-referrer', 'X-Content-Type-Options':'nosniff' }); res.end(body);
  } catch { res.writeHead(404); res.end('Not found'); }
}).listen(Number(process.env.PORT||4173), process.env.HOST||'127.0.0.1', () => console.log(`AIConnect entry: http://${process.env.HOST||'127.0.0.1'}:${process.env.PORT||4173}`));
