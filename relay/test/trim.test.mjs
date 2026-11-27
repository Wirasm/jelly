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
const id = (n) => String(n).padStart(22, "r");
const op = (share) => fetch(at(`/api/live/${share}/op`), { method: "POST", headers: auth, body: JSON.stringify({ by: "agent", op: "do", verb: "inc" }) });

/** Publishes a jelly as `share` and makes four changes: seqs 1 to 4. */
async function shared(share) {
  const doc = { name: "t:trim", about: "", view: "", verbs: "const verbs = { inc(s) { s.n = (s.n || 0) + 1; } }", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  assert.equal((await fetch(at(`/api/live/${share}`), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) })).status, 200);
  for (let i = 0; i < 4; i++) assert.equal((await op(share)).status, 200);
}

/** The daemon's socket, from `since`: resolves with its catch-up and the socket. */
function owner(share, since) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(at(`/api/live/${share}/ws?since=${since}`).href.replace("http", "ws"), { headers: auth });
    ws.onmessage = (m) => { ws.onmessage = null; resolve({ ws, first: JSON.parse(m.data) }); };
    ws.onerror = reject;
  });
}
const seqs = (batch) => batch.events.map((e) => e.seq);

test("events the daemon saved this week stay on the relay, so a daemon behind hears each one", async () => {
  const share = id(1);
  await shared(share);

  const a = await owner(share, 0);
  assert.deepEqual(seqs(a.first), [1, 2, 3, 4]);
  assert.ok(a.first.events.every((e) => Math.abs(e.at_ms - Date.now()) < 60_000), "each event carries the relay's time");
  a.ws.send(JSON.stringify({ saved: 4 }));
  a.ws.send("ping");
  await new Promise((r) => setTimeout(r, 100));
  a.ws.close();

  const b = await owner(share, 1);
  assert.deepEqual(seqs(b.first), [2, 3, 4], "nothing a week old, so nothing went");
  assert.deepEqual(b.first.doc.state, { n: 4 }, "the catch-up carries the whole jelly");
  b.ws.close();
  const next = await (await op(share)).json();
  assert.deepEqual(next.events.map((e) => e.seq), [5]);
});

test("the daemon's report deletes the saved events the relay made over a week ago, and numbering goes on", async () => {
  const share = id(2);
  await shared(share);
  // The relay stamps each event from its own clock, so the test makes 1, 2 and
  // 4 old where they are stored.
  const worker = server.getWorker();
  const sql = await worker.getDurableObjectStorage("LIVE", { name: share });
  await sql.exec("UPDATE events SET ev = json_set(ev, '$.at_ms', ?) WHERE seq IN (1, 2, 4)", Date.now() - 8 * DAY);
  const left = async () => (await sql.exec("SELECT seq FROM events ORDER BY seq")).map((r) => r.seq);
  assert.deepEqual(await left(), [1, 2, 3, 4]);

  const a = await owner(share, 0);
  a.ws.send(JSON.stringify({ saved: 4 }));
  for (let i = 0; i < 50 && (await left()).length === 4; i++) await new Promise((r) => setTimeout(r, 20));
  a.ws.close();
  assert.deepEqual(await left(), [3, 4], "1 and 2 went; 3 is from this week, and 4 is the newest");

  // The object starts again and numbers on from the newest event, which it kept.
  await worker.evictDurableObject("LIVE", { name: share, webSockets: "close" });
  const next = await (await op(share)).json();
  assert.deepEqual(next.events.map((e) => e.seq), [5]);
  const b = await owner(share, 0);
  assert.deepEqual(seqs(b.first), [3, 4, 5]);
  assert.deepEqual(b.first.doc.state, { n: 5 });
  b.ws.close();
});
