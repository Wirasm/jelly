// The pages a link or a PIN opens, where they are served from, and taking a
// share down, checked against the real Worker running locally: `npm test`
// (wrangler starts it; nothing is deployed).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { createTestHarness } from "wrangler";

const SECRET = "test-secret";
const auth = { Authorization: `Bearer ${SECRET}` };
const config = new URL("../wrangler.jsonc", import.meta.url);
// One relay as it is deployed today, and one with both settings: shared pages
// on their own origin, and a place to report one.
const PAGES = "http://pages.test";
const REPORT = "mailto:abuse@example.com?subject=Report%20share%20";
const plain = createTestHarness({ workers: [{ configPath: config, vars: { RELAY_SECRET: SECRET } }] });
const split = createTestHarness({ workers: [{ configPath: config, vars: { RELAY_SECRET: SECRET, SHARE_ORIGIN: `${PAGES}/`, REPORT_URL: REPORT } }] });
let url, api;
before(async () => { ({ url } = await plain.listen()); ({ url: api } = await split.listen()); });
after(async () => { await plain.close(); await split.close(); });

const SHARE = "a".repeat(22);
const LINK = "s".repeat(22);
const VIEW = "<!doctype html><title>the view</title><p>the view's own words</p>";

/** Shares a jelly on the relay at `base` and returns what minting its view-only link answered. */
async function publish(base) {
  const doc = { name: "t:plan", about: "", view: VIEW, verbs: "const verbs = {}", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  const r = await fetch(new URL(`/api/live/${SHARE}`, base), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) });
  assert.equal(r.status, 200);
  const m = await fetch(new URL(`/api/live/${SHARE}/people`, base), { method: "POST", headers: auth, body: JSON.stringify({ secret: LINK, who: "", can_act: false, days: 1 }) });
  assert.equal(m.status, 200);
  return m.json();
}

const page = async (path) => (await fetch(new URL(path, url))).text();

/** GET `path` as a browser on the share origin sends it: the same Worker, another Host. */
const onPages = (path) => new Promise((resolve, reject) => {
  http.get(new URL(path, api), { headers: { Host: new URL(PAGES).host } }, (res) => {
    let body = "";
    res.on("data", (d) => { body += d; });
    res.on("end", () => resolve({ status: res.statusCode, body }));
  }).on("error", reject);
});

test("every page a link or a PIN opens says Made with jelly, in the relay's own page", async () => {
  assert.equal((await publish(url)).share_origin, undefined);
  const mark = '<a class="made" href="https://getjellyware.com" target="_blank" rel="noopener">Made with jelly</a>';
  const link = await page(`/j/${SHARE}${LINK}`);
  assert.ok(link.includes(mark), "the link page");
  // The view arrives over the socket and goes into the sandboxed iframe: the
  // mark is not in it, so no view can take it out.
  assert.ok(!link.includes("the view's own words"));
  // No REPORT_URL, no Report link.
  assert.ok(!link.includes(">Report<"));
  assert.ok((await page("/pin")).includes(mark), "the PIN page");
  assert.ok((await page(`/j/${SHARE}${"x".repeat(22)}`)).includes(mark), "a link that does not work");
});

test("the holder of the publish secret takes a share down by its id, and it stays down", async () => {
  await publish(url);
  const share = new URL(`/api/live/${SHARE}`, url);
  assert.equal((await fetch(share, { method: "DELETE" })).status, 401);
  assert.equal((await fetch(new URL(`/j/${SHARE}${LINK}`, url))).status, 200);
  assert.equal((await fetch(share, { method: "DELETE", headers: auth })).status, 200);
  assert.equal((await fetch(new URL(`/j/${SHARE}${LINK}`, url))).status, 404);
  // Nothing of it is left: the daemon that made it hears 404, and cannot mint on it again.
  const mint = await fetch(new URL(`/api/live/${SHARE}/people`, url), { method: "POST", headers: auth, body: JSON.stringify({ secret: LINK, who: "", can_act: false, days: 1 }) });
  assert.equal(mint.status, 404);
});

test("with SHARE_ORIGIN set, pages are served there only and the daemon is told where links live", async () => {
  assert.equal((await publish(api)).share_origin, PAGES);
  // On the host the daemon publishes to, everything a person opens goes on to the share origin.
  for (const path of [`/j/${SHARE}${LINK}`, "/pin", `/a/${SHARE}/assets/logo.png`]) {
    const r = await fetch(new URL(path, api), { redirect: "manual" });
    assert.equal(r.status, 308, path);
    assert.equal(r.headers.get("Location"), PAGES + path);
  }
  const link = await onPages(`/j/${SHARE}${LINK}`);
  assert.equal(link.status, 200);
  // The Report link names the share by its id, which opens nothing, never by the link.
  assert.ok(link.body.includes(`href="mailto:abuse@example.com?subject=Report%20share%20${SHARE}"`));
  assert.ok(!link.body.includes(LINK));
  assert.equal((await onPages("/pin")).status, 200);
});
