// The agent (or anyone who is not a person) runs the game; people join and vote.
const isPerson = (by) => by === "person" || by.startsWith("person:");
const host = (by) => { if (isPerson(by)) throw new Error("only the quizmaster runs the rounds"); };
const verbs = {
  join(state, args, { by }) {
    if (!by.startsWith("person:")) throw new Error("set your name first");
    state.players[by] = state.players[by] || { score: 0 };
  },
  open(state, { seconds }, { by, at_ms }) {
    host(by);
    const r = state.round + 1;
    if (!state.rounds[r]) throw new Error("no more rounds; add one first");
    state.round = r; state.phase = "question";
    state.ends_at_ms = at_ms + 1000 * (seconds || state.rounds[r].seconds || 15);
    state.votes[r] = {};
  },
  vote(state, { option }, { by, at_ms }) {
    if (state.phase !== "question") throw new Error("no question is open");
    if (at_ms > state.ends_at_ms) throw new Error("time is up");
    if (!state.players[by]) throw new Error("join first");
    const votes = state.votes[state.round];
    if (votes[by]) throw new Error("you already answered");
    votes[by] = { option, left_ms: state.ends_at_ms - at_ms };
  },
  reveal(state, args, { by }) {
    host(by);
    if (state.phase !== "question") throw new Error("nothing to reveal");
    const r = state.rounds[state.round];
    // Right answers score 500 plus up to 500 more for speed.
    for (const [who, v] of Object.entries(state.votes[state.round])) {
      if (v.option === r.correct) state.players[who].score += 500 + Math.round(500 * v.left_ms / (1000 * r.seconds));
    }
    state.phase = "reveal";
  },
  add(state, { q, options, correct, seconds }, { by }) {
    host(by);
    if (!q || !Array.isArray(options) || options.length < 2 || options.length > 4) throw new Error("a question and 2 to 4 options");
    if (!(correct >= 0 && correct < options.length)) throw new Error("correct is the index of the right option");
    state.rounds.push({ q, options, correct, seconds: seconds || 15 });
  },
  finish(state, args, { by }) { host(by); state.phase = "done"; },
};
