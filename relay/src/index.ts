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
// RELAY_SECRET, the owner's daemon, can publish, shape, mint or revoke links.
//
// A PIN room: the owner makes a 6-digit PIN, and anyone who types it at /pin
// with their email gets their own link, like one minted with `--to`. The owner
// can put more jellies in the room while the PIN works; each link page in the
// room lists them, and a person moves to another with their own link (POST
// /room), never typing the PIN again. One Pins object per relay holds the one
// live PIN and its room. It stores only HMAC(RELAY_SECRET, PIN), throttles
// wrong tries per IP, and locks the PIN after a few wrong tries.
//
// The relay counts its own use (counts.ts): shares, opens and people per share,
// by day. GET /api/counts returns them to the holder of RELAY_SECRET.
//
// A view is HTML someone else wrote, shown under this relay's host. With
// SHARE_ORIGIN set, the pages people open (/j, /pin, /room, /a) are served only from
// that origin, so a bad page cannot get the host the daemon publishes to
// flagged. REPORT_URL puts a Report link on every link page, and
// DELETE /api/live/<share id> takes a share down for good.

import { DurableObject } from "cloudflare:workers";
import { engine, run, Refusal } from "./verbs";
import { shell, MADE, MADE_CSS } from "./shell";
import { joinPage } from "./join";
import * as counting from "./counts";

interface Env {
  LIVE: DurableObjectNamespace<Live>;
  PINS: DurableObjectNamespace<Pins>;
  RELAY_SECRET: string;
  /** Where shared pages are served from, like "https://pages.example". Unset: this relay's own host. */
  SHARE_ORIGIN?: string;
  /** Where a person reports a shared page. The share id is added to its end. Unset: no Report link. */
  REPORT_URL?: string;
}

/** The origin links are made on, when it is not the one the daemon publishes to. */
const shareOrigin = (env: Env) => (env.SHARE_ORIGIN ? new URL(env.SHARE_ORIGIN).origin : undefined);

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };
interface Comment { id: string; anchor: Json; text: string; by: string; at_ms: number; resolved: boolean; replies: { text: string; by: string; at_ms: number }[] }
interface Ask { id: string; seen?: boolean; title: string; options?: { id: string }[]; group?: string | null; answer?: Json; unasked?: Json; [k: string]: Json | undefined }
interface Doc { name: string; about: string; view: string; verbs: string; state: Json; version: number; view_rev: number; comments: Comment[]; asks: Ask[]; unsent?: number }
interface Ev { seq: number; at_ms: number; kind: string; by: string; verb?: string; args?: Json; source?: Json; quiet?: true; version: number; view_rev: number }
type Op = { op: string; by?: string; [k: string]: any };
/** A jelly in the PIN's room: its share, its name, and what its button says on a link page. */
interface InRoom { share: string; name: string; label: string }
/** Who a socket belongs to: the owner's daemon, or a link. */
interface Seat { owner: boolean; who: string; can_act: boolean }

const SHARE = "[A-Za-z0-9]{22}";
const LINK = new RegExp(`^/j/(${SHARE})([A-Za-z0-9]{22})(/ws)?/?$`);
const ASSET = new RegExp(`^/a/(${SHARE})/assets/([^/]+)$`);
const OWNER = new RegExp(`^/api/live/(${SHARE})(/.*)?$`);
/** What a link that may act can do. Everything else is the owner's. */
const PERSON_OPS = ["do", "comment", "reply", "resolve", "answer", "send"];
/** Assets are kept in parts, so no row comes near SQLite's 8 MB row limit. */
const PART = 1 << 20;
/** The jelly is one row, so links may fill only part of it: a change from a
 *  link that would take the saved jelly past this is refused, and the rest of
 *  the row stays for the owner's daemon. */
const LINK_FULL = 4 << 20;
/** The most a link sends in one change, in bytes (a verb's args, a comment's
 *  anchor; the recipes send a few ids and a note), and the longest comment,
 *  reply or answer it can write, in characters. The owner is not limited. */
const LINK_MESSAGE = 64 * 1024;
const LINK_TEXT = 10_000;
const MAX_DAYS = 365;
const MAX_HOURS = 24;
/** Wrong tries, by anyone, before every live PIN locks. With one PIN live, a
 *  guesser gets in before the lock with a chance of 10 in a million. */
