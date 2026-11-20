// The jelly relay: live share links for jellies, on your own Cloudflare account.
//
// Each shared jelly is one Durable Object, named by a random share id. It holds
// the jelly (view, verbs, state, comments, asks), runs its verbs, keeps an event
// log, and pushes every change over WebSockets: to the owner's daemon, which
// mirrors it into the local store, and to everyone who has the jelly open.
//
// A link is /j/<share id><secret>, 22 + 22 random characters. The object stores
// only SHA-256(secret) with whose it is ("ana@example.com", or "" for the
// view-only link), so its storage never holds a working link. Only the holder of
// RELAY_SECRET, the owner's daemon, can publish, reshape, mint or revoke links.

import { DurableObject } from "cloudflare:workers";
import { engine, run, Refusal } from "./verbs";
import { shell } from "./shell";

interface Env {
  LIVE: DurableObjectNamespace<Live>;
  RELAY_SECRET: string;
}

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };
interface Comment { id: string; anchor: Json; text: string; by: string; at_ms: number; resolved: boolean; replies: { text: string; by: string; at_ms: number }[] }
interface Ask { id: string; seen?: boolean; title: string; options?: { id: string }[]; group?: string | null; answer?: Json; withdrawn?: Json; [k: string]: Json | undefined }
interface Doc { name: string; about: string; view: string; verbs: string; state: Json; version: number; view_rev: number; comments: Comment[]; attention: Ask[] }
interface Ev { seq: number; at_ms: number; kind: string; by: string; verb?: string; args?: Json; source?: Json; version: number; view_rev: number }
type Op = { op: string; by?: string; [k: string]: any };
/** Who a socket belongs to: the owner's daemon, or a link. */
interface Seat { owner: boolean; who: string; can_act: boolean }

const SHARE = "[A-Za-z0-9]{22}";
const LINK = new RegExp(`^/j/(${SHARE})([A-Za-z0-9]{22})(/ws)?/?$`);
const ASSET = new RegExp(`^/a/(${SHARE})/assets/([^/]+)$`);
const OWNER = new RegExp(`^/api/live/(${SHARE})(/.*)?$`);
/** What a link that may act can do. Everything else is the owner's. */
const PERSON_OPS = ["call", "comment", "reply", "resolve", "answer"];
/** SQLite rows hold at most 2 MB, so assets are kept in parts. */
const PART = 1 << 20;
const MAX_DAYS = 365;

const HEADERS = {
  "Referrer-Policy": "no-referrer",
  "X-Robots-Tag": "noindex, nofollow",
  "Cache-Control": "private, no-store",
  "X-Content-Type-Options": "nosniff",
};

