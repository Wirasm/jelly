// A timeline: what was seen, in time, and the explanations that compete for it.
// `marks` are the things seen (not jelly's own events); `hypotheses` point at
// the marks that support and contradict them. The agent's `confidence` and
// the person's `verdict` are kept apart: both show on the card.
const need = (v, what) => {
  const t = String(v ?? "").trim();
  if (!t) throw new Error(what);
  return t;
};
const one = (list, id, what) => {
  const x = (list || []).find((x) => x.id === id);
  if (!x) {
    const all = (list || []).map((x) => x.id);
    throw new Error(`no ${what} ${id}; there ${all.length === 1 ? "is" : "are"}: ${all.slice(0, 8).join(", ") || "none"}${all.length > 8 ? ` and ${all.length - 8} more` : ""}`);
  }
  return x;
};
const fresh = (list, prefix) => {
  let n = (list || []).length + 1;
  while ((list || []).some((x) => x.id === prefix + n)) n++;
  return prefix + n;
};
const ids = (state, list, what) => {
  const out = [...new Set(list || [])];
  for (const id of out) one(state.marks, id, `mark (in ${what})`);
  return out;
};
const VERDICTS = ["confirmed", "ruled_out", "next"];
const KINDS = ["ok", "bad", "info"];

const verbs = {
  // ---- the person ----
  // What they believe: confirmed, ruled_out, or next ("check this next").
  judge(state, { hypothesis, verdict, note }, { by, at_ms }) {
    const h = one(state.hypotheses, hypothesis, "hypothesis");
    if (!VERDICTS.includes(verdict)) throw new Error(`verdict is one of: ${VERDICTS.join(", ")}`);
    h.verdict = { kind: verdict, note: String(note || "").trim(), by, at_ms };
  },
  take_back(state, { hypothesis }) {
    one(state.hypotheses, hypothesis, "hypothesis").verdict = null;
  },
  // "This matters."
  pin(state, { mark }, { by, at_ms }) {
    one(state.marks, mark, "mark").pinned = { by, at_ms };
  },
  unpin(state, { mark }) {
    one(state.marks, mark, "mark").pinned = null;
  },
  // Something they know, placed next to a mark that is already there.
  know(state, { lane, rel, mark, text, kind }, { by, at_ms }) {
    const ref = one(state.marks, mark, "mark");
    one(state.lanes, lane, "lane");
    if (!["before", "with", "after"].includes(rel)) throw new Error("rel is before, with or after");
    const label = need(text, "say what happened");
    // Half way to the neighbour on that side, so it lands between the two.
    const end = ref.t + (ref.dur || 0);
    const near = state.marks.map((m) => (rel === "before" ? ref.t - (m.t + (m.dur || 0)) : m.t - end)).filter((d) => d > 0);
    const step = Math.max(1, Math.min(60, near.length ? Math.min(...near) / 2 : 30));
    const t = rel === "before" ? ref.t - step : rel === "after" ? end + step : ref.t;
    const id = fresh(state.marks, "k");
    state.marks.push({ id, lane, t, label, kind: KINDS.includes(kind) ? kind : "info", approx: true,
      when: `${rel === "with" ? "at the same time as" : rel} “${ref.label}”`, by, at_ms });
  },

  // ---- the agent ----
  // Add a mark as you find it, or revise one by id. `t` is seconds from the
  // start of the axis; `dur` makes it a bar.
  mark(state, a, { by, at_ms }) {
    state.marks = state.marks || [];
    const old = a.id && state.marks.find((m) => m.id === a.id);
    const m = old || { id: a.id || fresh(state.marks, "m"), by, at_ms };
    for (const k of ["lane", "t", "dur", "label", "detail", "show", "kind", "when", "approx"]) if (a[k] !== undefined) m[k] = a[k];
    one(state.lanes, m.lane, "lane");
    if (typeof m.t !== "number" || !isFinite(m.t)) throw new Error("t is a number: seconds from the start of the axis");
    if (m.dur != null && !(typeof m.dur === "number" && m.dur >= 0)) throw new Error("dur is a number of seconds, 0 or more");
    need(m.label, "a mark needs a label: a few words");
    if (m.kind && !KINDS.includes(m.kind)) throw new Error(`kind is one of: ${KINDS.join(", ")}`);
    if (!old) state.marks.push(m);
  },
  // Add an explanation, or revise one by id: its wording, your confidence
  // (0 to 100), the marks for and against it, what would settle it. The
  // person's verdict stays.
  hypothesis(state, a, { by, at_ms }) {
    state.hypotheses = state.hypotheses || [];
    const old = a.id && state.hypotheses.find((h) => h.id === a.id);
    const h = old || { id: a.id || fresh(state.hypotheses, "h"), supports: [], contradicts: [], verdict: null, by };
    if (a.text !== undefined) h.text = a.text;
    if (a.settle !== undefined) h.settle = a.settle;
    if (a.why !== undefined) h.why = a.why;
    if (a.confidence !== undefined) {
      if (!(typeof a.confidence === "number" && a.confidence >= 0 && a.confidence <= 100)) throw new Error("confidence is a number from 0 to 100");
      if (old && old.confidence !== a.confidence) h.was = old.confidence;
      h.confidence = a.confidence;
    }
    if (a.supports !== undefined) h.supports = ids(state, a.supports, "supports");
    if (a.contradicts !== undefined) h.contradicts = ids(state, a.contradicts, "contradicts");
    const both = h.supports.filter((id) => h.contradicts.includes(id));
    if (both.length) throw new Error(`${both.join(", ")} cannot both support and contradict ${h.id}`);
    need(h.text, "a hypothesis needs its text: one sentence");
    if (h.confidence == null) throw new Error("give your confidence, 0 to 100");
    h.at_ms = at_ms;
    if (!old) state.hypotheses.push(h);
  },
  // One mark for or against one hypothesis: as is "supports", "contradicts" or "none".
  link(state, { hypothesis, mark, as }) {
    const h = one(state.hypotheses, hypothesis, "hypothesis");
    one(state.marks, mark, "mark");
    if (!["supports", "contradicts", "none"].includes(as)) throw new Error("as is supports, contradicts or none");
    h.supports = (h.supports || []).filter((id) => id !== mark);
    h.contradicts = (h.contradicts || []).filter((id) => id !== mark);
    if (as !== "none") h[as].push(mark);
  },
  lane(state, { id, label }) {
    state.lanes = state.lanes || [];
    const l = state.lanes.find((l) => l.id === need(id, "a lane needs an id"));
    if (l) l.label = need(label, "a lane needs a label");
    else state.lanes.push({ id, label: need(label, "a lane needs a label") });
  },
  // What it turned out to be, and which hypothesis that is, if one.
  conclude(state, { text, hypothesis }, { by, at_ms }) {
    if (hypothesis) one(state.hypotheses, hypothesis, "hypothesis");
    state.conclusion = { text: need(text, "say what it turned out to be"), hypothesis: hypothesis || null, by, at_ms };
  },
  reopen(state) {
    state.conclusion = null;
  },
};
