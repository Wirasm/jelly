// The Send count on a shared jelly, checked against the real Worker running locally.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createTestHarness } from "wrangler";

const SECRET = "test-secret";
const auth = { Authorization: `Bearer ${SECRET}` };
const server = createTestHarness({ workers: [{ configPath: new URL("../wrangler.jsonc", import.meta.url), vars: { RELAY_SECRET: SECRET } }] });
let url;
before(async () => { ({ url } = await server.listen()); });
after(() => server.close());

test("a quiet verb call from a link does not count toward Send; a plain one does", async () => {
  const share = "q".padStart(22, "a");
  const link = "l".padStart(22, "s");
  const doc = { name: "t:race", about: "", view: "", verbs: "const verbs = { pos(s, { x }) { s.x = x; } }", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  assert.equal((await fetch(new URL(`/api/live/${share}`, url), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) })).status, 200);
  assert.equal((await fetch(new URL(`/api/live/${share}/people`, url), { method: "POST", headers: auth, body: JSON.stringify({ secret: link, who: "ana", can_act: true, days: 1 }) })).status, 200);

  const ws = new WebSocket(new URL(`/j/${share}${link}/ws`, url).href.replace("http", "ws"));
  const messages = [];
  await new Promise((ok, no) => { ws.onmessage = (m) => { messages.push(JSON.parse(m.data)); ok(); }; ws.onerror = no; });
  /** Runs `pos` and returns the event and the count the relay broadcast with it. */
  const pos = async (x, quiet) => {
    const seen = messages.length;
    ws.send(JSON.stringify({ ref: x, op: "do", body: { verb: "pos", args: { x }, quiet } }));
    for (let i = 0; i < 100; i++) {
      const msg = messages.slice(seen).find((m) => m.type === "events");
      if (msg) return { quiet: msg.events[0].quiet, unsent: msg.doc.unsent ?? 0 };
      await new Promise((r) => setTimeout(r, 20));
    }
    throw new Error("no event came back");
  };
  assert.deepEqual(await pos(1, true), { quiet: true, unsent: 0 });
  assert.deepEqual(await pos(2, true), { quiet: true, unsent: 0 });
  assert.deepEqual(await pos(3, undefined), { quiet: undefined, unsent: 1 });
  ws.close();
});
