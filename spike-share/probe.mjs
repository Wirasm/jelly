// Two viewers on one shared jelly: do verbs run as in the daemon, and do changes reach both live?
import WebSocket from "ws";
const base = "http://127.0.0.1:8799/s/quiz-spike-0002";
const call = async (verb, args, by) => { const t = performance.now(); const r = await fetch(base + "/call", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ verb, args, by }) }); const j = await r.json(); return { ok: r.ok, ms: Math.round(performance.now() - t), ...j }; };
const seen = { ana: [], bo: [] };
const open = (who) => new Promise((res) => { const ws = new WebSocket(base.replace("http", "ws") + "/ws"); ws.on("message", (m) => seen[who].push(JSON.parse(m).version)); ws.on("open", () => res(ws)); });
const [a, b] = await Promise.all([open("ana"), open("bo")]);
const out = [];
out.push(["join Ana", await call("join", {}, "person:Ana")]);
out.push(["join Bo", await call("join", {}, "person:Bo")]);
out.push(["person opens (refused)", await call("open", {}, "person:Ana")]);
out.push(["agent opens", await call("open", { seconds: 15 }, "claude")]);
out.push(["Ana votes", await call("vote", { option: 0 }, "person:Ana")]);
out.push(["Ana votes again (refused)", await call("vote", { option: 1 }, "person:Ana")]);
out.push(["Bo votes", await call("vote", { option: 2 }, "person:Bo")]);
out.push(["reveal", await call("reveal", {}, "claude")]);
// 30 calls one after another, then 30 at once, to see the latency and that none are lost.
const seq = []; for (let i = 0; i < 30; i++) seq.push((await call("add", { q: "q" + i, options: ["a", "b"], correct: 0 }, "claude")).ms);
const par = await Promise.all(Array.from({ length: 30 }, (_, i) => call("add", { q: "p" + i, options: ["a", "b"], correct: 0 }, "claude")));
await new Promise((r) => setTimeout(r, 500));
out.push(["runaway verb", await call("__loop", {}, "claude").catch((e) => ({ error: String(e) }))]);
for (const [what, r] of out) console.log(what.padEnd(26), r.ok ? `ok v${r.version} ${r.ms}ms` : `refused: ${r.error}`);
const state = await (await fetch(base + "/state")).json();
const sorted = (x) => [...x].sort((p, q) => p - q);
console.log("scores", JSON.stringify(state.players));
console.log("sequential ms p50/max", sorted(seq)[15], Math.max(...seq), "· parallel 30 all ok:", par.every((r) => r.ok), "versions unique:", new Set(par.map((r) => r.version)).size === 30);
console.log("rounds now", state.rounds.length, "· Ana saw", seen.ana.length, "updates, Bo saw", seen.bo.length, "· same versions:", JSON.stringify(seen.ana) === JSON.stringify(seen.bo));
a.close(); b.close();
