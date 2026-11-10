# triage

**Use when** there are roughly 10 to 100 things (issues, failing tests, review
comments, TODOs, findings) and the person has to agree what happens to each.
You sort them first; the person only corrects. It answers "is this the right
pile for each, and where were you guessing?"

```bash
cp -r <skill>/recipes/triage /tmp/triage      # then write your own state.json there
jelly new triage --from /tmp/triage --about "Triage: <what is being sorted>"
jelly see triage                              # fix every `!` and `error` line first
jelly focus triage
```

The example `state.json` is a real triage: the issues and findings from the
sessions that built these recipes. Replace it with your own. The view and verbs
are a starting point; change them when the list needs something else.

## State

| Key | What goes in it |
| --- | --- |
| `title` | What is being sorted, in a few words. |
| `ask` | What the person asked for, in their words. Quote them. |
| `next` | What you do when they say go: "Fix the Do now lane, top to bottom". |
| `lanes[]` | `id`, `label`, `hint` (a few words under the label), `tone` (`accent`, `warn`, `idle`, `good` or `bad`: the lane's colour), and `ordered: true` when the order inside the lane means something. Two to five lanes. |
| `items[]` | `id`, `ref` (what they know it by: `#12`, a test name; three characters fit on a dot), `title`, `lane`, `agent_lane` (the same as `lane` to start), `reason` (one line: why this lane), `confidence` (0 a guess, 1 certain), and `impact` and `effort` (1 to 5) to place it on the scatter. |
| `unsure_below` | Items with less `confidence` than this are shown first. 0.6 unless you set it. |
| `whys[]` | Reasons offered after the person moves something, one tap each. Write ones that fit this list. |

The verbs fill in `moved`, `checked`, `done`, `accepted`, `go` and `applied`.
Add items later with the `add` verb, not by editing state: it checks them.

## Rules that make it worth their time

- **Sort everything before you show it.** Every item arrives in a lane with a
  reason. An unsorted pile hands the person your job.
- **Be honest about confidence.** The page puts what you were unsure of first,
  so a flat 0.9 on everything hides exactly the items they should look at.
  Aim for a handful below `unsure_below`, not half the list.
- **One line of reason, about this item.** "Low priority" is a lane, not a
  reason. Say what you saw: "Only bites with two agents in one project."
- **Impact and effort on every item you can judge.** The scatter is where a
  wrong lane shows: a quick win sitting in Later, a big bet in Do now.
- **Ask on the item.** A real decision that blocks sorting goes on its card:
  `jelly ask triage "…" --option … --recommend … --at "#i-<item id>"`.
  A lane is `#lane-<lane id>`.
- **Their move stands.** Once the person has moved an item, `resort` refuses.
  If you disagree, ask.

## Hearing them and answering

What the person does arrives in `jelly inbox` as calls: `keep` (you were unsure
and right), `move` (to another lane, or to another place in an ordered lane),
`note` (why they moved it), `accept` and `reopen` (a lane is sorted, or not
after all) and `go`. Each moved item keeps `moved.from`, `moved.by` and
`moved.note` in state, so `jelly state triage` shows every correction.

```bash
# New things turned up: add them, sorted, with a reason.
jelly call triage add @more.json     # {"items":[{"id","ref","title","lane","reason","confidence","impact","effort"}]}
# You learned something and changed your mind about one they have not moved:
jelly call triage resort '{"item":"7","lane":"now","reason":"Blocks #1 after all","confidence":0.8}'
# You dealt with one:
jelly call triage done '{"item":"5","result":"Fixed in ef60c62."}'
# They said go: do `next`, then
jelly call triage applied '{"note":"Logged two issues; started on #2."}'
```

`go` is refused until every lane is accepted. Adding or re-sorting an item
takes back the acceptance of the lanes it touches, and any such change takes
back `go`, so act on `go` only when it is the latest thing in the inbox.