const text = (body: string, status = 200) => new Response(body, { status, headers: { "Content-Type": "text/plain; charset=utf-8", ...HEADERS } });
const notFound = () =>
  new Response(
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>jelly</title>
<body style="font:16px -apple-system,sans-serif;display:grid;place-items:center;height:90vh;margin:0;background:#fff8f3;color:#2a1620">
<p>This link has expired or was never shared.</p></body>`,
    { status: 404, headers: { "Content-Type": "text/html; charset=utf-8", ...HEADERS } },
  );

async function sha256(s: string): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// Constant-time compare of the bearer secret.
async function authorized(req: Request, env: Env): Promise<boolean> {
  if (!env.RELAY_SECRET) return false;
  const given = req.headers.get("Authorization")?.replace(/^Bearer /, "") ?? "";
  const enc = new TextEncoder();
  const [a, b] = await Promise.all([given, env.RELAY_SECRET].map((s) => crypto.subtle.digest("SHA-256", enc.encode(s))));
  return crypto.subtle.timingSafeEqual(a, b);
}

/** `src/store.rs` set_at: put `value` at a JSON pointer, or remove what is there. */
function setAt(root: Json, pointer: string, value: Json | undefined): Json {
  if (pointer === "") return value === undefined ? {} : value;
  const cut = pointer.lastIndexOf("/");
  if (cut < 0) throw new Refusal(`${JSON.stringify(pointer)} is not a JSON pointer; start it with /`);
  const parentPath = pointer.slice(0, cut);
  const key = pointer.slice(cut + 1).replaceAll("~1", "/").replaceAll("~0", "~");
  let parent: Json = root;
  for (const raw of parentPath.split("/").slice(1)) {
    const k = raw.replaceAll("~1", "/").replaceAll("~0", "~");
    parent = Array.isArray(parent) ? (/^(0|[1-9]\d*)$/.test(k) ? parent[Number(k)] : undefined) as Json
      : parent && typeof parent === "object" && Object.hasOwn(parent, k) ? parent[k] : (undefined as unknown as Json);
    if (parent === undefined) throw new Refusal(`nothing at ${parentPath || "/"}`);
  }
  if (Array.isArray(parent)) {
    const i = key === "-" ? parent.length : /^\d+$/.test(key) ? Number(key) : NaN;
    if (Number.isNaN(i)) throw new Refusal(`${JSON.stringify(key)} is not an array index`);
    if (value !== undefined && i <= parent.length) parent[i] = value;
    else if (value === undefined && i < parent.length) parent.splice(i, 1);
    else throw new Refusal(`${pointer} is past the end of the array`);
  } else if (parent && typeof parent === "object") {
    if (value !== undefined) parent[key] = value;
    else if (Object.hasOwn(parent, key)) delete parent[key];
    else throw new Refusal(`nothing at ${pointer}`);
  } else throw new Refusal(`${parentPath} is neither an object nor an array`);
  return root;
}

export class Live extends DurableObject<Env> {
  private doc: Doc | undefined;
  private seq = 0;
  private share = "";

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    // Tables are made when the owner publishes, so a request for an unknown
    // share id does not leave an object with storage behind.
    const sql = ctx.storage.sql;
    if (!sql.exec("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kv'").toArray().length) return;
    const doc = this.get("doc");
    this.doc = doc ? JSON.parse(doc) : undefined;
    this.share = this.get("share") ?? "";
    this.seq = sql.exec<{ s: number | null }>("SELECT MAX(seq) AS s FROM events").one().s ?? 0;
  }

  private get(k: string): string | undefined {
    return this.ctx.storage.sql.exec<{ v: string }>("SELECT v FROM kv WHERE k = ?", k).toArray()[0]?.v;
  }
  private put(k: string, v: string) {
    this.ctx.storage.sql.exec("INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v", k, v);
  }

  /** The parts of the jelly a message carries. The view and verbs go only with
   *  a reshape, and verbs only to the owner: a link has no use for them. */
  private part(d: Doc, kind: string, owner: boolean): Partial<Doc> {
    const { view, verbs, about, ...rest } = d;
    const reshaped = kind === "reshape" || kind === "all";
    return { ...rest, ...(reshaped && { view, about }), ...(reshaped && owner && { verbs }) };
  }

  private seat(ws: WebSocket): Seat {
    return ws.deserializeAttachment() as Seat;
  }

  private since(seq: number) {
    const events = this.ctx.storage.sql.exec<{ ev: string }>("SELECT ev FROM events WHERE seq > ? ORDER BY seq", seq).toArray().map((r) => JSON.parse(r.ev));
    return { type: "events", events, doc: this.part(this.doc!, "all", true) };
  }

  /** Applies one change and tells everyone. No await anywhere in here: a Durable
   *  Object lets other requests in at every await, so reading the jelly, running
   *  the verb and saving the result must happen in one synchronous stretch. That
   *  is what makes verbs run one at a time, as the daemon's store lock does. */
  private apply(mod: Awaited<ReturnType<typeof engine>>, op: Op, by: string): { events: Ev[]; doc: Partial<Doc> } {
    const d = this.doc!;
    const now = Date.now();
    let args: Json | undefined, verb: string | undefined, source: Json | undefined;
    let bump = false;
    const ask = (id: string) => d.attention.find((a) => a.id === id) ?? (() => { throw new Refusal(`${d.name} has no attention item ${id}`); })();
    const comment = (id: string) => d.comments.find((c) => c.id === id) ?? (() => { throw new Refusal(`${d.name} has no comment ${id}`); })();
    switch (op.op) {
      case "call":
        d.state = run(mod, d.verbs, op.verb, d.state, op.args, by, now) as Json;
        verb = op.verb; args = op.args ?? null; source = op.source ?? undefined; bump = true;
        break;
      case "set":
        d.state = setAt(structuredClone(d.state), op.pointer ?? "", op.remove ? undefined : op.value);
        args = { pointer: op.pointer ?? "" }; bump = true;
        break;
      case "reshape": {
        if (op.view == null && op.verbs == null && op.about == null) throw new Refusal("reshape needs a new view, new verbs or a new about");
        if (op.about != null) d.about = String(op.about).trim();
        if (op.view != null || op.verbs != null) {
          d.view = op.view ?? d.view; d.verbs = op.verbs ?? d.verbs;
          d.view_rev++; bump = true;
        }
        break;
      }
      case "attend": {
        const item: Ask = { ...op.item, id: `a${d.attention.length + 1}` };
        d.attention.push(item);
        args = item as Json;
        break;
      }
      case "withdraw":
        ask(op.id).withdrawn = { by, at_ms: now };
        args = op.id;
        break;
      case "answer": {
        const item = ask(op.id);
        if (item.withdrawn) throw new Refusal(`${op.id} was withdrawn`);
        const option: string | null = op.option ?? null;
        if (option !== null && !item.options?.some((o) => o.id === option)) throw new Refusal(`${JSON.stringify(option)} is not an option of ${op.id}`);
        const t = String(op.text ?? "").trim();
        if (!item.seen && option === null && !t) throw new Refusal("this ask needs an option or some text");
        item.answer = { option, text: t, by, at_ms: now };
        args = { id: op.id, seen: !!item.seen, title: item.title, option, text: t, group: item.group ?? null };
        break;
      }
      case "comment": {
        const t = String(op.text ?? "").trim();
        if (!t) throw new Refusal("empty comment");
        const id = `c${d.comments.length + 1}`;
        d.comments.push({ id, anchor: op.anchor ?? null, text: t, by, at_ms: now, resolved: false, replies: [] });
        args = { id, anchor: op.anchor ?? null, text: t };
        break;
      }
      case "reply": {
        const t = String(op.text ?? "").trim();
        if (!t) throw new Refusal("empty reply");
        const c = comment(op.id);
        c.replies.push({ text: t, by, at_ms: now });
        c.resolved = op.resolve === true;
        args = { id: op.id, text: t, resolved: c.resolved, anchor: c.anchor, comment: c.text };
        break;
      }
      case "resolve":
        comment(op.id).resolved = true;
        args = op.id;
        break;
      default:
        throw new Refusal(`no such change: ${op.op}`);
    }
    if (bump) d.version++;
    const ev: Ev = { seq: ++this.seq, at_ms: now, kind: op.op === "attend" ? "attention" : op.op, by, version: d.version, view_rev: d.view_rev };
    if (verb !== undefined) ev.verb = verb;
    if (args !== undefined) ev.args = args;
    if (source !== undefined) ev.source = source;
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec("INSERT INTO events (seq, ev) VALUES (?, ?)", ev.seq, JSON.stringify(ev));
      this.put("doc", JSON.stringify(d));
    });
    for (const ws of this.ctx.getWebSockets()) {
      try { ws.send(JSON.stringify({ type: "events", events: [ev], doc: this.part(d, ev.kind, this.seat(ws).owner) })); } catch { /* closing */ }
    }
    return { events: [ev], doc: this.part(d, ev.kind, true) };
  }

  /** Runs `op` from a link or the owner: everything that can fail before the
   *  change is awaited first, then the change itself runs without a break. */
  private async change(op: Op, by: string) {
    const mod = await engine();
    if (!this.doc) throw new Refusal("this jelly is no longer shared");
    try {
      return this.apply(mod, op, by);
    } catch (e) {
      // Refusals come before any change; a failed save (a jelly over 2 MB) does not.
      this.doc = JSON.parse(this.get("doc")!);
      this.seq = this.ctx.storage.sql.exec<{ s: number | null }>("SELECT MAX(seq) AS s FROM events").one().s ?? 0;
      throw e;
    }
  }

  /** Ends the share: everyone's socket closes and the storage is gone. */
  private async end(code: number, reason: string) {
    for (const ws of this.ctx.getWebSockets()) ws.close(code, reason);
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
    this.doc = undefined;
    this.seq = 0;
  }

  async alarm() {
    await this.end(4404, "expired");
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const [, share, kind, rest] = url.pathname.match(/^\/([^/]+)\/(owner|link|assets)\/?(.*)$/) ?? [];
    if (kind === "owner") return this.owner(req, share, rest, url);
    if (kind === "assets") return this.doc ? this.asset(decodeURIComponent(rest)) : notFound();
    const [secret, ws] = rest.split("/");
    const row = this.doc && this.ctx.storage.sql.exec<{ who: string; can_act: number }>("SELECT who, can_act FROM people WHERE hash = ?", await sha256(secret)).toArray()[0];
    if (ws !== "ws") return row ? new Response(shell(this.doc?.about ?? ""), { headers: { "Content-Type": "text/html; charset=utf-8", "Content-Security-Policy": "frame-ancestors 'none'", ...HEADERS } }) : notFound();
    if (req.headers.get("Upgrade") !== "websocket") return text("expected a WebSocket\n", 426);
    const [client, server] = Object.values(new WebSocketPair());
    if (!row || !this.doc) {
      // Accept, then close at once with a code the page understands, so it stops retrying.
      server.accept();
      server.close(this.doc ? 4403 : 4404, this.doc ? "revoked" : "unshared");
      return new Response(null, { status: 101, webSocket: client });
    }
    const seat: Seat = { owner: false, who: row.who, can_act: row.can_act === 1 };
    this.ctx.acceptWebSocket(server, [`p:${row.who}`]);
    server.serializeAttachment(seat);
    const me = { by: seat.can_act ? `person:${row.who}` : "", who: row.who, can_act: seat.can_act };
    server.send(JSON.stringify({ type: "hello", doc: this.part(this.doc, "all", false), runtime: this.get("runtime"), me, assets: `/a/${this.share}/` }));
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer) {
    const seat = this.seat(ws);
    if (seat.owner || typeof message !== "string") return;
    let msg: { ref?: number; op?: string; body?: Record<string, unknown> };
    try { msg = JSON.parse(message); } catch { return; }
    const reply = (ok: boolean, value?: unknown, error?: string) => ws.send(JSON.stringify({ type: "reply", ref: msg.ref, ok, value, error }));
    if (!seat.can_act) return reply(false, undefined, "This link is view only.");
    if (!msg.op || !PERSON_OPS.includes(msg.op)) return reply(false, undefined, `a link cannot ${msg.op}`);
    try {
      const { events, doc } = await this.change({ ...msg.body, op: msg.op }, `person:${seat.who}`);
      reply(true, { ...events[0], state: doc.state });
    } catch (e) {
      reply(false, undefined, e instanceof Refusal ? e.message : String(e));
    }
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string) {
    try { ws.close(code, reason); } catch { /* already closed */ }
  }

  private asset(file: string): Response {
    const parts = this.ctx.storage.sql.exec<{ type: string; body: ArrayBuffer }>("SELECT type, body FROM assets WHERE file = ? ORDER BY part", file).toArray();
    if (!parts.length) return new Response("not found", { status: 404, headers: HEADERS });
    // Opaque-origin views fetch assets with CORS; a script in an SVG must not run on this origin.
    return new Response(new Blob(parts.map((p) => p.body)), {
      headers: { "Content-Type": parts[0].type, "Access-Control-Allow-Origin": "*", "Content-Security-Policy": "sandbox", ...HEADERS },
    });
  }

  private async owner(req: Request, share: string, path: string, url: URL): Promise<Response> {
    const sql = this.ctx.storage.sql;
    const json = <T>() => req.json<T>();
    try {
      // Publish (or republish) the jelly, with the runtime its view runs on.
      if (path === "" && req.method === "PUT") {
        const { doc, runtime } = await json<{ doc: Doc; runtime: string }>();
        sql.exec(`CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY, ev TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS people (hash TEXT PRIMARY KEY, who TEXT NOT NULL, can_act INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS assets (file TEXT NOT NULL, part INTEGER NOT NULL, type TEXT NOT NULL, body BLOB NOT NULL, PRIMARY KEY (file, part));`);
        this.doc = doc;
        this.share = share;
        this.put("doc", JSON.stringify(doc));
        this.put("runtime", runtime);
        this.put("share", share);
        return Response.json({ seq: this.seq });
      }
      if (!this.doc) return text("no such share\n", 404);
      if (path === "" && req.method === "DELETE") {
        const last = this.since(Number(url.searchParams.get("since")) || 0);
        await this.end(4404, "unshared");
        return Response.json(last);
      }
      if (path === "people" && req.method === "POST") {
        const p = await json<{ secret: string; who: string; can_act: boolean; days: number }>();
        if (!/^[A-Za-z0-9]{22}$/.test(p.secret)) return text("bad secret\n", 400);
        sql.exec("INSERT INTO people (hash, who, can_act) VALUES (?, ?, ?) ON CONFLICT(hash) DO UPDATE SET who = excluded.who, can_act = excluded.can_act",
          await sha256(p.secret), p.who, p.can_act ? 1 : 0);
        // A share lasts as long as its longest link.
        const until = Date.now() + Math.min(Math.max(p.days, 1), MAX_DAYS) * 86_400_000;
        const current = await this.ctx.storage.getAlarm();
        if (!current || current < until) await this.ctx.storage.setAlarm(until);
        return Response.json({ expires_at: Math.max(current ?? 0, until) });
      }
      if (path === "people" && req.method === "DELETE") {
        const who = url.searchParams.get("who") ?? "";
        sql.exec("DELETE FROM people WHERE who = ?", who);
        for (const ws of this.ctx.getWebSockets(`p:${who}`)) ws.close(4403, "revoked");
        return new Response(null, { status: 204 });
      }
      if (path === "op" && req.method === "POST") {
        const op = await json<Op>();
        if (!op.by) return text("say who is making this change: by\n", 400);
        return Response.json(await this.change(op, op.by));
      }
      if (path === "ws") {
        if (req.headers.get("Upgrade") !== "websocket") return text("expected a WebSocket\n", 426);
        const [client, server] = Object.values(new WebSocketPair());
        this.ctx.acceptWebSocket(server, ["owner"]);
        server.serializeAttachment({ owner: true, who: "", can_act: true } satisfies Seat);
        // Catch up from where the daemon left off, then every change as it happens.
        server.send(JSON.stringify(this.since(Number(url.searchParams.get("since")) || 0)));
        return new Response(null, { status: 101, webSocket: client });
      }
      const asset = path.match(/^assets\/([^/]+)$/);
      if (asset && req.method === "PUT") {
        const file = decodeURIComponent(asset[1]);
        const body = await req.arrayBuffer();
        sql.exec("DELETE FROM assets WHERE file = ?", file);
        for (let at = 0, part = 0; at < body.byteLength || part === 0; at += PART, part++) {
          sql.exec("INSERT INTO assets (file, part, type, body) VALUES (?, ?, ?, ?)", file, part, req.headers.get("Content-Type") ?? "application/octet-stream", body.slice(at, at + PART));
        }
        return new Response(null, { status: 204 });
      }
      if (asset && req.method === "DELETE") {
        sql.exec("DELETE FROM assets WHERE file = ?", decodeURIComponent(asset[1]));
        return new Response(null, { status: 204 });
      }
      return text("not found\n", 404);
    } catch (e) {
      if (e instanceof Refusal) return Response.json({ error: e.message }, { status: 400 });
      throw e;
    }
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const path = url.pathname;
    const live = (share: string, rest: string) => env.LIVE.getByName(share).fetch(new Request(`https://live/${share}/${rest}${url.search}`, req));

    if (path === "/robots.txt") return text("User-agent: *\nDisallow: /\n");

    const link = path.match(LINK);
    if (link && req.method === "GET") return live(link[1], `link/${link[2]}${link[3] ?? ""}`);

    const asset = path.match(ASSET);
    if (asset && req.method === "GET") return live(asset[1], `assets/${asset[2]}`);

    const owner = path.match(OWNER);
    if (owner) {
      if (!(await authorized(req, env))) return text("unauthorized\n", 401);
      return live(owner[1], `owner${owner[2] ?? ""}`);
    }

    if (path === "/") return text("A jelly relay. Shared jellies open at /j/<link>.\n");
    return notFound();
  },
} satisfies ExportedHandler<Env>;
