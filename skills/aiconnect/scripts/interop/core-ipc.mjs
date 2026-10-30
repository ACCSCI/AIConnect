import { readFile } from 'node:fs/promises';
import { connect, isIP } from 'node:net';
import { join } from 'node:path';

const MAX = 1024 * 1024;
export function coreClient(directory) {
  return async function core(request) {
    const info = JSON.parse(await readFile(join(directory, 'daemon.json'), 'utf8'));
    const match = /^((?:127\.\d+\.\d+\.\d+)|(?:\[::1\])):(\d+)$/.exec(info.address);
    if (!match || !isIP(match[1].replace(/[\[\]]/g, '')) || typeof info.token !== 'string') throw new Error('unsafe_daemon_metadata');
    const payload = `${JSON.stringify({ token: info.token, request })}\n`;
    if (Buffer.byteLength(payload) > MAX) throw new Error('request_too_large');
    return new Promise((resolve, reject) => {
      const socket = connect({ host: match[1].replace(/[\[\]]/g, ''), port: Number(match[2]) });
      let bytes = 0, chunks = [];
      const fail = error => { socket.destroy(); reject(error); };
      socket.setTimeout(30000, () => fail(new Error('daemon_request_timeout')));
      socket.on('error', () => fail(new Error('daemon_unreachable')));
      socket.on('connect', () => socket.write(payload));
      socket.on('data', chunk => {
        bytes += chunk.length;
        if (bytes > MAX) return fail(new Error('response_too_large'));
        chunks.push(chunk);
        if (!chunk.includes(10)) return;
        socket.destroy();
        try {
          const response = JSON.parse(Buffer.concat(chunks).toString('utf8').split('\n')[0]);
          if (response.ok !== true) reject(new Error(response.error || 'core_request_failed'));
          else resolve(response.result);
        } catch { reject(new Error('invalid_daemon_response')); }
      });
      socket.on('end', () => { if (!socket.destroyed) fail(new Error('daemon_response_truncated')); });
    });
  };
}
