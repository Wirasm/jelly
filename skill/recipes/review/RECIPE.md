# review

**Use when** you changed code and the person has to decide whether it is what they
asked for. It answers "did I get what I asked for, and what needs my eyes?" The
diff is the backup, not the page.

```bash
cp -r <skill>/recipes/review /tmp/review      # then write your own state.json there
git diff main...HEAD > /tmp/review/assets/full.diff
jelly new review --from /tmp/review --about "Review: <what they asked for>"
jelly see review                              # fix every `!` and `error` line first
jelly focus review
```

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
| `outcomes[]` | `id`, `text`, `status` (`seen`, `tested` or `unverified`), `evidence`. One per thing they asked for. |
| `groups[]` | `id`, `intent`, `outcome` (an outcome id), `risk` (`low`, `medium`, `high`), `why`, `evidence`, `files[]`, `diff`, and `mechanical: true` when there is nothing to decide. |
| `not_done[]` | `kind` (`skipped`, `unverified` or `outside`) and `text`. |

## Rules that make it worth their time

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

## Hearing them and answering

The person approves, pushes back or asks, per group. It arrives in `jelly inbox`
as `approve`, `push_back`, `question`, `take_back` and `go` calls, with comments
on single lines (`#g-<id>-L<n>`).

```bash
# They pushed back or asked: fix it, then answer on the group. With `delta`,
# the card shows only what changed since they looked.
jelly call review revise @revise.json   # {"group","note","diff","delta","evidence"}
jelly call review revise '{"group":"check","note":"It is safe: the lock is held here."}'   # an answer, no code change
jelly call review outcome '{"id":"no-browser","status":"seen","evidence":"…"}'
# They said go: do `next`, then
jelly call review applied '{"note":"Committed as 3f2a1c and pushed."}'
```

`go` is refused until every group is approved, and any new say takes it back, so
act on `go` only when it is the latest thing in the inbox.
