# quiz

**Use when** several people answer the same questions in rounds against a clock,
and you run it: a quiz on a call, a vote between designs, a check of what a class
understood. It answers "what did everyone pick, and who was fast and right?"

```bash
# Write your own state.json. To change the view or verbs: jelly recipes quiz --out DIR
jelly make quiz --recipe quiz --state state.json --about "Quiz: <what it is about>"
jelly try quiz --type "#who=Ana" --click "#go"   # check joining without changing anything
jelly show quiz
```

People join with a name (it becomes `person:<name>` on their events) and wait in
the lobby. You are the quizmaster: only non-person callers can open, reveal,
add or finish. The example `state.json` is a short Nordic food quiz.

`join` and `vote` are each person's own decisions, and everyone else sees them
(the names in the lobby, how many answered, the bars), so both stay in state
and count on Send. Nobody needs to press Send in a quiz: you follow the votes
with `--every`.

## State

| Key | What goes in it |
| --- | --- |
| `title` | What the quiz is, in a few words. |
| `phase` | `lobby`, `question`, `reveal` or `done`. Verbs move it; leave it `lobby`. |
| `round` | The open round's index. Start at `-1`. |
| `rounds[]` | `q`, `options` (2 to 4), `correct` (index), `seconds`. |
| `players` | Filled by `join`: `{"person:Ana": {"score": 0}}`. |
| `votes[]` | Filled by `vote`, one object per round: `{"person:Ana": {"option", "left_ms"}}`. |

A right answer scores 500 plus up to 500 for speed.

## Running it

```bash
jelly do quiz open '{}'                 # the next round; prints `since seq:N`
jelly hear --wait quiz --every --since seq:N --json   # one event per vote; repeat until all voted or time is up
jelly do quiz reveal '{}'               # scores, the right answer and the vote bars
jelly do quiz add '{"q":"…","options":["…","…"],"correct":0}'   # a round written from what you just saw
jelly do quiz finish '{}'               # final scores
```

- **Write later rounds from earlier answers.** After a reveal, read
  `jelly state quiz /votes` and ask about what people got wrong. That is what a
  quiz you run can do that a fixed one cannot.
- **Shape between rounds, not during one.** Open pages swap to the new view and
  keep their state, but a shape mid-question costs people seconds.
- **Not a secret ballot.** Anyone with the page can read the votes in state before
  the reveal. Use it for games and quick checks, not for decisions that must stay private.
