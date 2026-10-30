// HISTORICAL UNUSED EXAMPLE. Production wrangler.jsonc has no main entry point.
// The site now serves /healthz as a static JSON asset; this script is not deployed.
export default {
  async fetch(request, env) {
    const { pathname } = new URL(request.url);
    if (pathname === '/healthz') {
      const headers = { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' };
      if (!['GET', 'HEAD'].includes(request.method)) return new Response(null, { status: 405, headers: { ...headers, Allow: 'GET, HEAD' } });
      return new Response(request.method === 'HEAD' ? null : JSON.stringify({ ok: true, service: 'aiconnect-distribution', chatRelay: false, accountDatabase: false, releaseChannel: 'local-development' }), { headers });
    }
    if (pathname.startsWith('/api/')) return Response.json({ error: 'no_chat_api', message: 'AIConnect messaging runs on your local daemon, not this website.' }, { status: 404, headers: { 'Cache-Control': 'no-store' } });
    return env.ASSETS.fetch(request);
  },
};
