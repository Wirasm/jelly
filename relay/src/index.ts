// The jelly relay. Phase 1 of #108: read-only links.
//
// The owner's CLI publishes a snapshot (today's offline `jelly share` HTML)
// under a random token; anyone with /j/<token> can open it. The relay stores
// only SHA-256(token), so its storage and logs never hold a working link.

interface Env {
  SHARES: KVNamespace;
  RELAY_SECRET: string;
}

const TOKEN = /^[A-Za-z0-9]{22,64}$/;
const DAY = 86_400;
const MAX_TTL = 365 * DAY;

// Headers on every shared page. The CSP sandbox gives the page an opaque
// origin, so one shared jelly's script can never read another's.
const PAGE_HEADERS = {
  "Content-Type": "text/html; charset=utf-8",
  "Content-Security-Policy": "sandbox allow-scripts allow-popups allow-popups-to-escape-sandbox; frame-ancestors 'none'",
  "Referrer-Policy": "no-referrer",
  "X-Robots-Tag": "noindex, nofollow",
  "Cache-Control": "private, no-store",
  "X-Content-Type-Options": "nosniff",
};

async function key(token: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return "s:" + [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// Constant-time compare of the bearer secret.
async function authorized(req: Request, env: Env): Promise<boolean> {
  if (!env.RELAY_SECRET) return false;
  const given = req.headers.get("Authorization")?.replace(/^Bearer /, "") ?? "";
  const enc = new TextEncoder();
  const [a, b] = await Promise.all([given, env.RELAY_SECRET].map((s) => crypto.subtle.digest("SHA-256", enc.encode(s))));
  return crypto.subtle.timingSafeEqual(a, b);
}

const text = (body: string, status = 200, headers: HeadersInit = {}) =>
  new Response(body, { status, headers: { "Content-Type": "text/plain; charset=utf-8", ...headers } });

const notFound = () =>
  new Response(
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>jelly</title>
<body style="font:16px -apple-system,sans-serif;display:grid;place-items:center;height:90vh;margin:0;background:#fff8f3;color:#2a1620">
<p>This link has expired or was never shared.</p></body>`,
    { status: 404, headers: { "Content-Type": "text/html; charset=utf-8", "X-Robots-Tag": "noindex", "Cache-Control": "no-store" } },
  );

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const path = url.pathname;

    if (path === "/robots.txt") return text("User-agent: *\nDisallow: /\n");

    // Opening a share.
    const open = path.match(/^\/j\/([^/]+)\/?$/);
    if (open && (req.method === "GET" || req.method === "HEAD")) {
      if (!TOKEN.test(open[1])) return notFound();
      const html = await env.SHARES.get(await key(open[1]));
      return html === null ? notFound() : new Response(req.method === "HEAD" ? null : html, { headers: PAGE_HEADERS });
    }

    // Publishing and revoking: the owner's CLI only.
    const api = path.match(/^\/api\/shares\/([^/]+)$/);
    if (api) {
      if (!(await authorized(req, env))) return text("unauthorized\n", 401);
      if (!TOKEN.test(api[1])) return text("bad token\n", 400);
      const k = await key(api[1]);
      if (req.method === "PUT") {
        const ttl = Math.min(Number(url.searchParams.get("ttl")) || 30 * DAY, MAX_TTL);
        if (ttl < 60) return text("ttl must be at least 60 seconds\n", 400);
        await env.SHARES.put(k, await req.text(), { expirationTtl: ttl });
        return Response.json({ path: `/j/${api[1]}`, expires_at: Date.now() + ttl * 1000 });
      }
      if (req.method === "DELETE") {
        await env.SHARES.delete(k);
        return new Response(null, { status: 204 });
      }
      return text("method not allowed\n", 405);
    }

    if (path === "/") return text("A jelly relay. Shared jellies open at /j/<link>.\n", 200, { "X-Robots-Tag": "noindex" });
    return notFound();
  },
} satisfies ExportedHandler<Env>;
