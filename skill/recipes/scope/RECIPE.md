# scope

**Use when** the decision is a tradeoff between many parts, not a pick between two
or three options: what goes in a release, where a budget goes, which risks to
accept. You build the model and a cut you recommend; the person moves things and
watches what it costs them. For a pick between a few options, use `jelly ask`.

```bash
cp -r <skill>/recipes/scope /tmp/scope        # then write your own state.json there
jelly new scope --from /tmp/scope --about "Scope: <what is being decided>"
jelly call scope recount                      # writes `result` from your items and levers
jelly see scope                               # fix every `!` and `error` line first
jelly focus scope
```

The example `state.json` is real: what goes into jelly's next release, from the
issues and recipes of the session that built this. Replace it with your own.

The view and verbs are a starting point. The sums are one function, `model`, at
the top of `verbs.js`. Change it there when your tradeoff adds up differently
(risk instead of time, two budgets, a fixed order): the verbs and the page both
use it, so they cannot disagree.

## State

| Key | What goes in it |
| --- | --- |
| `title`, `ask` | What is being decided, in a few words, and one line on what you want from them. |
| `unit` | What cost is counted in: `h`, `days`, `€`, `points`. |
| `start` | The first day, `YYYY-MM-DD`. With a `pace` lever this gives a landing date. Leave it out for a budget with no calendar. |
| `groups[]` | `id`, `label`. Up to six. They colour the bar, the chart and the tiles. |
| `levers[]` | `id`, `role`, `label`, `min`, `max`, `step`, `value`, `unit`. Roles: `pace` (units done per week), `people` (how many at once), `window` (days from `start` to the target), `buffer` (percent on top), `budget` (the room itself, when there is no calendar). Two or three levers, not six. |
| `items[]` | `id`, `short` (a few characters: it is the label on the chart), `label`, `cost`, `value` (1 to 5), `group`, `why` (one line), `needs[]` (ids), `in` (true or false), `done`. |
| `recommended` | Your cut: `items[]` (ids), `levers` (id to value), `why` (one or two sentences). Set it with the `recommend` verb. |
| `result` | Written by the verbs, never by you: `in`, `out`, `fell` (wanted, but out with what it needs), `done`, `cost`, `buffer`, `load`, `capacity`, `spare`, `fits`, `value`, `date`, `late`, `vs` (the difference from your cut) and a one-sentence `summary`. |
| `locked` | Set when the person locks the plan: who, when, what was in, and the summary. |

## Rules that make it worth their time

- **Do the cut first.** Never show everything "in" and leave the cutting to them.
  Set what you would ship, make it fit, and say why in `recommended.why`. Their
  job is to disagree with you in two or three places.
- **Estimate honestly, and say what the number is.** A cost is what is left to
  do, not what it took. If a third of your items share one cost, you have not
  estimated them. Value is 1 to 5 for the person, not for you.
- **Put the real dependencies in `needs`.** Taking out an item shows what falls
  out with it; putting one in brings what it needs. Without `needs` the page lets
  them build a plan that cannot ship.
- **Levers are the assumptions you are least sure of.** Hours a week, the date,
  the buffer. Give each a range that is plausible at both ends. A lever nobody
  would move is a number: put it in the model.
- **One line of `why` per item.** It is the only prose on a tile. Say what it
  costs them to leave it out.
- **Ask on the thing itself.** `jelly ask scope "…" --at "#i-<item id>"` or
  `--at "#lever-<lever id>"` for a judgment you could not make.
- **Read the outcome, do not recompute it.** `jelly state scope /result/summary`
  is the sentence; `/result` has every number. After `jelly set`, run
  `jelly call scope recount`.

## Hearing them and answering

The person toggles items, moves levers, goes back to your cut and locks. It
arrives in `jelly inbox` as `put`, `lever`, `reset`, `lock` and `unlock` calls. A
slider calls `lever` once, when they let go. `lock` is refused while the plan
does not fit, and says by how much.

```bash
jelly state scope /result/summary        # "Lands Tue 24 Nov: fits with 1.2 h spare · 11 in, 4 out, 2 done · value 51 of 60"
jelly state scope /result/vs             # what they changed against your cut
jelly state scope /locked                # null until they lock; then what they chose

# You learned something: re-estimate. The old number stays on the tile, struck out.
jelly call scope item '{"id":"i8","cost":2,"why":"Confirmed: only the WebSocket is open."}'
jelly call scope item '{"id":"docs","short":"docs","label":"A page on recipes","cost":1,"value":2,"group":"recipes"}'
jelly call scope done '{"item":"i6"}'    # finished: its cost stops counting, its value is banked
jelly call scope recommend '{"items":["i1","i2","i4"],"why":"…"}'   # a new cut; with no items, what is set now
```

If a re-estimate makes a locked plan stop fitting, the lock is taken off and the
page says why (`reopened`). Tell the person in the same turn. When they lock,
`locked.in` is the list to act on.
