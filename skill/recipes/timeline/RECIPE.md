# timeline

**Use when** you are working out why something happened: an incident, a flaky
test, a regression, a slow path. It lays what was seen out in time, puts your
competing explanations beside it, and lets the person say which they believe.
Start it as soon as you have two observations, and add to it as you go.

```bash
cp -r <skill>/recipes/timeline /tmp/timeline   # then write your own state.json there
jelly new timeline --from /tmp/timeline --about "Why <the thing> happened"
jelly see timeline                             # fix every `!` and `error` line first
jelly focus timeline
```

The example `state.json` is a real investigation: why `jelly see` sometimes took
20 s and came back without a snapshot. It is still open, and says so. Replace it
with your own. The view and verbs are a starting point; change them when the
investigation needs something else.

## State

| Key | What goes in it |
| --- | --- |
| `title`, `question` | A few words, then the question you are answering. |
| `status` | One line on where it stands: open, worked around, fixed. |
| `clock` | The time at `t = 0`, as `"21:02"`. Ticks then show clock time; without it they show `+m:ss`. |
| `lanes[]` | `id`, `label`. One row per kind of evidence: commands run, what the browser did, what is on disk, code. Three to six. |
| `marks[]` | One thing that was seen. `id`, `lane`, `t` (seconds from the start of the axis), `label` (a few words: it sits on the axis), `kind` (`ok`, `bad` or `info`), and optionally `dur` (seconds: makes it a bar), `detail`, `show`, `when` (your own wording of the time), `approx: true`. |
| `marks[].show` | The evidence itself: `{"kind", "before", "after", "caption"}`, as in the review recipe. `terminal` (the default) takes output, `image` takes assets, `html` runs live. Usually only `after`. A list shows several. |
| `hypotheses[]` | `id`, `text` (one sentence), `confidence` (0 to 100, yours), `why`, `supports[]` and `contradicts[]` (mark ids), `settle` (what would settle it). The person's say goes in `verdict`, which you do not write. |
| `conclusion` | Written by `conclude`: what it turned out to be. Leave it `null` until you know. |

Long quiet stretches are cut out of the axis, so two busy minutes hours apart
both stay readable; the cut says how long it was.

## Rules that make it worth their time

- **A mark is something seen, not something you think.** A command and its
  output, a timing, a file that was there. What you think goes in a hypothesis.
  Give every mark a `show` with the output you actually got.
- **Say what you did not measure.** Use `approx: true` and `when: "about 23:20"`
  when you are placing a mark from memory or from order alone.
- **Keep at least two hypotheses, and link against as well as for.** A hypothesis
  with nothing against it has not been tested. When a new mark contradicts your
  favourite, link it and lower the confidence in the same turn; the card shows
  what it was before.
- **Confidence is yours; the verdict is theirs.** Do not raise your number
  because they confirmed. If you disagree with their verdict, say so in `why`.
- **`settle` is a next step someone could run.** "Give each render its own
  profile and run it fifty times", not "investigate further".
- **Do not conclude early.** A workaround that works is a `status`, not a
  conclusion. `conclude` only when a `settle` was run and a mark shows the result.
- **Ask only what they can know and you cannot.** Put it on the hypothesis or
  the mark: `jelly ask timeline "…" --option … --recommend … --why "…" --at "#h-<id>"`
  (or `--at "#e-<mark id>"`, which shows below the axis).

## Hearing them and answering

The person picks a hypothesis to light up what is for and against it, opens any
mark to see the evidence, and reacts. It arrives in `jelly inbox` as:

- `judge` `{hypothesis, verdict, note}`: `confirmed`, `ruled_out`, or `next`
  ("check this next": do that before anything else). `take_back` undoes it.
- `pin` `{mark}`: "this matters". Explain it, or build the next check on it.
- `know` `{lane, rel, mark, text, kind}`: something they saw, placed before,
  with or after one of your marks. It becomes a mark with `by: "person"`; link
  it to the hypotheses it bears on.

```bash
# As you find things. A mark that arrives while they watch is shown arriving.
jelly call timeline mark @mark.json   # {"lane","t","dur","label","kind","detail","show","when"}; with "id", revises
jelly call timeline hypothesis '{"text":"Runs collide on the shared profile","confidence":25,"settle":"…","supports":["disk-lock"]}'
jelly call timeline hypothesis '{"id":"lock","confidence":10,"why":"It renders even with the lock held."}'
jelly call timeline link '{"hypothesis":"lock","mark":"ask-8s","as":"contradicts"}'   # or supports, none
jelly call timeline lane '{"id":"net","label":"Network"}'
jelly set timeline --at /status '"Fixed in 3f2a1c; watching for a day."'
jelly call timeline conclude '{"text":"Chrome did not exit because …","hypothesis":"linger"}'   # reopen takes it back
```
