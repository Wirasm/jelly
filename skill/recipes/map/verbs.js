// A map of a system. The agent draws it (layers, nodes, edges, traces) with
// `jelly set`; these verbs are what happens to it afterwards. The person flags
// nodes and marks traces understood; the agent answers and marks what a change
// touches.
const node = (state, id) => {
  const n = (state.nodes || []).find((n) => n.id === id);
  if (!n) throw new Error(`no node ${id}; this map has: ${(state.nodes || []).map((n) => n.id).join(", ")}`);
  return n;
};
const trace = (state, id) => {
  const t = (state.traces || []).find((t) => t.id === id);
  if (!t) throw new Error(`no trace ${id}; this map has: ${(state.traces || []).map((t) => t.id).join(", ") || "none"}`);
  return t;
};
const flagOf = (state, id) => {
  const f = (state.flags || []).find((f) => f.id === id);
  if (!f) throw new Error(`no flag ${id}; open flags: ${(state.flags || []).filter((f) => !f.reply).map((f) => f.id).join(", ") || "none"}`);
  return f;
};
const KINDS = ["wrong", "more", "careful"];

const verbs = {
  // ---- the person ----
  // "This is wrong", "tell me more" or "careful here", on one node.
  flag(state, { node: id, kind, note }, { by, at_ms }) {
    node(state, id);
    if (!KINDS.includes(kind)) throw new Error(`kind is one of: ${KINDS.join(", ")}`);
    const text = String(note || "").trim();
    if (kind === "wrong" && !text) throw new Error("say what is wrong");
    state.flags ||= [];
    state.flag_seq = (state.flag_seq || 0) + 1;
    state.flags.push({ id: `f${state.flag_seq}`, node: id, kind, note: text, by, at_ms, reply: null });
  },
  unflag(state, { flag }) {
    flagOf(state, flag);
    state.flags = state.flags.filter((f) => f.id !== flag);
  },
  // They followed a trace to the end and it makes sense ({ yes: false } takes it back).
  understood(state, { trace: id, yes }, { by, at_ms }) {
    trace(state, id).understood = yes === false ? null : { by, at_ms };
  },

  // ---- the agent ----
  // Answer a flag. With `fix`, the node's own text changes too:
  // { what, risk, files, label } — whatever was wrong.
  answer(state, { flag, note, fix }, { by, at_ms }) {
    const f = flagOf(state, flag);
    const text = String(note || "").trim();
    if (!text) throw new Error("say what the answer is, or what you changed");
    f.reply = { note: text, by, at_ms };
    if (fix) {
      const n = node(state, f.node);
      for (const k of ["what", "risk", "files", "label"]) if (fix[k] !== undefined) n[k] = fix[k];
      f.fixed = true;
    }
  },
  // Lay a change over the map: the nodes it touches, and one line on each.
  // { title, note, nodes: [{ id, note }] }. Replaces the last overlay.
  touch(state, { title, note, nodes }, { by, at_ms }) {
    if (!Array.isArray(nodes) || !nodes.length) throw new Error("give nodes: [{ id, note }]");
    for (const t of nodes) node(state, t.id);
    for (const n of state.nodes) delete n.touched;
    for (const t of nodes) node(state, t.id).touched = { note: t.note || "" };
    state.change = { title: String(title || "").trim() || "A planned change", note: note || "", by, at_ms };
  },
  untouch(state) {
    for (const n of state.nodes || []) delete n.touched;
    state.change = null;
  },
  // A named path through the map. steps: [{ at: <node id>, say, cite, show }].
  add_trace(state, { id, title, steps }) {
    if (!id || !/^[a-z0-9-]+$/.test(id)) throw new Error("give the trace an id of lowercase letters, digits and dashes");
    if (!String(title || "").trim()) throw new Error("give the trace a title: what happens when…");
    if (!Array.isArray(steps) || steps.length < 2) throw new Error("a trace needs at least two steps");
    steps.forEach((s, i) => {
      node(state, s.at);
      if (!String(s.say || "").trim()) throw new Error(`step ${i + 1} needs a line in say`);
    });
    state.traces ||= [];
    state.traces = state.traces.filter((t) => t.id !== id);
    state.traces.push({ id, title, steps, understood: null });
  },
};
