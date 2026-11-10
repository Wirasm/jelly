// A scope: items that are in or out, a few levers, and what that adds up to.
//
// `model` does all the sums. It is pure: state in, result out. Every verb ends
// by writing `state.result = model(state)`, and the view runs the very same
// function (it reads this file from the daemon) to redraw while a slider moves.
// Change the arithmetic here and both follow.
//
// Lever roles the model knows:
//   pace    units of work done per week (per person)
//   people  how many work at once (default 1)
//   window  calendar days from `state.start` to the target
//   buffer  percent added on top of the cost of what is in
//   budget  the capacity itself, when there is no calendar (money, points)
function model(state) {
  const items = state.items || [], levers = state.levers || [], rec = state.recommended || null;
  const unit = state.unit || "";
  const r2 = (n) => Math.round(n * 100) / 100;
  const amount = (n) => `${r2(Math.abs(n))}${unit ? " " + unit : ""}`;
  const DAY = 86400000;
  const WD = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const MO = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const at = (iso) => Date.parse(iso + "T00:00:00Z");
  const iso = (ms) => new Date(ms).toISOString().slice(0, 10);
  const nice = (d) => { const t = new Date(at(d)); return `${WD[t.getUTCDay()]} ${t.getUTCDate()} ${MO[t.getUTCMonth()]}`; };
  const days = (n) => `${Math.abs(n)} day${Math.abs(n) === 1 ? "" : "s"}`;
  const by = {};
  for (const i of items) by[i.id] = i;

  // One plan: which items are wanted, and what each lever is set to.
  const run = (wants, set) => {
    const lever = (role) => { const l = levers.find((l) => l.role === role); return l ? Number(set(l)) : undefined; };
    // An item counts when it is wanted and everything it needs counts too.
    const memo = {};
    const live = (id, path) => {
      if (id in memo) return memo[id];
      const it = by[id];
      if (!it) return false;
      if (it.done) return (memo[id] = true);
      if (!wants(it) || path.includes(id)) return false;
      return (memo[id] = (it.needs || []).every((n) => live(n, path.concat(id))));
    };
    const p = { in: [], out: [], fell: [], done: [] };
    let cost = 0, value = 0, value_total = 0;
    for (const it of items) {
      value_total += it.value || 0;
      if (it.done) { p.done.push(it.id); value += it.value || 0; }
      else if (live(it.id, [])) { p.in.push(it.id); cost += it.cost || 0; value += it.value || 0; }
      else if (wants(it)) p.fell.push({ id: it.id, with: (it.needs || []).find((n) => !live(n, [])) });
      else p.out.push(it.id);
    }
    const buffer = r2(cost * (lever("buffer") || 0) / 100), load = r2(cost + buffer);
    const pace = lever("pace"), people = lever("people") || 1, window = lever("window");
    let capacity = lever("budget") ?? state.capacity ?? null;
    let date = null, deadline = null, took = null, late = null;
    if (pace && state.start) {
      const rate = pace * people;
      took = Math.ceil(r2(load / rate * 7));
      date = iso(at(state.start) + took * DAY);
      const win = window ?? (state.deadline ? Math.round((at(state.deadline) - at(state.start)) / DAY) : null);
      if (win != null) { deadline = iso(at(state.start) + win * DAY); capacity = r2(rate * win / 7); late = took - win; }
    }
    const spare = capacity == null ? null : r2(capacity - load);
    return { ...p, cost: r2(cost), buffer, load, capacity, spare, fits: spare == null || spare >= 0, value, value_total, date, days: took, deadline, late };
  };

  const cur = run((i) => i.in === true, (l) => l.value);
  const ghost = rec ? run((i) => (rec.items || []).includes(i.id), (l) => (rec.levers && l.id in rec.levers ? rec.levers[l.id] : l.value)) : null;
  let vs = null;
  if (ghost) {
    const moved = levers.filter((l) => rec.levers && l.id in rec.levers && rec.levers[l.id] !== l.value).map((l) => l.id);
    const added = cur.in.filter((id) => !ghost.in.includes(id)), dropped = ghost.in.filter((id) => !cur.in.includes(id));
    vs = { same: !moved.length && !added.length && !dropped.length, added, dropped, levers: moved,
      load: r2(cur.load - ghost.load), value: cur.value - ghost.value, days: cur.days == null || ghost.days == null ? null : cur.days - ghost.days };
  }
  // One sentence the agent can read without redoing anything.
  const fit = cur.spare == null ? "" : cur.fits ? `fits with ${amount(cur.spare)} spare` : `${amount(cur.spare)} over`;
  const summary = (cur.date
    ? `Lands ${nice(cur.date)}${fit ? `: ${fit}` : ""}${cur.late > 0 ? `, ${days(cur.late)} after the target` : ""}`
    : `${amount(cur.load)}${cur.capacity == null ? "" : ` of ${amount(cur.capacity)}`}${fit ? `: ${fit}` : ""}`)
    + ` · ${cur.in.length} in, ${cur.out.length + cur.fell.length} out, ${cur.done.length} done · value ${cur.value} of ${cur.value_total}`;
  return { ...cur, unit, date_text: cur.date ? nice(cur.date) : null, deadline_text: cur.deadline ? nice(cur.deadline) : null,
    rec: ghost && { in: ghost.in, load: ghost.load, capacity: ghost.capacity, spare: ghost.spare, fits: ghost.fits, value: ghost.value, date: ghost.date, date_text: ghost.date ? nice(ghost.date) : null, days: ghost.days },
    vs, summary };
}

