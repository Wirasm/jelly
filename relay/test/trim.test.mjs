// The relay deletes the events the owner's daemon says it saved, and a daemon
// further behind gets the whole jelly to catch up from. Checked against the
// real Worker running locally: `npm test` (nothing is deployed).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createTestHarness } from "wrangler";

const SECRET = "test-secret";
const auth = { Authorization: `Bearer ${SECRET}` };
const server = createTestHarness({ workers: [{ configPath: new URL("../wrangler.jsonc", import.meta.url), vars: { RELAY_SECRET: SECRET } }] });
let url;
before(async () => { ({ url } = await server.listen()); });
after(() => server.close());

const at = (path) => new URL(path, url);
const share = "t".padStart(22, "r");
const op = (body) => fetch(at(`/api/live/${share}/op`), { method: "POST", headers: auth, body: JSON.stringify({ by: "agent", ...body }) });

/** The daemon's socket, from `since`: resolves with its catch-up and the socket. */
function owner(since) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(at(`/api/live/${share}/ws?since=${since}`).href.replace("http", "ws"), { headers: auth });
    ws.onmessage = (m) => { ws.onmessage = null; resolve({ ws, first: JSON.parse(m.data) }); };
    ws.onerror = reject;
  });
}
const seqs = (batch) => batch.events.map((e) => e.seq);

test("events the daemon saved are deleted, the newest stays, and a daemon behind gets the whole jelly", async () => {
  const doc = { name: "t:trim", about: "", view: "", verbs: "const verbs = { inc(s) { s.n = (s.n || 0) + 1; } }", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  assert.equal((await fetch(at(`/api/live/${share}`), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) })).status, 200);
  for (let i = 0; i < 4; i++) assert.equal((await op({ op: "do", verb: "inc" })).status, 200);

  const a = await owner(0);
  assert.deepEqual(seqs(a.first), [1, 2, 3, 4]);
  // Says it saved everything, and more than there is: the newest event stays.
  a.ws.send(JSON.stringify({ saved: 2 }));
  a.ws.send("ping");
  a.ws.send(JSON.stringify({ saved: 99 }));
  await new Promise((r) => setTimeout(r, 100));
  a.ws.close();

  const b = await owner(1);
  assert.deepEqual(seqs(b.first), [4], "only the newest event is left");
  assert.deepEqual(b.first.doc.state, { n: 4 }, "the catch-up carries the whole jelly");
  assert.equal(typeof b.first.doc.view, "string");
  b.ws.close();

  // Numbering goes on after the newest kept event.
  const next = await (await op({ op: "do", verb: "inc" })).json();
  assert.deepEqual(next.events.map((e) => e.seq), [5]);
  // Ending the share hands a daemon that is behind the whole jelly too.
  const last = await (await fetch(at(`/api/live/${share}?since=0`), { method: "DELETE", headers: auth })).json();
  assert.deepEqual(seqs(last), [4, 5]);
  assert.deepEqual(last.doc.state, { n: 5 });
});