const PIN_TRIES = 10;
/** Wrong tries from one IP per window. Fewer than PIN_TRIES, so one person or
 *  one script cannot lock a room alone. Right PINs never count. */
const IP_TRIES = 5;
const IP_WINDOW = 10 * 60_000;

const HEADERS = {
  "Referrer-Policy": "no-referrer",
  "X-Robots-Tag": "noindex, nofollow",
  "Cache-Control": "private, no-store",
  "X-Content-Type-Options": "nosniff",
};

const text = (body: string, status = 200) => new Response(body, { status, headers: { "Content-Type": "text/plain; charset=utf-8", ...HEADERS } });
const notFound = () =>
  new Response(
    `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>jelly</title><style>${MADE_CSS}</style>
<body style="font:16px -apple-system,sans-serif;display:grid;place-items:center;height:90vh;margin:0;background:#fff8f3;color:#2a1620;text-align:center">
<div><p>This link has expired or was never shared.</p>${MADE}</div></body>`,
    { status: 404, headers: { "Content-Type": "text/html; charset=utf-8", ...HEADERS } },
  );

/** Bytes in `s` as UTF-8, as SQLite stores it. */
const size = (s: string) => new TextEncoder().encode(s).length;

const hex = (d: ArrayBuffer) => [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");

async function sha256(s: string): Promise<string> {
  return hex(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)));
}

/** A uniformly random integer below `n`. */
function below(n: number): number {
  const top = Math.floor(2 ** 32 / n) * n;
  for (;;) {
    const [x] = crypto.getRandomValues(new Uint32Array(1));
    if (x < top) return x % n;
  }
}

/** A link secret like the daemon's (`relay::token`): `len` of [A-Za-z0-9]. */
function token(len: number): string {
  const abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  return Array.from({ length: len }, () => abc[below(62)]).join("");
}

const bearer = (req: Request) => req.headers.get("Authorization")?.replace(/^Bearer /, "") ?? "";

