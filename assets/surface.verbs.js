// The surface is a jelly too. `focus` is the jelly open full size; an agent
// that wants the person to look at something calls it. `null` closes it.
const verbs = {
  focus(state, { name }) {
    state.focus = name || null;
  },
};
