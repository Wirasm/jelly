import { DurableObject } from "cloudflare:workers";
import { newQuickJSWASMModuleFromVariant, newVariant, shouldInterruptAfterDeadline, type QuickJSWASMModule } from "quickjs-emscripten-core";
import baseVariant from "@jitl/quickjs-wasmfile-release-sync";
// Workers cannot fetch a .wasm at runtime; importing it compiles it at deploy time.
import wasmModule from "./quickjs.wasm";
const variant = newVariant(baseVariant, { wasmModule });

interface Env { JELLY: DurableObjectNamespace<SharedJelly> }
interface Jelly { view: string; verbs: string; state: unknown; version: number }

let qjs: Promise<QuickJSWASMModule> | undefined;
const engine = () => (qjs ??= newQuickJSWASMModuleFromVariant(variant));

/** Runs one verb exactly like the daemon: a fresh context, 1 s limit, no I/O.
 *  Synchronous on purpose: a Durable Object lets other requests in at every await,
 *  so read state → run verb → save must have no await in between. */
function runVerb(mod: QuickJSWASMModule, src: string, verb: string, state: unknown, args: unknown, by: string): unknown {
  const rt = mod.newRuntime();
  rt.setInterruptHandler(shouldInterruptAfterDeadline(Date.now() + 1000));
  rt.setMemoryLimit(64 * 1024 * 1024);
  const ctx = rt.newContext();
  try {
    const code = `${src}
;(() => {
  if (typeof verbs !== "object" || typeof verbs[${JSON.stringify(verb)}] !== "function") throw new Error("no verb " + ${JSON.stringify(verb)});
  const s = JSON.parse(${JSON.stringify(JSON.stringify(state))});
  const r = verbs[${JSON.stringify(verb)}](s, JSON.parse(${JSON.stringify(JSON.stringify(args ?? {}))}), { by: ${JSON.stringify(by)}, at_ms: ${Date.now()} });
  return JSON.stringify(r === undefined ? s : r);
})()`;
    const result = ctx.evalCode(code);
    if (result.error) {
      const err = ctx.dump(result.error); result.error.dispose();
      const msg = typeof err === "object" && err ? (err.message ?? JSON.stringify(err)) : String(err);
      throw new Error(/interrupted/i.test(msg) ? "verb ran longer than 1s and was stopped" : msg);
    }
    const out = ctx.getString(result.value); result.value.dispose();
    return JSON.parse(out);
  } finally { ctx.dispose(); rt.dispose(); }
}

export class SharedJelly extends DurableObject<Env> {
  private jelly(): Jelly | undefined {
    const row = this.ctx.storage.sql.exec<{ v: string }>("SELECT v FROM kv WHERE k = 'jelly'").toArray()[0];
    return row ? JSON.parse(row.v) : undefined;
  }
  private save(j: Jelly) {
    this.ctx.storage.sql.exec("INSERT INTO kv (k, v) VALUES ('jelly', ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v", JSON.stringify(j));
  }
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => { ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)"); });
  }
  async create(j: Omit<Jelly, "version">): Promise<number> {
    try { runVerb(await engine(), j.verbs, "__none__", {}, {}, "check"); }
    catch (e) { if (!/no verb __none__/.test((e as Error).message)) throw e; }
    this.save({ ...j, version: 1 });
    return 1;
  }
  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const body = req.method === "POST" ? await req.json<{ verb: string; args: unknown; by: string }>() : undefined;
    const mod = await engine();
    // No await from here to the save: this call is atomic within the object.
    const j = this.jelly();
    if (!j) return new Response("no such jelly", { status: 404 });
    if (url.pathname.endsWith("/ws")) {
      const [client, server] = Object.values(new WebSocketPair());
      this.ctx.acceptWebSocket(server);
      return new Response(null, { status: 101, webSocket: client });
    }
    if (url.pathname.endsWith("/call") && req.method === "POST") {
      const { verb, args, by } = body!;
      let next: unknown;
      const t0 = Date.now();
      try { next = runVerb(mod, j.verbs, verb, j.state, args, by || "person"); }
      catch (e) { return Response.json({ error: (e as Error).message }, { status: 400 }); }
      const updated = { ...j, state: next, version: j.version + 1 };
      this.save(updated);
      const ev = JSON.stringify({ kind: "call", verb, by, version: updated.version, state: next, ms: Date.now() - t0 });
      for (const ws of this.ctx.getWebSockets()) ws.send(ev);
      return new Response(ev, { headers: { "content-type": "application/json" } });
    }
    if (url.pathname.endsWith("/state")) return Response.json(j.state);
    return new Response(page(j), { headers: { "content-type": "text/html; charset=utf-8" } });
  }
}

/** A minimal jelly runtime: enough of window.jelly for recipe views. */
function page(j: Jelly): string {
  const boot = JSON.stringify({ state: j.state, version: j.version }).replace(/</g, "\\u003c");
  const runtime = `<script>(() => {
    const boot = ${boot}; let state = boot.state, version = boot.version; const subs = [];
    const name = new URLSearchParams(location.search).get("as") || localStorage.getItem("jelly-viewer") || "";
    const by = name ? "person:" + name : "person";
    const base = location.pathname.replace(/\\/$/, "");
    const apply = (ev) => { if (ev.version <= version) return; version = ev.version; state = ev.state; subs.forEach((f) => f(state, ev)); };
    const ws = new WebSocket((location.protocol === "https:" ? "wss://" : "ws://") + location.host + base + "/ws");
    ws.onmessage = (m) => apply(JSON.parse(m.data));
    window.jelly = {
      get state() { return state; }, get by() { return by; },
      get viewer() { return name; }, set viewer(v) { localStorage.setItem("jelly-viewer", v); location.reload(); },
      onState(fn) { subs.push(fn); fn(state, null); },
      async call(verb, args = {}) { const r = await fetch(base + "/call", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ verb, args, by }) }); const ev = await r.json(); if (!r.ok) throw new Error(ev.error); apply(ev); return ev; },
      isPerson: (b) => b === "person" || String(b).startsWith("person:"), who: (b) => (b === by ? "you" : String(b).replace(/^person:/, "")),
    };
  })();</script>`;
  return j.view.replace(/<head>/i, "<head>" + runtime).replace(/^(?![\s\S]*<head>)/i, runtime);
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    // /s/<token>/...  → that shared jelly.   POST /create?token=… makes one (spike only).
    if (url.pathname === "/create" && req.method === "POST") {
      const token = url.searchParams.get("token")!;
      const body = await req.json<Omit<Jelly, "version">>();
      try { await env.JELLY.getByName(token).create(body); }
      catch (e) { return Response.json({ error: (e as Error).message }, { status: 400 }); }
      return Response.json({ url: `${url.origin}/s/${token}/` });
    }
    const m = url.pathname.match(/^\/s\/([A-Za-z0-9_-]{8,})(\/.*)?$/);
    if (!m) return new Response("not found", { status: 404 });
    if (!m[2]) return Response.redirect(`${url.origin}/s/${m[1]}/`, 301);
    return env.JELLY.getByName(m[1]).fetch(req);
  },
} satisfies ExportedHandler<Env>;
