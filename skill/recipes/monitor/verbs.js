// A long run: the driver pushes data in, the person watches and steers.
// The person never stops the run directly. Their verbs write a request into
// `control`; the driver reads it between steps and answers with `ack`. A
// request the driver has not acknowledged yet is plainly "not seen".
const CAP = 240;   // points kept per metric; older ones are averaged in pairs
const ITEMS = 300; // per-item results kept; `tally` counts all of them
const LOG = 40;    // log lines kept
const OVER = ["done", "failed", "stopped"];

const over = (state) => OVER.includes(state.status);
const live = (state, what) => { if (over(state)) throw new Error(`the run is ${state.status}; cannot ${what}`); };
const num = (v, what) => { if (typeof v !== "number" || !isFinite(v)) throw new Error(`${what} must be a number`); return v; };
const metric = (state, id, make) => {
  state.metrics = state.metrics || [];
  let m = state.metrics.find((m) => m.id === id);
  if (!m && make) { m = { id, label: id, unit: "" }; state.metrics.push(m); }
  if (!m) throw new Error(`no metric ${id}; this run has: ${state.metrics.map((m) => m.id).join(", ")}`);
  return m;
};
// Halve the older half by averaging neighbours: recent points stay exact.
const squeeze = (pts) => {
  const half = pts.length >> 1, out = [];
  for (let i = 0; i + 1 < half; i += 2) out.push([Math.round((pts[i][0] + pts[i + 1][0]) / 2), +((pts[i][1] + pts[i + 1][1]) / 2).toFixed(3)]);
  if (half % 2) out.push(pts[half - 1]);
  return out.concat(pts.slice(half));
};
// A request from the person, numbered so the driver can say which it has seen.
const request = (state, { by, at_ms }, want) => {
  const c = (state.control = state.control || { seq: 0 });
  c.seq = (c.seq || 0) + 1;
  if (want) c.requested = { want, seq: c.seq, by, at_ms };
  return c.seq;
};

