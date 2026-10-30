import { lookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import http from 'node:http';
import https from 'node:https';

export function publicAddress(address) {
  if (isIP(address) === 4) {
    const [a,b,c] = address.split('.').map(Number);
    return !(a === 0 || a === 10 || a === 127 || a >= 224 || (a === 100 && b >= 64 && b <= 127)
      || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31) || (a === 192 && (b === 0 || b === 168))
      || (a === 198 && (b === 18 || b === 19 || b === 51)) || (a === 203 && b === 0 && c === 113));
  }
  // Only global-unicast IPv6; reject mapped IPv4, transition/tunnel and documentation ranges.
  if (isIP(address) === 6) return /^[23][0-9a-f]{3}:/i.test(address) && !/^(2001:(0*:|db8:|2:|10:)|2002:)/i.test(address);
  return false;
}

export function safeFetchFor(origin, { token, allowLoopback = false, maxBytes = 256 * 1024, timeoutMs = 15000, resolve = lookup, beforeRequest, chargeDownload } = {}) {
  const permitted = new URL(origin);
  if (permitted.username || permitted.password || permitted.hash) throw new Error('unsafe_provider_url');
  return async (input, init = {}) => {
    const url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url);
    if (url.origin !== permitted.origin || url.username || url.password || url.hash) throw new Error('provider_origin_mismatch');
    const method = init.method || 'GET';
    if (!['GET', 'POST'].includes(method)) throw new Error('unsupported_http_method');
    const host = url.hostname.replace(/[\[\]]/g, '');
    const loopback = host === '127.0.0.1' || host === '::1';
    if (url.protocol !== 'https:' && !(allowLoopback && loopback && url.protocol === 'http:')) throw new Error('https_required');
    if (beforeRequest) await beforeRequest(url, method);
    if (chargeDownload) await chargeDownload(0);
    let dnsTimer;
    const addresses = isIP(host) ? [{ address: host, family: isIP(host) }] : await Promise.race([
      resolve(host, { all: true, verbatim: true }),
      new Promise((_,reject) => { dnsTimer = setTimeout(() => reject(new Error('dns_timeout')),5000); }),
    ]).finally(() => clearTimeout(dnsTimer));
    if (!addresses.length || addresses.some(a => !publicAddress(a.address) && !(allowLoopback && loopback && a.address === host))) throw new Error('private_address_blocked');
    const pinned = addresses[0];
    const headers = new Headers(init.headers);
    for (const h of ['host','connection','proxy-authorization','cookie']) headers.delete(h);
    if (token) headers.set('authorization', `Bearer ${token}`);
    headers.set('accept-encoding','identity');
    const body = init.body == null ? undefined : (typeof init.body === 'string' ? init.body : Buffer.from(init.body));
    if (body && Buffer.byteLength(body) > maxBytes) throw new Error('request_too_large');
    if (init.signal?.aborted) throw new Error('request_aborted');
    return new Promise((resolveResponse, reject) => {
      const req = (url.protocol === 'https:' ? https : http).request(url, {
        method, headers: Object.fromEntries(headers), agent: false,
        lookup: (_host, options, callback) => options?.all ? callback(null, [pinned]) : callback(null, pinned.address, pinned.family),
      }, async res => {
        const status = res.statusCode;
        if (status >= 300 && status < 400) { res.destroy(); reject(new Error('redirect_blocked')); return; }
        let total = 0; const chunks = [];
        try {
          // Await each accounting update with stream backpressure: no unbounded IPC queue.
          for await (const chunk of res) {
            total += chunk.length;
            if (chargeDownload) await chargeDownload(chunk.length);
            if (total > maxBytes) throw new Error('response_too_large');
            chunks.push(chunk);
          }
          resolveResponse(new Response([204,304].includes(status) ? null : Buffer.concat(chunks), {
            status, headers: Object.fromEntries(Object.entries(res.headers).filter(([,v]) => typeof v === 'string')),
          }));
        } catch (error) { res.destroy(); req.destroy(); reject(error); }
      });
      const timer = setTimeout(() => req.destroy(new Error('http_timeout')),timeoutMs);
      const abort = () => req.destroy(new Error('request_aborted'));
      init.signal?.addEventListener('abort', abort, { once: true });
      req.setTimeout(timeoutMs, () => req.destroy(new Error('http_timeout')));
      req.on('close', () => { clearTimeout(timer); init.signal?.removeEventListener('abort', abort); });
      req.on('error', reject);
      req.end(body);
    });
  };
}
