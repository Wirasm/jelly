// An ask page. Its questions are attention items on this jelly, made by
// `jelly ask NAME --spec`; answers live on those items. State holds only what
// the page shows around them.
const verbs = {
  setup(state, { title, context, round }) {
    state.title = title || "";
    state.context = context || "";
    state.round = round || 1;
    state.applied = null;
  },
  // The agent acted on the answers.
  applied(state, { note }, { by, at_ms }) {
    state.applied = { note: note || "", by, at_ms };
  },
};
