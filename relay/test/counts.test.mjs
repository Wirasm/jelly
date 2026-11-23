// The relay's counts, checked against the real Worker running locally:
// `npm test` (wrangler starts it; nothing is deployed).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createTestHarness } from "wrangler";

const SECRET = "test-secret";
const auth = { Authorization: `Bearer ${SECRET}` };
const server = createTestHarness({ workers: [{ configPath: new URL("../wrangler.jsonc", import.meta.url), vars: { RELAY_SECRET: SECRET } }] });
let url;
before(async () => { ({ url } = await server.listen()); });
after(() => server.close());

const id = (n) => String(n).padStart(22, "a");
const today = new Date().toISOString().slice(0, 10);

async function publish(share) {
  const doc = { name: "t:plan", about: "", view: "", verbs: "const verbs = {}", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  const r = await fetch(new URL(`/api/live/${share}`, url), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) });
  assert.equal(r.status, 200);
}

async function mint(share, secret, who) {
  const r = await fetch(new URL(`/api/live/${share}/people`, url), { method: "POST", headers: auth, body: JSON.stringify({ secret, who, can_act: !!who, days: 1 }) });
  assert.equal(r.status, 200);
}

/** Opens a link's page socket and waits for its hello. */
function open(share, secret) {
  const ws = new WebSocket(new URL(`/j/${share}${secret}/ws`, url).href.replace("http", "ws"));
  return new Promise((resolve, reject) => {
    ws.onmessage = () => resolve(ws);
    ws.onerror = ws.onclose = () => reject(new Error("the link's socket did not open"));
  });
}

async function counts() {
  const r = await fetch(new URL("/api/counts", url), { headers: auth });
  assert.equal(r.status, 200);
  return r.json();
}

/** Counting runs beside the request it counts, so the numbers land a moment later. */
async function settled(want) {
  for (let i = 0; ; i++) {
    const got = await counts();
    try { return assert.deepEqual(got, want); } catch (e) { if (i === 40) throw e; }
    await new Promise((r) => setTimeout(r, 50));
  }
}

test("counts are for the holder of the publish secret only", async () => {
  assert.equal((await fetch(new URL("/api/counts", url))).status, 401);
  assert.equal((await fetch(new URL("/api/counts", url), { headers: { Authorization: "Bearer wrong" } })).status, 401);
});

test("shares, returning shares, opens and people per share land in today's bucket", async () => {
  const [a, b] = [id(1), id(2)];
  const link = (n) => String(n).padStart(22, "s");
  await publish(a);
  await publish(a); // a republish is the same share
  await publish(b); // the same secret sharing again: a returning share
  await mint(a, link(1), "ana@example.com");
  await mint(a, link(2), "bo@example.com");
  await mint(a, link(3), "");
  await mint(b, link(4), "ana@example.com");

  const sockets = [
    await open(a, link(1)),
    await open(a, link(1)), // Ana's second page: still one person, and one open today
    await open(a, link(2)),
    await open(a, link(3)), // two pages on the view-only link are two people
    await open(a, link(3)),
    await open(b, link(4)),
  ];
  const day = { shares: 2, returning_shares: 1, opens: 4, rooms: { 1: 1, 4: 1 } };
  await settled({ days: [{ day: today, ...day }], total: { ...day, sharers: 1, came_back: 0 } });

  // The peak stays when people leave, and a link that never opened counts nothing.
  for (const ws of sockets) ws.close();
  assert.equal((await fetch(new URL(`/j/${a}${link(9)}`, url))).status, 404);
  await settled({ days: [{ day: today, ...day }], total: { ...day, sharers: 1, came_back: 0 } });
});
