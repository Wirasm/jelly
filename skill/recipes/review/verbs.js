// A review: the person reacts to groups of changes, the agent answers.
// A group's `verdict` is the person's current say; `last` is what they said
// before the agent's latest reply, so the card can show the exchange.
const find = (state, id) => {
  const g = (state.groups || []).find((g) => g.id === id);
  if (!g) throw new Error(`no group ${id}; this review has: ${(state.groups || []).map((g) => g.id).join(", ")}`);
  return g;
};
const words = (note, what) => {
  const t = String(note || "").trim();
  if (!t) throw new Error(`say ${what}`);
  return t;
};
const say = (state, group, kind, note, { by, at_ms }) => {
  const g = find(state, group);
  g.verdict = { kind, note, by, at_ms, rev: g.rev || 1 };
  state.go = null; // a new say reopens the review
};

const verbs = {
  // ---- the person ----
  // One group, or several at once ({ groups: [...] }) for the mechanical ones.
  approve(state, { group, groups }, ctx) {
    for (const id of groups || [group]) say(state, id, "approved", "", ctx);
    state.go = null;
  },
  push_back(state, { group, note }, ctx) {
    say(state, group, "change", words(note, "what should change"), ctx);
  },
  question(state, { group, note }, ctx) {
    say(state, group, "question", words(note, "what you want to know"), ctx);
  },
  take_back(state, { group }) {
    find(state, group).verdict = null;
    state.go = null;
  },
  // Every group is approved: do the next step.
  go(state, _args, { by, at_ms }) {
    const open = (state.groups || []).filter((g) => g.verdict?.kind !== "approved");
    if (open.length) throw new Error(`not yet: ${open.map((g) => g.id).join(", ")} ${open.length === 1 ? "is" : "are"} not approved`);
    state.go = { by, at_ms };
  },

  // ---- the agent ----
  // Answer a push-back or a question. With `diff`, the group's code changed:
  // pass `delta` (only what changed since they looked) so re-review is cheap.
  revise(state, { group, note, diff, delta, evidence, files }, { by, at_ms }) {
    const g = find(state, group);
    g.last = g.verdict || g.last || null;
    g.verdict = null;
    g.reply = { note: words(note, "what you did or what the answer is"), by, at_ms };
    if (diff !== undefined) {
      g.diff = diff;
      g.delta = delta || null;
      g.rev = (g.rev || 1) + 1;
    }
    if (evidence !== undefined) g.evidence = evidence;
    if (files !== undefined) g.files = files;
    state.go = null;
  },
  // An outcome's status changed: "seen", "tested" or "unverified".
  outcome(state, { id, status, evidence }) {
    const o = (state.outcomes || []).find((o) => o.id === id);
    if (!o) throw new Error(`no outcome ${id}`);
    if (!["seen", "tested", "unverified"].includes(status)) throw new Error("status is seen, tested or unverified");
    o.status = status;
    if (evidence !== undefined) o.evidence = evidence;
  },
  // The next step is done ("Committed as 3f2a1c").
  applied(state, { note }, { by, at_ms }) {
    state.applied = { note: note || "", by, at_ms };
  },
};