const verbs = {
  // ---- the person ----
  pause(state, _a, ctx) {
    live(state, "pause it");
    if (state.control?.requested?.want === "pause") throw new Error("already asked to pause");
    request(state, ctx, "pause");
  },
  resume(state, _a, ctx) {
    live(state, "resume it");
    if (state.control?.requested?.want !== "pause") throw new Error("it is not paused");
    request(state, ctx, "run");
  },
  stop(state, _a, ctx) {
    live(state, "stop it");
    if (state.control?.requested?.want === "stop") throw new Error("already asked to stop");
    request(state, ctx, "stop");
  },
  // A limit on a metric: { metric, value }, or value null to remove it.
  threshold(state, { metric: id, value }, ctx) {
    const m = metric(state, id);
    if (value === null) { delete m.threshold; delete m.over; request(state, ctx); return; }
    m.threshold = { value: num(value, "value"), by: ctx.by, at_ms: ctx.at_ms, seq: request(state, ctx) };
    m.over = (m.points || []).filter((p) => p[1] > value).length; // recount against the new limit
  },
  // What the person makes of an anomaly: "expected" or "look"; null takes it back.
  judge(state, { anomaly, kind, note }, ctx) {
    const a = (state.anomalies || []).find((a) => a.id === anomaly);
    if (!a) throw new Error(`no anomaly ${anomaly}`);
    if (kind == null) { delete a.verdict; return; }
    if (!["expected", "look"].includes(kind)) throw new Error("kind is expected or look");
    a.verdict = { kind, note: String(note || ""), by: ctx.by, at_ms: ctx.at_ms, seq: request(state, ctx) };
  },

  // ---- the driver ----
  // One point { m, v, t? } or many { points: [...] }, with optional
  // { items: [{ r: "pass" | "fail" | "slow", label }], done, total }.
  push(state, a, { at_ms }) {
    live(state, "push to it");
    const points = a.points || (a.m ? [a] : []);
    for (const p of points) {
      const m = metric(state, p.m, true), v = num(p.v, `${p.m}.v`);
      m.points = m.points || [];
      m.points.push([p.t || at_ms, v]);
      if (m.points.length > CAP) m.points = squeeze(m.points);
      m.n = (m.n || 0) + 1;
      m.sum = +((m.sum || 0) + v).toFixed(3);
      m.min = m.n === 1 ? v : Math.min(m.min, v);
      m.max = m.n === 1 ? v : Math.max(m.max, v);
      m.last = v;
      if (m.threshold && v > m.threshold.value) m.over = (m.over || 0) + 1;
    }
    for (const it of a.items || []) {
      if (!["pass", "fail", "slow"].includes(it.r)) throw new Error("an item's r is pass, fail or slow");
      state.items = state.items || [];
      state.tally = state.tally || { pass: 0, fail: 0, slow: 0 };
      state.tally[it.r]++;
      state.items.push({ r: it.r, label: String(it.label || "").slice(0, 120) });
      if (state.items.length > ITEMS) state.items.splice(0, state.items.length - ITEMS);
    }
    if (a.total !== undefined) state.total = num(a.total, "total");
    if (a.done !== undefined) {
      state.done = num(a.done, "done");
      // Recent progress, for the time-remaining estimate.
      state.ticks = (state.ticks || []).concat([[at_ms, state.done]]).slice(-12);
    }
    if (!state.started_ms) state.started_ms = at_ms;
    if (state.status === "ready" || !state.status) state.status = "running";
    state.updated_ms = at_ms;
  },
  phase(state, { id }, { at_ms }) {
    live(state, "change its phase");
    if (!(state.phases || []).some((p) => p.id === id)) throw new Error(`no phase ${id}; this run has: ${(state.phases || []).map((p) => p.id).join(", ")}`);
    state.phase = id;
    state.updated_ms = at_ms;
  },
  log(state, { text, level }, { at_ms }) {
    if (!String(text || "").trim()) throw new Error("say what happened");
    state.log = (state.log || []).concat([{ t: at_ms, text: String(text).slice(0, 400), level: level === "warn" || level === "bad" ? level : "" }]).slice(-LOG);
  },
  // Something the person should look at: { title, text, metric?, t?, v?, show? }.
  flag(state, { id, title, text, metric: mid, t, v, show }, { by, at_ms }) {
    if (!String(title || "").trim()) throw new Error("give it a title");
    state.anomalies = state.anomalies || [];
    const aid = id || `a${state.anomalies.length + 1}`;
    if (state.anomalies.some((a) => a.id === aid)) throw new Error(`anomaly ${aid} exists`);
    state.anomalies.push({ id: aid, title: String(title), text: String(text || ""), metric: mid, t: t || at_ms, v, show, by, at_ms });
  },
  // The driver read `control` and has acted on everything up to `seq`.
  ack(state, { seq, note }, { by, at_ms }) {
    const c = state.control || {};
    if (num(seq, "seq") > (c.seq || 0)) throw new Error(`nothing was requested after ${c.seq || 0}`);
    if ((c.acked?.seq || 0) >= seq) throw new Error(`${seq} is already acknowledged`);
    c.acked = { seq, note: String(note || ""), by, at_ms };
    state.control = c;
    const want = c.requested && c.requested.seq <= seq ? c.requested.want : null;
    if (want && !over(state)) {
      state.status = { pause: "paused", run: "running", stop: "stopping" }[want];
      state.ticks = []; // a pause would skew the estimate
    }
  },
  // The run is over: { status: "done" | "failed" | "stopped", headline, text }.
  finish(state, { status, headline, text }, { by, at_ms }) {
    if (over(state)) throw new Error(`the run is already ${state.status}`);
    if (!OVER.includes(status || "done")) throw new Error("status is done, failed or stopped");
    state.status = status || "done";
    state.result = { headline: String(headline || ""), text: String(text || ""), by, at_ms };
    state.finished_ms = at_ms;
  },
};
