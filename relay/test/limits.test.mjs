// What one share can be made to do to itself: a verb that never returns, a
// link that sends too much, the event log, and ending a share while changes
// arrive. Checked against the real Worker running locally: `npm test`
// (wrangler starts it; nothing is deployed).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createTestHarness } from "wrangler";

const SECRET = "test-secret";
const auth = { Authorization: `Bearer ${SECRET}` };
const server = createTestHarness({ workers: [{ configPath: new URL("../wrangler.jsonc", import.meta.url), vars: { RELAY_SECRET: SECRET } }] });
let url;
before(async () => { ({ url } = await server.listen()); });
after(() => server.close());

const id = (n) => String(n).padStart(22, "z");
const at = (path) => new URL(path, url);
const VERBS = "const verbs = { spin() { while (true) {} }, inc(s) { s.n = (s.n || 0) + 1; }, keep(s, a) { s.kept = a; } }";

async function publish(share, state = {}) {
  const doc = { name: "t:limits", about: "", view: "", verbs: VERBS, state, version: 1, view_rev: 1, comments: [], asks: [] };
  assert.equal((await fetch(at(`/api/live/${share}`), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) })).status, 200);
}

/** A change from the owner's daemon, as `relay.rs` sends it. */
const op = (share, body) => fetch(at(`/api/live/${share}/op`), { method: "POST", headers: auth, body: JSON.stringify({ by: "agent", ...body }) });

/** Ana's page on `share`: `send` makes a change and resolves with the relay's reply. */
async function ana(share) {
  const secret = "a".repeat(22);
  await fetch(at(`/api/live/${share}/people`), { method: "POST", headers: auth, body: JSON.stringify({ secret, who: "ana@mail.test", can_act: true, days: 1 }) });
  const ws = new WebSocket(at(`/j/${share}${secret}/ws`).href.replace("http", "ws"));
  const waiting = new Map();
  let ref = 0;
  ws.onmessage = (m) => { const msg = JSON.parse(m.data); if (msg.type === "reply") waiting.get(msg.ref)?.(msg); };
  await new Promise((ok, no) => { ws.addEventListener("message", ok, { once: true }); ws.onerror = no; });
  const send = (o, body) => new Promise((ok) => { waiting.set(++ref, ok); ws.send(JSON.stringify({ ref, op: o, body })); });
  return { ws, send };
}

/** The daemon's socket, catching up from `since`: resolves with the seqs it was sent. */
function catchUp(share, since) {
  const ws = new WebSocket(at(`/api/live/${share}/ws?since=${since}`).href.replace("http", "ws"), { headers: auth });
  return new Promise((ok, no) => {
    ws.onmessage = (m) => { ws.close(); ok(JSON.parse(m.data).events.map((e) => e.seq)); };
    ws.onerror = no;
  });
}

// Run locally the clock moves during a verb, so a clock-based limit passes
// here too; deployed it stands still, which is why the relay counts steps.
test("a verb that never returns is stopped, and the share goes on working", async () => {
  const share = id(1);
  await publish(share);
  const r = await op(share, { op: "do", verb: "spin" });
  assert.equal(r.status, 400);
  assert.match((await r.json()).error, /verb ran too long and was stopped/);
  const next = await op(share, { op: "do", verb: "inc" });
  assert.equal(next.status, 200);
  assert.deepEqual((await next.json()).doc.state, { n: 1 });
});

test("a link cannot send text or args past the limits; the owner's daemon can", async () => {
  const share = id(2);
  await publish(share);
  const { ws, send } = await ana(share);
  const long = "x".repeat(10_001);
  assert.match((await send("comment", { text: long })).error, /A comment holds at most 10,000 characters/);
  const ok = await send("comment", { text: "x".repeat(10_000) });
  assert.equal(ok.ok, true);
  assert.match((await send("reply", { id: ok.value.args.id, text: long })).error, /at most 10,000 characters/);
  assert.match((await send("do", { verb: "keep", args: "x".repeat(300_000) })).error, /at most 256 KB/);
  assert.equal((await send("do", { verb: "keep", args: "x".repeat(200_000) })).ok, true);
  // The owner's daemon is not limited: it may hold whatever the person's own jelly holds.
  assert.equal((await op(share, { op: "comment", text: long })).status, 200);
  ws.close();
});

test("changes from links stop short of filling the jelly, and the owner's still land", async () => {
  const share = id(3);
  // A jelly 25 KB short of what links may fill: two long comments fit, the third does not.
  await publish(share, { big: "x".repeat((4 << 20) - 25_000) });
  const { ws, send } = await ana(share);
  const comment = () => send("comment", { text: "y".repeat(10_000) });
  for (let i = 0; i < 2; i++) assert.equal((await comment()).ok, true);
  assert.equal((await comment()).error, "This jelly is full, so it takes nothing more from links. Tell whoever shared it.");
  // A small change from a link still fits, and the owner's daemon is not held back.
  assert.equal((await send("do", { verb: "inc" })).ok, true);
  assert.equal((await op(share, { op: "comment", text: "y".repeat(100_000) })).status, 200);
  ws.close();
});

test("the relay forgets events the daemon has mirrored, and keeps the rest", async () => {
  const share = id(4);
  await publish(share);
  for (let i = 0; i < 4; i++) assert.equal((await op(share, { op: "do", verb: "inc" })).status, 200);
  assert.deepEqual(await catchUp(share, 2), [3, 4]);
  // The daemon said it had mirrored up to 2: those are gone; what comes after it stays.
  assert.deepEqual(await catchUp(share, 0), [3, 4]);
  // Mirrored up to the newest: the newest stays, so the next change follows it.
  assert.deepEqual(await catchUp(share, 4), []);
  assert.deepEqual(await catchUp(share, 0), [4]);
  const r = await op(share, { op: "do", verb: "inc" });
  assert.deepEqual((await r.json()).events.map((e) => e.seq), [5]);
});

test("a change made while the share ends is refused, never accepted and then lost", async () => {
  const [share, other] = [id(5), id(6)];
  await publish(share);
  await publish(other);
  // In a PIN's room, ending tells the rest of the room: a wider gap for changes to fall in.
  for (const s of [share, other]) await fetch(at(`/api/live/${s}/pin`), { method: "POST", headers: auth, body: JSON.stringify({ hours: 1, days: 1 }) });
  // Changes keep coming, as from agents still at work, until the share is gone.
  const accepted = [];
  const agent = async () => {
    for (;;) {
      const r = await op(share, { op: "do", verb: "inc" });
      // 404 once it is gone, or 400 "no longer shared" for a change that came in as it ended.
      if (r.status !== 200) return assert.ok([400, 404].includes(r.status), String(r.status));
      accepted.push(...(await r.json()).events.map((e) => e.seq));
    }
  };
  const agents = Array.from({ length: 8 }, agent);
  while (accepted.length < 8) await new Promise((r) => setTimeout(r, 5));
  const last = await (await fetch(at(`/api/live/${share}?since=0`), { method: "DELETE", headers: auth })).json();
  await Promise.all(agents);
  const kept = last.events.map((e) => e.seq);
  assert.deepEqual(accepted.filter((s) => !kept.includes(s)), [], "changes accepted after the share's last events were read");
});
