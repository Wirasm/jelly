// A live plan: the agent works through the steps, the person steers.
// A step's `status` is next, doing, done or blocked. A veto and a gate are the
// person's marks on a step, kept beside its status.
const name = (s) => `"${s.title}"`;
const find = (state, id) => {
  const s = (state.steps || []).find((s) => s.id === id);
  if (!s) throw new Error(`no step ${id}; this plan has: ${(state.steps || []).map((s) => s.id).join(", ")}`);
  return s;
};
const words = (text, what) => {
  const t = String(text || "").trim();
  if (!t) throw new Error(`say ${what}`);
  return t;
};
const quote = (note) => (note ? `: "${note.replace(/[.!\s]+$/, "")}"` : "");
const who = (by) => (by === "person" ? "the person" : by);
const started = (s) => s.status !== "next";
// Needs must exist, and nothing may need itself, directly or through others.
const checkNeeds = (state, id, needs) => {
  const reaches = (from, seen = []) => from === id || (!seen.includes(from) && (find(state, from).needs || []).some((n) => reaches(n, [...seen, from])));
  for (const n of needs) {
    const dep = find(state, n);
    if (n === id || reaches(n)) throw new Error(`${name(dep)} already depends on this step, so it cannot also come first`);
  }
};
// The order on the page is the order of work: a step comes after what it needs.
const checkOrder = (state) => {
  const at = Object.fromEntries(state.steps.map((s, i) => [s.id, i]));
  for (const s of state.steps) for (const n of s.needs || []) {
    if (at[n] > at[s.id]) throw new Error(`${name(s)} needs ${name(find(state, n))}, so it stays after it`);
  }
};
const place = (state, s, before) => {
  if (before == null) return state.steps.push(s);
  const j = state.steps.findIndex((t) => t.id === before);
  if (j < 0) throw new Error(`no step ${before} to put it before`);
  state.steps.splice(j, 0, s);
};