const item = (state, id) => {
  const it = (state.items || []).find((i) => i.id === id);
  if (!it) throw new Error(`no item ${id}; this scope has: ${(state.items || []).map((i) => i.id).join(", ")}`);
  return it;
};
const unlocked = (state) => { if (state.locked) throw new Error("the plan is locked; unlock it to change it"); };
// After every change: redo the sums, and reopen a locked plan that no longer fits.
const settle = (state, why, { by, at_ms }) => {
  state.result = model(state);
  if (state.locked && !state.result.fits) {
    state.locked = null;
    state.reopened = { why: `${why}, and the locked plan no longer fits`, by, at_ms };
  }
};

const verbs = {
  // ---- the person (and the agent, on their behalf) ----
  // Put an item in or take it out. Putting one in brings what it needs with it;
  // taking one out leaves what needs it wanted, so putting it back restores them.
  put(state, { item: id, in: want }, ctx) {
    unlocked(state);
    const it = item(state, id);
    if (it.done) throw new Error(`${id} is done; it is in whatever you choose`);
    if (typeof want !== "boolean") throw new Error("in is true or false");
    const pulled = [];
    const set = (x) => {
      if (x.in !== true && !x.done) { x.in = true; x.set = { by: ctx.by, at_ms: ctx.at_ms }; if (x !== it) pulled.push(x.id); }
      for (const n of x.needs || []) set(item(state, n));
    };
    if (want) set(it); else { it.in = false; it.set = { by: ctx.by, at_ms: ctx.at_ms }; }
    state.last = { what: want ? "in" : "out", item: id, pulled, by: ctx.by, at_ms: ctx.at_ms };
    state.reopened = null;
    settle(state, "", ctx);
  },
  // Move a lever. The value snaps to the lever's step and must be in its range.
  lever(state, { id, value }, ctx) {
    unlocked(state);
    const l = (state.levers || []).find((l) => l.id === id);
    if (!l) throw new Error(`no lever ${id}; this scope has: ${(state.levers || []).map((l) => l.id).join(", ")}`);
    if (typeof value !== "number" || !isFinite(value)) throw new Error("value is a number");
    if (value < l.min || value > l.max) throw new Error(`${l.label || id} goes from ${l.min} to ${l.max}`);
    const step = l.step || 1;
    l.value = Math.round((l.min + Math.round((value - l.min) / step) * step) * 1000) / 1000;
    l.set = { by: ctx.by, at_ms: ctx.at_ms };
    state.last = { what: "lever", lever: id, value: l.value, by: ctx.by, at_ms: ctx.at_ms };
    state.reopened = null;
    settle(state, "", ctx);
  },
  // Back to the agent's cut: its items and its lever values.
  reset(state, _args, ctx) {
    unlocked(state);
    const rec = state.recommended;
    if (!rec) throw new Error("there is no recommended cut to go back to");
    for (const it of state.items || []) it.in = (rec.items || []).includes(it.id);
    for (const l of state.levers || []) if (rec.levers && l.id in rec.levers) l.value = rec.levers[l.id];
    state.last = { what: "reset", by: ctx.by, at_ms: ctx.at_ms };
    state.reopened = null;
    settle(state, "", ctx);
  },
  // This is the plan. Refused while it does not fit.
  lock(state, _args, { by, at_ms }) {
    if (state.locked) throw new Error("the plan is already locked");
    const r = (state.result = model(state));
    if (!r.in.length) throw new Error("nothing is in: put something in first");
    if (!r.fits) throw new Error(`does not fit: ${Math.abs(r.spare)}${r.unit ? " " + r.unit : ""} over${r.late > 0 ? `, landing ${r.late} day${r.late === 1 ? "" : "s"} after the target` : ""}. Take something out or move a lever`);
    state.locked = { by, at_ms, in: r.in, load: r.load, date: r.date, value: r.value, summary: r.summary };
    state.reopened = null;
  },
  unlock(state) {
    if (!state.locked) throw new Error("the plan is not locked");
    state.locked = null;
  },

  // ---- the agent ----
  // Add an item, or change one: a new estimate keeps the old one in `was`.
  item(state, { id, short, label, cost, value, group, needs, why, in: want }, ctx) {
    if (!id) throw new Error("give an id");
    state.items = state.items || [];
    let it = state.items.find((i) => i.id === id);
    const fresh = !it;
    if (fresh) {
      if (!label) throw new Error("a new item needs a label");
      if (typeof cost !== "number" || typeof value !== "number") throw new Error("a new item needs a cost and a value");
      it = { id, in: false };
    }
    if (cost !== undefined && (typeof cost !== "number" || cost < 0)) throw new Error("cost is a number, zero or more");
    if (value !== undefined && (typeof value !== "number" || value < 1 || value > 5)) throw new Error("value goes from 1 to 5");
    for (const n of needs || []) {
      if (n === id) throw new Error("an item cannot need itself");
      item(state, n);
    }
    if (!fresh && ((cost !== undefined && cost !== it.cost) || (value !== undefined && value !== it.value))) it.was = { cost: it.cost, value: it.value };
    const next = { short, label, cost, value, group, needs, why, in: want };
    for (const k in next) if (next[k] !== undefined) it[k] = next[k];
    it.set = { by: ctx.by, at_ms: ctx.at_ms };
    if (fresh) state.items.push(it);
    settle(state, `${it.short || id} ${fresh ? "was added" : "changed"}`, ctx);
  },
  // The cut you recommend: item ids and lever values. With nothing, what is set now.
  recommend(state, { items, levers, why }, { by, at_ms }) {
    for (const id of items || []) item(state, id);
    for (const id in levers || {}) if (!(state.levers || []).some((l) => l.id === id)) throw new Error(`no lever ${id}`);
    state.recommended = {
      items: items || (state.items || []).filter((i) => i.in === true && !i.done).map((i) => i.id),
      levers: levers || Object.fromEntries((state.levers || []).map((l) => [l.id, l.value])),
      why: why || "", by, at_ms,
    };
    state.result = model(state);
  },
  // An item is finished: its cost no longer counts, its value is banked.
  done(state, { item: id, done }, ctx) {
    const it = item(state, id);
    it.done = done !== false;
    it.set = { by: ctx.by, at_ms: ctx.at_ms };
    settle(state, `${it.short || id} was marked ${it.done ? "done" : "not done"}`, ctx);
  },
  // Redo the sums after writing state with `jelly set`.
  recount(state, _args, ctx) {
    settle(state, "the numbers changed", ctx);
  },
};
