// A decision: an agent lays out one or more questions, each with options; the
// person picks, notes, or sends a question back, and can add questions or
// context of their own. Made by `jelly ask NAME --spec spec.json`; asking again
// with the same name starts a new round and keeps the old one in history.
//
// A spec is either one question ({question, context, recommend, why, options})
// or a set ({title, context, questions: [{id, question, context, recommend, why, options}]}).
// Both become the same state: one question is a set of one.

function normalize(spec) {
  const single = !Array.isArray(spec.questions);
  const qs = single ? [{ ...spec, context: "" }] : spec.questions;
  if (!qs.length) throw new Error("a decision needs at least one question");
  const qids = new Set();
  const questions = qs.map((q, i) => {
    const id = String(q.id || `q${i + 1}`);
    if (qids.has(id)) throw new Error(`question id ${id} is used twice`);
    qids.add(id);
    if (!q.question) throw new Error(`question ${id} needs a question`);
    if (!Array.isArray(q.options) || q.options.length < 1) throw new Error(`question ${id} needs options`);
    const ids = new Set();
    for (const o of q.options) {
      if (!o.id || !o.title) throw new Error(`every option needs an id and a title (question ${id})`);
      if (ids.has(o.id)) throw new Error(`option id ${o.id} is used twice in question ${id}`);
      ids.add(o.id);
    }
    if (q.recommend && !ids.has(q.recommend)) throw new Error(`recommend names no option of question ${id}: ${q.recommend}`);
    return {
      id,
      question: q.question,
      context: q.context || "",
      recommend: q.recommend || null,
      why: q.why || "",
      options: q.options.map((o) => ({ summary: "", gains: [], costs: [], effort: "", preview: "", ...o })),
      answer: null, // {kind: "pick", option, by, at_ms} | {kind: "none", option: null, by, at_ms}
      note: null, // {text, by, at_ms}: context or a condition, with or without an answer
    };
  });
  return {
    title: single ? "" : spec.title || "",
    context: spec.context || "",
    questions,
    added: [], // {id, text, by, at_ms}: questions or context the person adds for the agent
    seq: 0,
    status: "open",
    applied: null,
  };
}

// The only place status is decided. A question counts as answered when the
// person picked something or wrote a note: "not sure, keep it simple" is an
// answer too. Any change by the person reopens an applied decision, so the
// agent hears about it.
const responded = (q) => Boolean(q.answer || q.note?.text);
function settle(state) {
  state.applied = null;
  state.status = state.questions.every(responded) ? "answered" : "open";
}

function question(state, qid) {
  if (qid == null) {
    if (state.questions.length === 1) return state.questions[0];
    throw new Error(`say which question: q is one of ${state.questions.map((q) => q.id).join(", ")}`);
  }
  const q = state.questions.find((q) => q.id === qid);
  if (!q) throw new Error(`no question ${qid}; questions are ${state.questions.map((q) => q.id).join(", ")}`);
  return q;
}

function setNote(q, text, by, at_ms) {
  const t = String(text ?? "").trim();
  if (!t && q.answer?.kind === "none") throw new Error("None of these needs a note; clear the answer first");
  q.note = t ? { text: t, by, at_ms } : null;
}

const verbs = {
  init(state, spec) {
    return { ...normalize(spec), round: 1, history: [] };
  },
  revise(state, spec, { by, at_ms }) {
    const next = normalize(spec);
    const { history = [], round = 1, ...current } = state;
    return { ...next, round: round + 1, history: [...history, { ...current, round, closed_by: by, closed_at_ms: at_ms }] };
  },
  // option null or "" means none of these, which needs a note.
  pick(state, { q: qid, option: id, note }, { by, at_ms }) {
    const q = question(state, qid);
    if (id) {
      if (!q.options.some((o) => o.id === id)) throw new Error(`no option ${id}; options are ${q.options.map((o) => o.id).join(", ")}`);
      q.answer = { kind: "pick", option: id, by, at_ms };
      if (note !== undefined) setNote(q, note, by, at_ms);
    } else {
      const text = String(note ?? q.note?.text ?? "").trim();
      if (!text) throw new Error("say what is missing, so the next round can fix it");
      q.answer = { kind: "none", option: null, by, at_ms };
      setNote(q, text, by, at_ms);
    }
    settle(state);
  },
  none(state, { q, note }, ctx) {
    verbs.pick(state, { q, option: null, note }, ctx);
  },
  // Empty text removes the note.
  note(state, { q: qid, text }, { by, at_ms }) {
    setNote(question(state, qid), text, by, at_ms);
    settle(state);
  },
  clear(state, { q: qid }) {
    question(state, qid).answer = null;
    settle(state);
  },
  add(state, { text }, { by, at_ms }) {
    const t = String(text ?? "").trim();
    if (!t) throw new Error("nothing to add");
    state.seq += 1;
    state.added.push({ id: `a${state.seq}`, text: t, by, at_ms });
    settle(state);
  },
  drop(state, { id }) {
    const i = state.added.findIndex((a) => a.id === id);
    if (i < 0) throw new Error(`nothing added as ${id}`);
    state.added.splice(i, 1);
    settle(state);
  },
  applied(state, { note = "" }, { by, at_ms }) {
    if (state.status !== "answered") throw new Error("nothing to apply: not every question is answered");
    state.status = "applied";
    state.applied = { note, by, at_ms };
  },
};
