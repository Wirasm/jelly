const verbs = { pick(state, { id }) { if (!state.variants.some(v => v.id === id)) throw new Error("no variant " + id); state.pick = id; } };
