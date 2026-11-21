// A frame for comparing versions of one thing, step by step. The person picks one.
const find = (state, id) => {
  const v = (state.versions || []).find((v) => v.id === id);
  if (!v) throw new Error(`no version ${id}; there are: ${(state.versions || []).map((v) => v.id).join(", ")}`);
  return v;
};
const verbs = {
  // The person's choice. What they want changed in it comes as comments.
  pick(state, { id }, { by, at_ms }) {
    find(state, id);
    state.picked = { id, by, at_ms };
  },
  unpick(state) {
    state.picked = null;
  },
};
