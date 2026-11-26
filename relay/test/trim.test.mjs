// Which stored events a share deletes once the owner's daemon says it saved
// them (src/trim.ts): against SQLite in Node for the week, and against the real
// Worker running locally for the wiring (`npm test`; nothing is deployed).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { createTestHarness } from "wrangler";
import { forget, KEEP_MS } from "../src/trim.ts";

const DAY = 86_400_000;

test("of the events the daemon saved, those older than a week go; the newest and the unsaved stay", () => {
  const db = new DatabaseSync(":memory:");
  db.exec("CREATE TABLE events (seq INTEGER PRIMARY KEY, ev TEXT NOT NULL)");
  const now = Date.UTC(2026, 10, 26);
  const add = (seq, daysAgo) => db.prepare("INSERT INTO events (seq, ev) VALUES (?, ?)").run(seq, JSON.stringify({ seq, at_ms: now - daysAgo * DAY, kind: "do" }));
  add(1, 9); add(2, 8); add(3, 6); add(4, 8.5); add(5, 1); add(6, 30);
  const sql = { exec: (q, ...a) => db.prepare(q).run(...a) };
  forget(sql, 4, 6, now);
  const left = () => db.prepare("SELECT seq FROM events ORDER BY seq").all().map((r) => r.seq);
  // 1, 2 and 4 are saved and over a week old; 3 is six days old; 5 is not saved;
  // 6 is the newest, kept though it is thirty days old.
  assert.deepEqual(left(), [3, 5, 6]);
  forget(sql, 6, 6, now + 2 * DAY);
  assert.deepEqual(left(), [5, 6], "two days on, the six-day-old event is past the week");
  assert.equal(KEEP_MS, 7 * DAY);
});

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

test("events the daemon saved this week stay on the relay, so a daemon behind hears each one", async () => {
  const doc = { name: "t:trim", about: "", view: "", verbs: "const verbs = { inc(s) { s.n = (s.n || 0) + 1; } }", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  assert.equal((await fetch(at(`/api/live/${share}`), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) })).status, 200);
  for (let i = 0; i < 4; i++) assert.equal((await op({ op: "do", verb: "inc" })).status, 200);

  const a = await owner(0);
  assert.deepEqual(seqs(a.first), [1, 2, 3, 4]);
  assert.ok(a.first.events.every((e) => Math.abs(e.at_ms - Date.now()) < 60_000), "each event carries the relay's time");
  a.ws.send(JSON.stringify({ saved: 4 }));
  a.ws.send("ping");
  await new Promise((r) => setTimeout(r, 100));
  a.ws.close();

  const b = await owner(1);
  assert.deepEqual(seqs(b.first), [2, 3, 4], "nothing a week old, so nothing went");
  assert.deepEqual(b.first.doc.state, { n: 4 }, "the catch-up carries the whole jelly");
  b.ws.close();
  const next = await (await op({ op: "do", verb: "inc" })).json();
  assert.deepEqual(next.events.map((e) => e.seq), [5]);
});
