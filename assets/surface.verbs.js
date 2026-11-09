// The jelly page is a jelly too. `focus` is the jelly open full size; an agent
// that wants the person to look at something calls it. `null` closes it.
// `seen` remembers which version of each jelly the person has looked at, so the
// page can mark what is new or changed.
const verbs = {
  focus(state, { name }) {
    state.focus = name || null;
  },
  seen(state, { name, version }) {
    if (!name) throw new Error("seen needs a jelly name");
    state.seen ||= {};
    state.seen[name] = Math.max(state.seen[name] || 0, version || 0);
  },
};
