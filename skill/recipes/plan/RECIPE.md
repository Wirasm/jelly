# plan

**Use when** the work has more than three steps or will run longer than a few
minutes. The person watches you work through the plan and steers it: they
reorder what has not started, veto a step, or ask to be checked with before one.
It answers "where are you, what is next, and can I still change it?"

```bash
cp -r <skill>/recipes/plan /tmp/plan          # then write your own state.json there
jelly new plan --from /tmp/plan --about "Plan: <what they asked for>"
jelly see plan                                # fix every `!` and `error` line first
jelly focus plan
jelly call plan start '{"step":"schema"}'     # before you touch anything
```

The example `state.json` and `assets/` are a real plan: the session that built
these recipes. Replace both with your own. The view and verbs are a starting
point; change them when the plan needs something else.

## State

| Key | What goes in it |
| --- | --- |
| `title` | The whole job, in a few words. |
| `goal` | What the person asked for, in their words. Quote them. |
| `steps[]` | In the order you mean to work. `id` (lowercase, digits, dashes), `title` (a few words: it is a label on the track), `why` (one line, optional), `needs` (ids of steps that must be done first), `status` (`next` to begin with). |
| `steps[].gate` | `{"by", "at_ms", "note", "cleared": null}` on a step you will not start without their say. Set it with the `gate` verb. |
| `steps[].result` | Written by `finish`: `text` and `show`. |
| `steps[].progress` | Written by `progress`: `text` and `show` for the step in progress. |
| `show` | A picture, as in the review recipe: `{"kind", "before", "after", "caption"}`. `terminal` (the default) takes command output, `image` takes assets and gives a slider, `html` runs two pages live. Leave `before` out when there was nothing before. `labels` renames the two sides, `compare: "side"` puts two images next to each other, and a list shows several. |

## Rules that make it worth their time

- **The page moves when you move.** `start` a step before you work on it and
  `finish` it when it is done, one at a time. A plan that jumps from nothing to
  everything done told them nothing while they could still steer.
- **Show what each step produced.** `finish` takes one line and a `show`: the
  command output, a screenshot (`jelly see NAME --png`, then `jelly asset`), the
  page before and after. A step with no picture is a claim.
- **`needs` are real dependencies, not the order you happened to write.** They
  are drawn beside the track, and they decide what the person may reorder. Leave
  out a need and they can move a step above what it depends on.
- **Gate what is theirs to decide.** Anything hard to undo or outward-facing
  (a merge, a migration, a send) gets `gate` with a note saying why, set when you
  write the plan. Only the person clears a gate.
- **A refusal is an answer.** `start` refuses a vetoed step, a step whose gate is
  not cleared, and a step whose needs are not done. Do not work around it with
  `jelly set`; do another step, or ask.
- **Ask on the step.** `jelly ask plan "…" --option … --recommend … --why "…" --at "#s-<step id>"`
  puts the question on that step's card and a `?` on the track.
- **Keep the plan true.** When the work changes shape, `add`, `revise` or
  `remove` steps and say why in a `note`. Do not rewrite a done step.
- **Say when you stop.** `block` with what blocks it and what would unblock it.
  `wrap` with where things stand when you are finished or handing over.

## Hearing them and answering

The person reorders, vetoes, gates, clears a gate and leaves notes. It arrives
in `jelly inbox` as `move`, `veto`, `unveto`, `gate`, `clear_gate` and `note`
calls. Read the inbox before every `start`: a veto on the step in progress
stops it and puts it back to not started, and you should stop too.

```bash
jelly call plan start '{"step":"schema"}'
jelly call plan progress '{"step":"schema","text":"Three of five tables moved"}'
jelly call plan finish @finish.json     # {"step","text","show":{"kind":"terminal","before":"…","after":"…"}}
jelly call plan block '{"step":"deploy","reason":"Staging is down; retry when it answers"}'
jelly call plan gate '{"step":"merge","note":"Merging is yours to decide."}'
jelly call plan add '{"id":"backfill","title":"Backfill old rows","needs":["schema"],"before":"deploy"}'
jelly call plan revise '{"step":"deploy","needs":["schema","backfill"]}'
jelly call plan remove '{"step":"cleanup"}'
jelly call plan note '{"step":"deploy","text":"Moved after the backfill, as you asked."}'
jelly call plan wrap '{"text":"Shipped. The merge waits for you."}'
```

Answer a note with a `note` on the same step, and act on it. A step moves from
`blocked` back to work with `start`.
