// The pages a link or a PIN opens, checked against the real Worker running
// locally: `npm test` (wrangler starts it; nothing is deployed).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createTestHarness } from "wrangler";

const SECRET = "test-secret";
const auth = { Authorization: `Bearer ${SECRET}` };
const server = createTestHarness({ workers: [{ configPath: new URL("../wrangler.jsonc", import.meta.url), vars: { RELAY_SECRET: SECRET } }] });
let url;
before(async () => { ({ url } = await server.listen()); });
after(() => server.close());

const SHARE = "a".repeat(22);
const LINK = "s".repeat(22);
const VIEW = "<!doctype html><title>the view</title><p>the view's own words</p>";

async function publish() {
  const doc = { name: "t:plan", about: "", view: VIEW, verbs: "const verbs = {}", state: {}, version: 1, view_rev: 1, comments: [], asks: [] };
  const r = await fetch(new URL(`/api/live/${SHARE}`, url), { method: "PUT", headers: auth, body: JSON.stringify({ doc, runtime: "" }) });
  assert.equal(r.status, 200);
  const m = await fetch(new URL(`/api/live/${SHARE}/people`, url), { method: "POST", headers: auth, body: JSON.stringify({ secret: LINK, who: "", can_act: false, days: 1 }) });
  assert.equal(m.status, 200);
}

const page = async (path) => (await fetch(new URL(path, url))).text();

test("every page a link or a PIN opens says Made with jelly, in the relay's own page", async () => {
  await publish();
  const mark = '<a class="made" href="https://getjellyware.com" target="_blank" rel="noopener">Made with jelly</a>';
  const link = await page(`/j/${SHARE}${LINK}`);
  assert.ok(link.includes(mark), "the link page");
  // The view arrives over the socket and goes into the sandboxed iframe: the
  // mark is not in it, so no view can take it out.
  assert.ok(!link.includes("the view's own words"));
  assert.ok((await page("/pin")).includes(mark), "the PIN page");
  assert.ok((await page(`/j/${SHARE}${"x".repeat(22)}`)).includes(mark), "a link that does not work");
});
