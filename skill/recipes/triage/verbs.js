// A triage: the agent has sorted the items into lanes, the person corrects.
// An item's `lane` is where it is now; `agent_lane` is where the agent put it,
// so the page can say "you moved this from Later". The order of `items` is
// the order inside each lane.
const item = (state, id) => {
  const it = (state.items || []).find((i) => i.id === id);
  if (!it) throw new Error(`no item ${id}`);
  return it;
};
const lane = (state, id) => {
  const l = (state.lanes || []).find((l) => l.id === id);
  if (!l) throw new Error(`no lane ${id}; this triage has: ${(state.lanes || []).map((l) => l.id).join(", ")}`);
  return l;
};
const words = (text, what) => {
  const t = String(text || "").trim();
  if (!t) throw new Error(`say ${what}`);
  return t;
};
// A lane someone accepted stays accepted when that same someone changes it.
const touch = (state, id, by) => {
  if (state.accepted?.[id] && state.accepted[id].by !== by) delete state.accepted[id];
  state.go = null;
};
// Puts `it` before the item `before`, or at the end of its lane.
const place = (state, it, before) => {
  const rest = state.items.filter((i) => i !== it);
  let at = before == null ? -1 : rest.findIndex((i) => i.id === before);
  if (before != null && at < 0) throw new Error(`no item ${before} to put it before`);
  if (at < 0) {
    for (let i = rest.length - 1; i >= 0; i--) if (rest[i].lane === it.lane) { at = i + 1; break; }
    if (at < 0) at = rest.length;
  }
  rest.splice(at, 0, it);
  state.items = rest;
};

const verbs = {
  // ---- the person ----
  // To another lane, or to another place in the same lane ({ before: item id }).
  move(state, { item: id, lane: to, before }, { by, at_ms }) {
    const it = item(state, id);
    const from = it.lane;
    to = lane(state, to || from).id;
    it.lane = to;
    it.checked = { by, at_ms };
    if (to === it.agent_lane) it.moved = null;
    else if (to !== from) it.moved = { from: it.agent_lane, by, at_ms, note: "" };
    place(state, it, before);
    touch(state, from, by);
    touch(state, to, by);
  },
  // The agent was unsure, and was right.
  keep(state, { item: id }, { by, at_ms }) {
    item(state, id).checked = { by, at_ms };
  },
  // Why they moved it. Optional, and only for something they moved.
  note(state, { item: id, note }) {
    const it = item(state, id);
    if (!it.moved) throw new Error(`nothing to explain: ${id} is where the agent put it`);
    it.moved.note = words(note, "why you moved it");
  },
  // This lane is sorted.
  accept(state, { lane: id }, { by, at_ms }) {
    state.accepted = state.accepted || {};
    state.accepted[lane(state, id).id] = { by, at_ms };
  },
  reopen(state, { lane: id }) {
    if (state.accepted) delete state.accepted[lane(state, id).id];
    state.go = null;
  },
  // Every lane is sorted: do the next step.
  go(state, _args, { by, at_ms }) {
    const open = (state.lanes || []).filter((l) => !state.accepted?.[l.id]);
    if (open.length) throw new Error(`not yet: ${open.map((l) => l.label || l.id).join(", ")} ${open.length === 1 ? "is" : "are"} not accepted as sorted`);
    state.go = { by, at_ms };
  },

  // ---- the agent ----
  // { items: [{ id, title, lane, reason, confidence (0 to 1), impact, effort, ref }] }
  add(state, { items }, { by, at_ms }) {
    if (!Array.isArray(items) || !items.length) throw new Error("give items: [{ id, title, lane, reason, confidence }]");
    state.items = state.items || [];
    for (const raw of items) {
      const id = words(raw.id, "an id for each item");
      if (state.items.some((i) => i.id === id)) throw new Error(`there is already an item ${id}`);
      if (!(raw.confidence >= 0 && raw.confidence <= 1)) throw new Error(`${id}: confidence is a number from 0 (a guess) to 1 (certain)`);
      const it = { ...raw, id, title: words(raw.title, `a title for ${id}`), lane: lane(state, raw.lane).id, reason: words(raw.reason, `why ${id} goes there`), added: { by, at_ms } };
      it.agent_lane = it.lane;
      it.moved = null; it.checked = null; it.done = null;
      state.items.push(it);
      place(state, it, null);
      touch(state, it.lane, by);
    }
  },
  // The agent changed its mind. Refused once the person has moved the item:
  // their say stands, so ask them instead.
  resort(state, { item: id, lane: to, reason, confidence }, { by }) {
    const it = item(state, id);
    if (it.moved) throw new Error(`${it.moved.by} moved ${id} to ${it.lane}; ask them instead of sorting it again`);
    const from = it.lane;
    it.lane = it.agent_lane = lane(state, to).id;
    it.reason = words(reason, "why it goes there now");
    if (confidence !== undefined) {
      if (!(confidence >= 0 && confidence <= 1)) throw new Error("confidence is a number from 0 (a guess) to 1 (certain)");
      it.confidence = confidence;
    }
    it.checked = null;
    place(state, it, null);
    touch(state, from, by);
    touch(state, it.lane, by);
  },
  // The item is dealt with ("Fixed in ef60c62").
  done(state, { item: id, result }, { by, at_ms }) {
    item(state, id).done = { result: words(result, "what came of it"), by, at_ms };
  },
  // The next step is done.
  applied(state, { note }, { by, at_ms }) {
    state.applied = { note: note || "", by, at_ms };
  },
};
