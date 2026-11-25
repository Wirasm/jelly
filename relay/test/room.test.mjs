// A PIN's room: several jellies on one PIN, and people moving between them
// without typing it again. Checked against the real Worker running locally:
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

const id = (n) => String(n).padStart(22, "r");
const at = (path) => new URL(path, url);
const post = (path, body, headers = {}) => fetch(at(path), { method: "POST", headers: { "Content-Type": "application/json", ...headers }, body: JSON.stringify(body) });

async function publish(share, name) {
  const doc = { name, about: "", view: "", verbs: "const verbs = {}", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  assert.equal((await fetch(at(`/api/live/${share}`), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) })).status, 200);
}

/** `jelly share NAME --pin`, as the daemon asks for it. */
async function pin(share) {
  const r = await fetch(at(`/api/live/${share}/pin`), { method: "POST", headers: auth, body: JSON.stringify({ hours: 3, days: 1 }) });
  assert.equal(r.status, 200);
  return r.json();
}

const unpin = (share) => fetch(at(`/api/live/${share}/pin`), { method: "DELETE", headers: auth });

/** Types the PIN at /pin; returns the link token (share id + secret) or the error. */
async function join(code, email, ip = "10.0.0.1") {
  const r = await post("/pin", { pin: code, email }, { "CF-Connecting-IP": ip });
  const body = await r.json();
  return r.ok ? body.link.slice(3) : body.error;
}

/** Opens a link's page socket; resolves with its hello and a queue of what comes after. */
function open(token) {
  const ws = new WebSocket(at(`/j/${token}/ws`).href.replace("http", "ws"));
  const later = [];
  let wake;
  const next = () => later.shift() ?? new Promise((r, fail) => {
    wake = r;
    setTimeout(() => fail(new Error("the page heard nothing more")), 5000).unref();
  });
  return new Promise((resolve, reject) => {
    ws.onmessage = (m) => {
      const msg = JSON.parse(m.data);
      if (msg.type === "hello") return resolve({ ws, hello: msg, next });
      if (wake) { const w = wake; wake = undefined; w(msg); } else later.push(msg);
    };
    ws.onerror = ws.onclose = () => reject(new Error("the link's socket did not open"));
  });
}

const names = (room) => room.map((j) => j.name);

test("jellies shared with --pin while a PIN works share it, and people move between them without it", async () => {
  const [board, quiz] = [id(1), id(2)];
  await publish(board, "t:board");
  await publish(quiz, "t:quiz");

  const first = await pin(board);
  assert.match(first.pin, /^\d{6}$/);
  const ana = await join(first.pin, "ana@mail.test");
  assert.ok(ana.startsWith(board));
  const page = await open(ana);
  assert.deepEqual(names(page.hello.room), ["t:board"]);

  // The second jelly joins the same room: no new PIN, and Ana's open page hears of it.
  const second = await pin(quiz);
  assert.equal(second.pin, null);
  assert.ok(second.expires_at >= first.expires_at, "people can join at least --hours more");
  assert.deepEqual(names(second.room), ["t:board", "t:quiz"]);
  const told = await page.next();
  assert.equal(told.type, "room");
  assert.deepEqual(told.room.map((j) => j.label), ["board", "quiz"]);

  // She moves to the quiz on her own link, as herself.
  const moved = await post("/room", { from: ana, to: quiz });
  assert.equal(moved.status, 200);
  const token = (await moved.json()).link.slice(3);
  assert.ok(token.startsWith(quiz));
  const there = await open(token);
  assert.equal(there.hello.me.who, "ana@mail.test");
  assert.deepEqual(names(there.hello.room), ["t:board", "t:quiz"]);

  // Someone typing the PIN now lands on the jelly added last.
  assert.ok((await join(first.pin, "bo@mail.test")).startsWith(quiz));
  for (const ws of [page.ws, there.ws]) ws.close();
  assert.equal((await unpin(quiz)).status, 204);
});

test("only a link that may act moves, and only inside a live room", async () => {
  const [a, b, outside] = [id(3), id(4), id(5)];
  for (const [s, n] of [[a, "t:a"], [b, "t:b"], [outside, "t:outside"]]) await publish(s, n);
  const watch = "w".repeat(22);
  await fetch(at(`/api/live/${a}/people`), { method: "POST", headers: auth, body: JSON.stringify({ secret: watch, who: "", can_act: false, days: 1 }) });
  const { pin: code } = await pin(a);
  await pin(b);
  const ana = await join(code, "ana@mail.test");

  assert.equal((await post("/room", { from: a + watch, to: b })).status, 403, "the view-only link");
  assert.equal((await post("/room", { from: a + "x".repeat(22), to: b })).status, 403, "a link that does not work");
  assert.equal((await post("/room", { from: ana, to: outside })).status, 403, "a jelly not in the room");

  // Ending the PIN from any jelly in the room ends it for all of them.
  const page = await open(ana);
  assert.equal((await unpin(b)).status, 204);
  assert.deepEqual((await page.next()).room, []);
  assert.equal((await post("/room", { from: ana, to: b })).status, 403);
  assert.equal(await join(code, "bo@mail.test"), "That PIN does not work. Check it with whoever gave it to you.");
  assert.equal((await unpin(a)).status, 400, "nothing left to end");
  page.ws.close();
});

test("an unshared jelly leaves the room, and the last one out ends the PIN", async () => {
  const [a, b] = [id(6), id(7)];
  await publish(a, "t:a");
  await publish(b, "t:b");
  const { pin: code } = await pin(a);
  await pin(b);
  assert.equal((await fetch(at(`/api/live/${b}`), { method: "DELETE", headers: auth })).status, 200);
  assert.ok((await join(code, "ana@mail.test")).startsWith(a));
  assert.equal((await fetch(at(`/api/live/${a}`), { method: "DELETE", headers: auth })).status, 200);
  // A new --pin makes a new room rather than joining the ended one.
  const c = id(8);
  await publish(c, "t:c");
  assert.match((await pin(c)).pin, /^\d{6}$/);
  assert.equal((await unpin(c)).status, 204);
});

test("ten wrong PINs lock the room's PIN, and every jelly in it hears so", async () => {
  const [a, b] = [id(9), id(10)];
  await publish(a, "t:a");
  await publish(b, "t:b");
  const { pin: code } = await pin(a);
  await pin(b);
  const wrong = String((Number(code) + 1) % 1_000_000).padStart(6, "0");
  // One IP gets 5 tries per window, so the lock takes two.
  for (let i = 0; i < 10; i++) await join(wrong, "eve@mail.test", `10.0.1.${i % 2}`);
  assert.equal(await join(code, "ana@mail.test", "10.0.2.1"), "That PIN does not work. Check it with whoever gave it to you.");
  for (const share of [a, b]) {
    const { events } = await (await fetch(at(`/api/live/${share}?since=0`), { method: "DELETE", headers: auth })).json();
    assert.deepEqual(events.filter((e) => e.kind === "lock").map((e) => e.args), [{ tries: 10 }], share);
  }
});
