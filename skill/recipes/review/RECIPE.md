# review

**Use when** you changed code and the person has to decide whether it is what they
asked for. It answers "did I get what I asked for, and what needs my eyes?" The
diff is the backup, not the page.

```bash
# Write your own state.json. To change the view or verbs: jelly recipes review --out DIR
jelly make review --recipe review --state state.json --about "Review: <what they asked for>"
git diff main...HEAD > /tmp/full.diff && jelly asset review /tmp/full.diff
jelly look review                             # fix every `!` and `error` line; look again on a `failed` one
jelly show review
```

The example `state.json` and `assets/` are a real review: the change that made
`jelly ask` say so when its `--at` matches nothing. Replace both with your own.

The view and verbs are a starting point. Change them when the review needs
something else: before/after screenshots, a rendered preview, a table of rows.

## State

| Key | What goes in it |
| --- | --- |
| `title` | The change, in a few words. |
| `ask` | What the person asked for, in their words. Quote them. |
| `range` | What was compared, e.g. `main...HEAD`. |
| `total_lines` | Added plus removed lines of the whole change (`git diff --numstat`). Not needed when `assets/full.diff` is attached. |
| `next` | What you do when they say go: "Commit and push to x". |
| `outcomes[]` | `id`, `text`, `status` (`seen`, `tested`, `unverified` or `fails`), `evidence`. One per thing they asked for. |
| `groups[]` | `id`, `intent` (a few words: it is a label on the map), `outcome` (an outcome id, or a list of them), `risk` (`low`, `medium`, `high`), `why`, `show`, `evidence`, `files[]`, `diff`, and `mechanical: true` when there is nothing to decide. |
| `groups[].show` | The picture of the change: `{"kind", "before", "after", "caption"}`. `terminal` (the default) takes command output, `image` takes two assets (`assets/before.png`) and gives a slider, `html` takes two pages and runs them live. Leave `before` out when there was nothing before. `labels` renames the two sides, `compare: "side"` puts two images next to each other, and a list of these shows several pictures. |
| `not_done[]` | `kind` (`skipped`, `unverified`, `outside` or `regression`) and `text`. |

## Rules that make it worth their time

- **Show, do not describe.** The page is a map (what they asked for, wired to the
  changes made for it), then one change at a time with its before and after.
  Give every change that decides something a `show`: run the old and the new and
  paste both, or screenshot both (`jelly look NAME --width 1280`, then
  `jelly asset`). Without a `show` the person gets a diff, which is the thing this recipe is for
  avoiding. Keep `intent` and outcome `text` to a few words.
- **Group by intent, not by file.** One group per reason you changed code. A file
  can appear in several groups; each group's `diff` holds only its own hunks.
- **Every changed line is in a group.** The bar counts the lines in the groups
  against the whole diff and says so in red when they differ. Attach the whole
  diff as `assets/full.diff` so the count is checked against the real thing and
  the person can open it.
- **Evidence, not claims.** `seen` means you ran it and watched it work: paste the
  command and its output. `tested` names the test. Anything else is `unverified`,
  and it also goes in `not_done`.
- **Order groups by how much they need the person.** Judgment calls first. Mark
  renames, formatting and generated files `mechanical`.
- **Ask only what you could not decide.** Put each judgment call on its group:
  `jelly ask review "…" --option … --recommend … --why "…" --at "#g-<group id>"`.
  If more than a few groups carry a question, decide more yourself.
- **Say what is outside the ask.** Anything you changed that they did not ask for
  is a `not_done` item of kind `outside`, even when you think it is an improvement.

## Reviewing someone else's change

The same page works for a pull request you did not write. The person then
decides whether to merge it, not whether you did what they asked.

- `ask` is what the change says it does: quote the PR title and description.
  One outcome per claim.
- `fails` means you checked a claim and it does not hold. Paste the command and
  the output that shows it, as for `seen`. Do not soften it to `unverified`.
- Something that worked before and is broken now is a `not_done` item of kind
  `regression`: say what broke and how you saw it. Show it on the group that
  caused it, with the old output as `before` and the new as `after`.
- Set a group's `risk` from what you found, not from what the author wrote.
- `next` is what you do with the verdict: "Post this as a review on PR 142".

## Hearing them and answering

The person approves, pushes back or asks, per group. It arrives in `jelly hear`
as `approve`, `push_back`, `question`, `take_back` and `go` calls, with comments
on single lines (`#g-<id>-L<n>`).

```bash
# They pushed back or asked: fix it, then answer on the group. With `delta`,
# the card shows only what changed since they looked.
jelly do review revise @revise.json   # {"group","note","diff","delta","evidence"}
jelly do review revise '{"group":"check","note":"It is safe: the lock is held here."}'   # an answer, no code change
jelly do review outcome '{"id":"no-browser","status":"seen","evidence":"…"}'
# They said go: do `next`, then
jelly do review applied '{"note":"Committed as 3f2a1c and pushed."}'
```

`go` is refused until every group is approved, and any new say takes it back, so
act on `go` only when it is the latest thing in the inbox.