const verbs = {
  // ---- the person steers ----
  // Reorder a step that has not started: { step, dir: "up" | "down" }, or
  // { step, before: <step id> } with before: null for the end.
  move(state, { step, dir, before }, { by, at_ms }) {
    const s = find(state, step);
    if (started(s)) throw new Error(`${name(s)} ${s.status === "done" ? "is done" : "has already started"}, so it stays where it is`);
    const i = state.steps.indexOf(s);
    state.steps.splice(i, 1);
    if (dir) {
      const j = dir === "up" ? i - 1 : i + 1;
      if (j < 0) throw new Error(`${name(s)} is already first`);
      if (j > state.steps.length) throw new Error(`${name(s)} is already last`);
      state.steps.splice(j, 0, s);
    } else place(state, s, before === step ? null : before);
    checkOrder(state);
    s.moved = { by, at_ms };
  },
  // "Do not do this." A step in progress stops and goes back to not started.
  veto(state, { step, note }, { by, at_ms }) {
    const s = find(state, step);
    if (s.status === "done") throw new Error(`${name(s)} is already done; there is nothing left to veto`);
    s.vetoed = { by, at_ms, note: String(note || "").trim(), stopped: s.status === "doing" };
    if (s.status === "doing") { s.status = "next"; s.started = null; }
  },
  unveto(state, { step }) {
    const s = find(state, step);
    if (!s.vetoed) throw new Error(`${name(s)} is not vetoed`);
    s.vetoed = null;
  },
  // "Check with me before this." The agent may set one too; only the person clears it.
  gate(state, { step, note }, { by, at_ms }) {
    const s = find(state, step);
    if (started(s)) throw new Error(`${name(s)} ${s.status === "done" ? "is done" : "has already started"}; a gate goes on a step before it starts`);
    s.gate = { by, at_ms, note: String(note || "").trim(), cleared: null };
  },
  clear_gate(state, { step }, { by, at_ms }) {
    const s = find(state, step);
    if (!s.gate) throw new Error(`${name(s)} has no gate`);
    if (by !== "person") throw new Error(`only the person clears a gate; ${name(s)} waits for them`);
    s.gate.cleared = { by, at_ms };
  },
  // A word on a step, from anyone.
  note(state, { step, text }, { by, at_ms }) {
    const s = find(state, step);
    (s.notes = s.notes || []).push({ text: words(text, "what the note is"), by, at_ms });
  },

  // ---- the agent works ----
  start(state, { step }, { by, at_ms }) {
    const s = find(state, step);
    if (s.status === "done") throw new Error(`${name(s)} is already done`);
    if (s.status === "doing") throw new Error(`${name(s)} is already in progress`);
    if (s.vetoed) throw new Error(`${name(s)} was vetoed by ${who(s.vetoed.by)}${quote(s.vetoed.note)}. Leave it, or ask them to bring it back`);
    if (s.gate && !s.gate.cleared) throw new Error(`${name(s)} waits for the person to clear its gate${quote(s.gate.note)}. Ask them; do not start it`);
    const open = (s.needs || []).map((n) => find(state, n)).filter((d) => d.status !== "done");
    if (open.length) throw new Error(`${name(s)} needs ${open.map(name).join(" and ")} first`);
    s.status = "doing";
    s.started = { by, at_ms };
    s.blocked = null;
  },
  // Still on it: what is happening now, and a picture when there is one.
  progress(state, { step, text, show }, { by, at_ms }) {
    const s = find(state, step);
    if (s.status !== "doing") throw new Error(`${name(s)} is not in progress; start it first`);
    s.progress = { text: words(text, "what is happening now"), show: show || null, by, at_ms };
  },
  // Done, with what happened in one line and, when there is something to see, a picture of it.
  finish(state, { step, text, show }, { by, at_ms }) {
    const s = find(state, step);
    if (s.status === "done") throw new Error(`${name(s)} is already done`);
    if (s.status !== "doing") throw new Error(`${name(s)} was never started; start it first, so the person sees it happen`);
    s.result = { text: words(text, "what happened, in one line"), show: show || null, by, at_ms };
    s.status = "done";
  },
  block(state, { step, reason }, { by, at_ms }) {
    const s = find(state, step);
    if (s.status === "done") throw new Error(`${name(s)} is already done`);
    s.blocked = { reason: words(reason, "what blocks it and what would unblock it"), by, at_ms };
    s.status = "blocked";
  },
  // A new step: { id, title, why, needs: [...], before: <step id> } (before: leave out for the end).
  add(state, { id, title, why, needs, before }, { by, at_ms }) {
    state.steps = state.steps || [];
    const sid = words(id, "the new step's id");
    if (!/^[a-z0-9][a-z0-9-]*$/.test(sid)) throw new Error(`a step id is lowercase letters, digits and dashes; got "${sid}"`);
    if (state.steps.some((s) => s.id === sid)) throw new Error(`there is already a step ${sid}`);
    const s = { id: sid, title: words(title, "the step's title"), why: String(why || ""), needs: needs || [], status: "next", added: { by, at_ms } };
    checkNeeds(state, sid, s.needs);
    place(state, s, before);
    checkOrder(state);
  },
  // Change a step that is not done: { step, title, why, needs }.
  revise(state, { step, title, why, needs }, { by, at_ms }) {
    const s = find(state, step);
    if (s.status === "done") throw new Error(`${name(s)} is done; add a new step instead of rewriting what happened`);
    if (title !== undefined) s.title = words(title, "the step's title");
    if (why !== undefined) s.why = String(why);
    if (needs !== undefined) { checkNeeds(state, s.id, needs); s.needs = needs; checkOrder(state); }
    s.revised = { by, at_ms };
  },
  remove(state, { step }) {
    const s = find(state, step);
    if (started(s)) throw new Error(`${name(s)} ${s.status === "done" ? "is done" : "has already started"}; it stays in the plan`);
    const users = state.steps.filter((t) => (t.needs || []).includes(s.id));
    if (users.length) throw new Error(`${users.map(name).join(" and ")} need${users.length === 1 ? "s" : ""} ${name(s)}; revise ${users.length === 1 ? "it" : "them"} first`);
    state.steps.splice(state.steps.indexOf(s), 1);
  },
  // The plan is over: one line on where things stand.
  wrap(state, { text }, { by, at_ms }) {
    state.wrap = { text: words(text, "where things stand"), by, at_ms };
  },
};