// Constant-time compare of the bearer secret.
async function authorized(req: Request, env: Env): Promise<boolean> {
  if (!env.RELAY_SECRET) return false;
  const given = bearer(req);
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
    this.columns();
    const doc = this.get("doc");
    this.doc = doc ? JSON.parse(doc) : undefined;
    this.share = this.get("share") ?? "";
    this.seq = sql.exec<{ s: number | null }>("SELECT MAX(seq) AS s FROM events").one().s ?? 0;
  }

  /** Columns added after the first relay: shares made before them get them here.
   *  `opened_ms` came on 21 November 2026, and a share lasts at most 365 days
   *  unless the daemon extends it, which wakes it and runs this, so by
   *  22 November 2027 no share can lack it and this can go. */
  private columns() {
    const sql = this.ctx.storage.sql;
    if (!sql.exec("SELECT 1 FROM pragma_table_info('people') WHERE name = 'opened_ms'").toArray().length) {
      sql.exec("ALTER TABLE people ADD COLUMN opened_ms INTEGER");
    }
  }

  private get(k: string): string | undefined {
    return this.ctx.storage.sql.exec<{ v: string }>("SELECT v FROM kv WHERE k = ?", k).toArray()[0]?.v;
  }
  private put(k: string, v: string) {
    this.ctx.storage.sql.exec("INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v", k, v);
  }

  /** The parts of the jelly a message carries. The view and verbs go only with
   *  a shape, and verbs only to the owner: a link has no use for them. */
  private part(d: Doc, kind: string, owner: boolean): Partial<Doc> {
    const { view, verbs, about, ...rest } = d;
    const shaped = kind === "shape" || kind === "all";
    return { ...rest, ...(shaped && { view, about }), ...(shaped && owner && { verbs }) };
  }

  private seat(ws: WebSocket): Seat {
    return ws.deserializeAttachment() as Seat;
  }

  /** How many people are connected now, for the counts. Someone with two pages
   *  open is one person; the view-only link has no names, so each page is one.
   *  The owner's daemon is not a person. */
  private people(): number {
    const seats = this.ctx.getWebSockets().map((ws) => this.seat(ws)).filter((s) => !s.owner);
    return new Set(seats.filter((s) => s.who).map((s) => s.who)).size + seats.filter((s) => !s.who).length;
  }

  private since(seq: number) {
    const events = this.ctx.storage.sql.exec<{ ev: string }>("SELECT ev FROM events WHERE seq > ? ORDER BY seq", seq).toArray().map((r) => JSON.parse(r.ev));
    return { type: "events", events, doc: this.part(this.doc!, "all", true) };
  }

  /** Applies one change and tells everyone. No await anywhere in here: a Durable
   *  Object lets other requests in at every await, so reading the jelly, running
   *  the verb and saving the result must happen in one synchronous stretch. That
   *  is what makes verbs run one at a time, as the daemon's store lock does. */
  private apply(mod: Awaited<ReturnType<typeof engine>>, op: Op, by: string, full: number): { events: Ev[]; doc: Partial<Doc> } {
    const d = this.doc!;
    const now = Date.now();
    let args: Json | undefined, verb: string | undefined, source: Json | undefined;
    let bump = false;
    const ask = (id: string) => d.asks.find((a) => a.id === id) ?? (() => { throw new Refusal(`${d.name} has no ask ${id}`); })();
    const comment = (id: string) => d.comments.find((c) => c.id === id) ?? (() => { throw new Refusal(`${d.name} has no comment ${id}`); })();
    switch (op.op) {
      case "do":
        d.state = run(mod, d.verbs, op.verb, d.state, op.args, by, now) as Json;
        verb = op.verb; args = op.args ?? null; source = op.source ?? undefined; bump = true;
        break;
      case "set":
        d.state = setAt(structuredClone(d.state), op.pointer ?? "", op.remove ? undefined : op.value);
        args = { pointer: op.pointer ?? "" }; bump = true;
        break;
      case "shape": {
        if (op.view == null && op.verbs == null && op.about == null) throw new Refusal("shape needs a new view, new verbs or a new about");
        if (op.about != null) d.about = String(op.about).trim();
        if (op.view != null || op.verbs != null) {
          d.view = op.view ?? d.view; d.verbs = op.verbs ?? d.verbs;
          d.view_rev++; bump = true;
        }
        break;
      }
      case "ask": {
        const item: Ask = { ...op.item, id: `a${d.asks.length + 1}` };
        d.asks.push(item);
        args = item as Json;
        break;
      }
      case "unask":
        ask(op.id).unasked = { by, at_ms: now };
        args = op.id;
        break;
      case "answer": {
        const item = ask(op.id);
        if (item.unasked) throw new Refusal(`${op.id} was unasked`);
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
      case "send":
        args = d.unsent ?? 0;
        break;
      case "lock":
        args = { tries: op.tries };
        break;
      default:
        throw new Refusal(`no such change: ${op.op}`);
    }
    if (bump) d.version++;
    // What people did since they last pressed Send, as the daemon counts it.
    // A quiet verb call (`jelly.do(verb, args, { quiet: true })`) does not count.
    const quiet = op.op === "do" && op.quiet === true;
    if (op.op === "send") d.unsent = 0;
    else if (by.startsWith("person") && PERSON_OPS.includes(op.op) && !quiet) d.unsent = (d.unsent ?? 0) + 1;
    const ev: Ev = { seq: ++this.seq, at_ms: now, kind: op.op, by, version: d.version, view_rev: d.view_rev };
    if (verb !== undefined) ev.verb = verb;
    if (args !== undefined) ev.args = args;
    if (source !== undefined) ev.source = source;
    if (quiet) ev.quiet = true;
    const saved = JSON.stringify(d);
    if (full !== Infinity && size(saved) > full && size(saved) > size(this.get("doc")!)) {
      throw new Refusal("This jelly is full, so it takes nothing more from links. Tell whoever shared it.");
    }
    this.ctx.storage.transactionSync(() => {
      this.ctx.storage.sql.exec("INSERT INTO events (seq, ev) VALUES (?, ?)", ev.seq, JSON.stringify(ev));
      this.put("doc", saved);
    });
    for (const ws of this.ctx.getWebSockets()) {
      try { ws.send(JSON.stringify({ type: "events", events: [ev], doc: this.part(d, ev.kind, this.seat(ws).owner) })); } catch { /* closing */ }
    }
    return { events: [ev], doc: this.part(d, ev.kind, true) };
  }

  /** Runs `op` from a link or the owner: everything that can fail before the
   *  change is awaited first, then the change itself runs without a break.
   *  `full` is the most the saved jelly may grow to: LINK_FULL for a link. */
  private async change(op: Op, by: string, full = Infinity) {
    const mod = await engine();
    if (!this.doc) throw new Refusal("this jelly is no longer shared");
    try {
      return this.apply(mod, op, by, full);
    } catch (e) {
      // A verb, a full jelly or a failed save can refuse after the jelly in memory changed.
      this.doc = JSON.parse(this.get("doc")!);
      this.seq = this.ctx.storage.sql.exec<{ s: number | null }>("SELECT MAX(seq) AS s FROM events").one().s ?? 0;
      throw e;
    }
  }

  /** Ends the share: everyone's socket closes and the storage is gone. It is
   *  over before the first await, so a change that arrives meanwhile is
   *  refused instead of landing and then being deleted unheard. */
  private async end(code: number, reason: string) {
    this.doc = undefined;
    this.seq = 0;
    for (const ws of this.ctx.getWebSockets()) ws.close(code, reason);
    // Deletes the alarm too (compatibility date 2026-02-24 and later).
    await this.ctx.storage.deleteAll();
    // The share is gone whatever the PIN's room says. When leaving fails, the
    // room keeps listing it until the PIN ends: a page in the room still shows
    // its button, and following it or a PIN landing on it says it no longer works.
    try { await this.env.PINS.getByName("pins").leave(this.share); } catch { /* as above */ }
  }

  async alarm() {
    await this.end(4404, "expired");
  }

  /** Lets the holder of a link in as `who`. Both `--to` and the PIN mint through here. */
  private admit(hash: string, who: string, canAct: boolean) {
    this.ctx.storage.sql.exec("INSERT INTO people (hash, who, can_act) VALUES (?, ?, ?) ON CONFLICT(hash) DO UPDATE SET who = excluded.who, can_act = excluded.can_act",
      hash, who, canAct ? 1 : 0);
  }

  /** Makes the share last at least `days` from now; returns when it expires. */
  private async extend(days: number): Promise<number> {
    const until = Date.now() + Math.min(Math.max(days, 1), MAX_DAYS) * 86_400_000;
    const current = await this.ctx.storage.getAlarm();
    if (!current || current < until) await this.ctx.storage.setAlarm(until);
    return Math.max(current ?? 0, until);
  }

  /** Someone typed this share's PIN with their email: their own link, returned
   *  as its secret. The same email again (another device) is the same person
   *  with one more link; revoking that person stops them all. */
  async join(email: string): Promise<string | null> {
    const secret = token(22);
    const hash = await sha256(secret);
    if (!this.doc) return null;
    this.admit(hash, email, true);
    return secret;
  }

  /** Who holds this link, when it may act; null for a view-only link or one that does not work. */
  async who(secret: string): Promise<string | null> {
    const hash = await sha256(secret);
    if (!this.doc) return null;
    const row = this.ctx.storage.sql.exec<{ who: string; can_act: number }>("SELECT who, can_act FROM people WHERE hash = ?", hash).toArray()[0];
    return row?.can_act ? row.who : null;
  }

  /** The PIN's room as it is now (empty when it ended), kept for pages that
   *  connect later and sent to the pages open now that may act. */
  async room(room: InRoom[], until: number) {
    if (!this.doc) return;
    this.put("room", JSON.stringify({ room, until }));
    for (const ws of this.ctx.getWebSockets()) {
      const seat = this.seat(ws);
      if (!seat.owner && seat.can_act) try { ws.send(JSON.stringify({ type: "room", room })); } catch { /* closing */ }
    }
  }

  /** The PIN locked: an event, so the owner hears it in the inbox. The room is over. */
  async locked(tries: number) {
    try { await this.change({ op: "lock", tries }, "relay"); } catch { /* unshared meanwhile */ }
    await this.room([], 0);
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const [, share, kind, rest] = url.pathname.match(/^\/([^/]+)\/(owner|link|assets)\/?(.*)$/) ?? [];
    if (kind === "owner") return this.owner(req, share, rest, url);
    if (kind === "assets") return this.doc ? this.asset(decodeURIComponent(rest)) : notFound();
    const [secret, ws] = rest.split("/");
    const hash = await sha256(secret);
    const row = this.doc && this.ctx.storage.sql.exec<{ who: string; can_act: number; opened_ms: number | null }>("SELECT who, can_act, opened_ms FROM people WHERE hash = ?", hash).toArray()[0];
    const report = this.env.REPORT_URL ? this.env.REPORT_URL + this.share : undefined;
    if (ws !== "ws") return row ? new Response(shell(this.doc?.about ?? "", report), { headers: { "Content-Type": "text/html; charset=utf-8", "Content-Security-Policy": "frame-ancestors 'none'", ...HEADERS } }) : notFound();
    if (req.headers.get("Upgrade") !== "websocket") return text("expected a WebSocket\n", 426);
    const [client, server] = Object.values(new WebSocketPair());
    if (!row || !this.doc) {
      // Accept, then close at once with a code the page understands, so it stops retrying.
      server.accept();
      server.close(this.doc ? 4403 : 4404, this.doc ? "revoked" : "unshared");
      return new Response(null, { status: 101, webSocket: client });
    }
    // The owner sees who has opened their link, and when they last did.
    this.ctx.storage.sql.exec("UPDATE people SET opened_ms = ? WHERE hash = ?", Date.now(), hash);
    const seat: Seat = { owner: false, who: row.who, can_act: row.can_act === 1 };
    this.ctx.acceptWebSocket(server, [`p:${row.who}`]);
    server.serializeAttachment(seat);
    // Counted beside the open, not before it: a link works whether or not the count lands.
    const first = !row.opened_ms || counting.today(row.opened_ms) !== counting.today();
    this.ctx.waitUntil(this.env.PINS.getByName("pins").opened(this.share, this.people(), first));
    const me = { by: seat.can_act ? `person:${row.who}` : "", who: row.who, can_act: seat.can_act };
    const kept: { room: InRoom[]; until: number } | undefined = JSON.parse(this.get("room") ?? "null") ?? undefined;
    const room = seat.can_act && kept && kept.until > Date.now() ? kept.room : [];
    server.send(JSON.stringify({ type: "hello", doc: this.part(this.doc, "all", false), runtime: this.get("runtime"), me, assets: `/a/${this.share}/`, share: this.share, room }));
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
    if (size(message) > LINK_MESSAGE) return reply(false, undefined, `This change is too big: a link sends at most ${LINK_MESSAGE / 1024} KB at a time.`);
    const what = ({ comment: "A comment", reply: "A reply", answer: "An answer" } as Record<string, string>)[msg.op];
    const chars = [...String(msg.body?.text ?? "").trim()].length;
    if (what && chars > LINK_TEXT) {
      return reply(false, undefined, `${what} holds at most ${LINK_TEXT.toLocaleString("en")} characters; this one has ${chars.toLocaleString("en")}. Shorten it or split it.`);
    }
    try {
      const { events, doc } = await this.change({ ...msg.body, op: msg.op }, `person:${seat.who}`, LINK_FULL);
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
        this.columns();
        // A republish of a share that is already here is not a new share.
        if (!this.doc) this.ctx.waitUntil(sha256(`sharer:${bearer(req)}`).then((sharer) => this.env.PINS.getByName("pins").shared(sharer)));
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
        this.admit(await sha256(p.secret), p.who, p.can_act);
        // A share lasts as long as its longest link.
        return Response.json({ expires_at: await this.extend(p.days), share_origin: shareOrigin(this.env) });
      }
      if (path === "people" && req.method === "GET") {
        // One row per person, whatever number of links they hold: when they last
        // opened one, and how many of their pages are open now.
        const rows = sql.exec<{ who: string; can_act: number; opened_ms: number | null }>(
          "SELECT who, MAX(can_act) AS can_act, MAX(opened_ms) AS opened_ms FROM people GROUP BY who ORDER BY who").toArray();
        const people = rows.map((r) => ({ who: r.who, can_act: r.can_act === 1, opened_ms: r.opened_ms, here: this.ctx.getWebSockets(`p:${r.who}`).length }));
        return Response.json({ people, expires_at: await this.ctx.storage.getAlarm(), share_origin: shareOrigin(this.env) });
      }
      if (path === "people" && req.method === "DELETE") {
        const who = url.searchParams.get("who") ?? "";
        // Every link of `who`'s: minted with --to, or made by joining with the PIN.
        if (!sql.exec("DELETE FROM people WHERE who = ?", who).rowsWritten) {
          throw new Refusal(`${this.doc.name} has no ${who ? `link for ${who}` : "view-only link"}`);
        }
        for (const ws of this.ctx.getWebSockets(`p:${who}`)) ws.close(4403, "revoked");
        return new Response(null, { status: 204 });
      }
      if (path === "pin" && req.method === "POST") {
        const p = await json<{ hours: number; days: number }>();
        const share_expires_at = await this.extend(p.days);
        const jelly = { share, name: this.doc.name, label: this.doc.about || this.doc.name.split(":").pop()! };
        const made = await this.env.PINS.getByName("pins").add(jelly, Math.min(Math.max(p.hours, 1), MAX_HOURS));
        return Response.json({ ...made, tries: PIN_TRIES, share_expires_at, share_origin: shareOrigin(this.env) });
      }
      if (path === "pin" && req.method === "DELETE") {
        if (!(await this.env.PINS.getByName("pins").end(share))) throw new Refusal(`${this.doc.name} is not in a PIN's room`);
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

/** The relay's PIN and its room, and the wrong tries against it. One object,
 *  so every guess passes the same throttle before any lookup. A relay has one
 *  publish secret, so one sharer, and at most one PIN live: the meeting's. As
 *  the relay's one object it also holds what the relay counts (counts.ts). */
export class Pins extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS pin (hash TEXT PRIMARY KEY, expires_at INTEGER NOT NULL, misses INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS room (share TEXT PRIMARY KEY, name TEXT NOT NULL, label TEXT NOT NULL, at_ms INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS misses (ip TEXT NOT NULL, at_ms INTEGER NOT NULL);`);
    counting.tables(ctx.storage.sql);
  }

  async shared(sharer: string) { counting.shared(this.ctx.storage.sql, sharer); }
  async opened(share: string, people: number, first: boolean) { counting.opened(this.ctx.storage.sql, share, people, first); }
  async counts() { return counting.counts(this.ctx.storage.sql); }

  /** A plain hash of a 6-digit PIN is reversed in a moment, so it is keyed with the relay's secret. */
  private async hash(pin: string): Promise<string> {
    const enc = new TextEncoder();
    const key = await crypto.subtle.importKey("raw", enc.encode(this.env.RELAY_SECRET), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    return hex(await crypto.subtle.sign("HMAC", key, enc.encode(pin)));
  }

  /** The live PIN, if any. A room lasts as long as its PIN. */
  private live(now = Date.now()): { hash: string; expires_at: number } | undefined {
    const sql = this.ctx.storage.sql;
    sql.exec("DELETE FROM misses WHERE at_ms <= ?", now - IP_WINDOW);
    if (sql.exec("DELETE FROM pin WHERE expires_at <= ?", now).rowsWritten) sql.exec("DELETE FROM room");
    return sql.exec<{ hash: string; expires_at: number }>("SELECT hash, expires_at FROM pin").toArray()[0];
  }

  /** The room's jellies, the one added last at the end. */
  private room(): InRoom[] {
    return this.ctx.storage.sql.exec<{ share: string; name: string; label: string }>("SELECT share, name, label FROM room ORDER BY at_ms").toArray().map((r) => ({ ...r }));
  }

  /** Ends the PIN and its room; returns the shares that were in it. */
  private close(): string[] {
    const shares = this.room().map((r) => r.share);
    this.ctx.storage.sql.exec("DELETE FROM pin");
    this.ctx.storage.sql.exec("DELETE FROM room");
    return shares;
  }

  /** Tells each jelly in `shares` what the room is now. */
  private async tell(shares: string[], room: InRoom[], until: number) {
    await Promise.all(shares.map((s) => this.env.LIVE.getByName(s).room(room, until)));
  }

  /** Puts a jelly in the room, making a new PIN when none is live. `pin` is
   *  null when the room was open already: only its HMAC is kept, and the owner
   *  gave the PIN out when it was made. People can join at least `hours` more. */
  async add(jelly: InRoom, hours: number): Promise<{ pin: string | null; expires_at: number; room: InRoom[] }> {
    const sql = this.ctx.storage.sql;
    const now = Date.now();
    const live = this.live(now);
    const pin = live ? null : String(below(1_000_000)).padStart(6, "0");
    const hash = pin && (await this.hash(pin));
    const expires_at = Math.max(live?.expires_at ?? 0, now + hours * 3_600_000);
    // Added again, it is the newest: where people who type the PIN now land.
    const room = [...this.room().filter((r) => r.share !== jelly.share), jelly];
    // The room is told before anything is stored: when telling fails, the
    // owner gets an error and no PIN is left working that they never saw.
    await this.tell(room.map((r) => r.share), room, expires_at);
    if (hash) sql.exec("INSERT INTO pin (hash, expires_at, misses) VALUES (?, ?, 0)", hash, expires_at);
    else sql.exec("UPDATE pin SET expires_at = ?", expires_at);
    sql.exec("INSERT INTO room (share, name, label, at_ms) VALUES (?, ?, ?, ?) ON CONFLICT(share) DO UPDATE SET name = excluded.name, label = excluded.label, at_ms = excluded.at_ms",
      jelly.share, jelly.name, jelly.label, now);
    return { pin, expires_at, room };
  }

  /** Ends the PIN of the room `share` is in, for every jelly in it; false when it is in none. */
  async end(share: string): Promise<boolean> {
    if (!this.live() || !this.room().some((r) => r.share === share)) return false;
    await this.tell(this.close(), [], 0);
    return true;
  }

  /** `share` is no longer shared: it leaves the room, and an empty room's PIN ends. */
  async leave(share: string) {
    const live = this.live();
    if (!live || !this.ctx.storage.sql.exec("DELETE FROM room WHERE share = ?", share).rowsWritten) return;
    const room = this.room();
    if (!room.length) return void this.close();
    await this.tell(room.map((r) => r.share), room, live.expires_at);
  }

  /** Whether both shares are in the live room. */
  async together(a: string, b: string): Promise<boolean> {
    const shares = this.live() ? this.room().map((r) => r.share) : [];
    return shares.includes(a) && shares.includes(b);
  }

  /** The share the PIN opens: the jelly added to the room last. A wrong PIN
   *  counts against the IP and against the PIN, which locks at PIN_TRIES.
   *  Expired, ended and locked PINs are all just wrong. */
  async open(pin: string, ip: string): Promise<{ share: string } | { wait: number } | { wrong: true }> {
    const hash = await this.hash(pin);
    const sql = this.ctx.storage.sql;
    const now = Date.now();
    const live = this.live(now);
    const tried = sql.exec<{ n: number; first: number | null }>("SELECT COUNT(*) AS n, MIN(at_ms) AS first FROM misses WHERE ip = ?", ip).one();
    if (tried.n >= IP_TRIES) return { wait: Math.max(1, Math.ceil((tried.first! + IP_WINDOW - now) / 60_000)) };
    const room = this.room();
    if (live?.hash === hash && room.length) return { share: room[room.length - 1].share };
    sql.exec("INSERT INTO misses (ip, at_ms) VALUES (?, ?)", ip, now);
    if (live && sql.exec<{ misses: number }>("UPDATE pin SET misses = misses + 1 RETURNING misses").one().misses >= PIN_TRIES) {
      for (const share of this.close()) await this.env.LIVE.getByName(share).locked(PIN_TRIES);
    }
    return { wrong: true };
  }
}

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
// Reserved for examples (RFC 2606): never a person. Everyone who typed one
// would become the same person in the share.
const EXAMPLE = /@(.+\.)?example\.(com|net|org)$/;

/** POST /pin: a PIN and an email in, that person's own link out. */
async function enter(req: Request, env: Env): Promise<Response> {
  const no = (error: string, status: number) => Response.json({ error }, { status, headers: HEADERS });
  // JSON only: a page on another site cannot send it without a preflight, which this relay never allows.
  if (!req.headers.get("Content-Type")?.startsWith("application/json")) return no("expected JSON", 415);
  const body: { pin?: unknown; email?: unknown } = await req.json<object>().catch(() => ({}));
  const pin = String(body.pin ?? "").replace(/\s/g, "");
  const email = String(body.email ?? "").trim().toLowerCase();
  // Checked before the PIN is looked up, so these answers say nothing about it.
  if (!/^\d{6}$/.test(pin)) return no("A PIN is 6 digits.", 400);
  if (email.length > 254 || !EMAIL.test(email)) return no("Type your email address.", 400);
  if (EXAMPLE.test(email)) return no("That is an example address. Type your own email.", 400);
  const wrong = () => no("That PIN does not work. Check it with whoever gave it to you.", 403);
  const found = await env.PINS.getByName("pins").open(pin, req.headers.get("CF-Connecting-IP") ?? "unknown");
  if ("wait" in found) return no(`Too many wrong PINs from here. Try again in ${found.wait} minute${found.wait === 1 ? "" : "s"}.`, 429);
  if ("wrong" in found) return wrong();
  const secret = await env.LIVE.getByName(found.share).join(email);
  return secret ? Response.json({ link: `/j/${found.share}${secret}` }, { headers: HEADERS }) : wrong();
}

const FROM = new RegExp(`^(${SHARE})([A-Za-z0-9]{22})$`);

/** POST /room: a person in the PIN's room moves to another of its jellies.
 *  Their link on one jelly in the room is their way in: they get their own
 *  link on the other, for the same email, without the PIN. */
async function follow(req: Request, env: Env): Promise<Response> {
  const no = (error: string, status: number) => Response.json({ error }, { status, headers: HEADERS });
  if (!req.headers.get("Content-Type")?.startsWith("application/json")) return no("expected JSON", 415);
  const body: { from?: unknown; to?: unknown } = await req.json<object>().catch(() => ({}));
  const [, from, secret] = String(body.from ?? "").match(FROM) ?? [];
  const to = String(body.to ?? "");
  if (!from || !new RegExp(`^${SHARE}$`).test(to)) return no("expected a link and a share", 400);
  if (!(await env.PINS.getByName("pins").together(from, to))) return no("That jelly is no longer in this room.", 403);
  const who = await env.LIVE.getByName(from).who(secret);
  if (!who) return no("This link cannot move to another jelly.", 403);
  const made = await env.LIVE.getByName(to).join(who);
  return made ? Response.json({ link: `/j/${to}${made}` }, { headers: HEADERS }) : no("That jelly is no longer shared.", 403);
}

const JOIN_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'";

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const path = url.pathname;
    const live = (share: string, rest: string) => env.LIVE.getByName(share).fetch(new Request(`https://live/${share}/${rest}${url.search}`, req));

    if (path === "/robots.txt") return text("User-agent: *\nDisallow: /\n");

    // What people open lives on the share origin only. A link made before it
    // was set still arrives here, and goes on to the same path there.
    const pages = shareOrigin(env);
    if (pages && new URL(pages).host !== url.host && (path === "/pin" || path === "/room" || LINK.test(path) || ASSET.test(path))) {
      return new Response(null, { status: 308, headers: { Location: pages + path + url.search, ...HEADERS } });
    }

    if (path === "/pin" && req.method === "GET") {
      return new Response(joinPage, { headers: { "Content-Type": "text/html; charset=utf-8", "Content-Security-Policy": JOIN_CSP, ...HEADERS } });
    }
    if (path === "/pin" && req.method === "POST") return enter(req, env);
    if (path === "/room" && req.method === "POST") return follow(req, env);

    const link = path.match(LINK);
    if (link && req.method === "GET") return live(link[1], `link/${link[2]}${link[3] ?? ""}`);

    const asset = path.match(ASSET);
    if (asset && req.method === "GET") return live(asset[1], `assets/${asset[2]}`);

    if (path === "/api/counts" && req.method === "GET") {
      if (!(await authorized(req, env))) return text("unauthorized\n", 401);
      return Response.json(await env.PINS.getByName("pins").counts(), { headers: HEADERS });
    }

    const owner = path.match(OWNER);
    if (owner) {
      if (!(await authorized(req, env))) return text("unauthorized\n", 401);
      return live(owner[1], `owner${owner[2] ?? ""}`);
    }

    if (path === "/") return text("A jelly relay. Shared jellies open at /j/<link>, or with a PIN at /pin.\n");
    return notFound();
  },
} satisfies ExportedHandler<Env>;
