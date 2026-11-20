---
name: jelly
description: Show things to the person instead of writing walls of text. Use jelly whenever a picture, a prototype, a live dashboard, a plan, a diff, a comparison or a choice between options would explain better than prose, and whenever you need the person to choose. Also use it to read what the person did or said in a jelly, to change one while it runs, or when they say "show me" or "jelly".
---

# jelly

A jelly is a small live web page you make in minutes and the person uses while it
runs: a clickable prototype, a 3D scene, a dashboard fed by a script, a plan, a
diff, options side by side. Every jelly appears on the person's jelly page at
http://127.0.0.1:7878. They click it, answer your questions and comment on any
part of it, and you hear all of it.

**Show, then let them react.** Do the thinking first: lay out the options, what
each gains and costs, what each looks like, and which you recommend and why.
Give at least two options, three or more when you can; a recommendation without
alternatives or a reason is refused. Never hand the person an empty question.

The first `jelly` command starts the daemon if it is not running. `JELLY_URL` (or
`--url`) points the CLI at another daemon; the jelly page is then at that address.

## Make a jelly

Write three files in a scratch folder, never in the person's repo:

- `view.html`: any page. `window.jelly` exists before your scripts run. CDN
  imports, canvas, WebGL and fetches to public APIs all work.
- `verbs.js`: `const verbs = { add(state, args, { by, at_ms }) { … } };`. Verbs
  are the only way state changes; a button in the view and you call the same
  verb. Change `state` in place or return a new one; throw to refuse. A verb runs
  at most 1 second, with no network, files or processes. Use `const verbs = {};`
  for a page that only shows.
- `state.json`: the starting state.

```bash
jelly new plan --view view.html --verbs verbs.js --state state.json --about "One line: why this exists"
jelly new plan --from ./folder        # the same three files (and assets/) from a folder
jelly call plan add '{"title":"…"}'   # run a verb; '@args.json' or - for big args
jelly set plan --at /title '"Fixed"'  # write state yourself; `jelly set plan @state.json` replaces it
jelly reshape plan --view v2.html     # new view or verbs, same state; open pages keep scroll and focus
jelly undo plan                       # undo the last change to view, verbs or state
jelly state plan /items               # read state, or the part at a JSON pointer
```

In the view:

```js
// this jelly
jelly.onState((state, event) => render(state)); // now, and after every change by anyone
jelly.call("add", { title });                   // runs a verb; rejects with its error
jelly.asset("shot.png");                        // URL of a file added with `jelly asset`
// other jellies
jelly.other("plan").watch((state) => …);        // its state, live; .state is a promise, .open() shows it
// the person
jelly.person.asks;                              // asks on this jelly; open ones have no .answer or .withdrawn
jelly.person.onAsk(() => …);                    // now, and when asks or comments change
jelly.who(vote.by);                             // "you", another person's name, or the agent's
```

People write as `person`, or `person:<name>` once they set a name, so several
people on one jelly stay apart; in verbs, a person is
`by === "person" || by.startsWith("person:")`.

Keep data in state, not page variables or localStorage, so you and the person
see the same thing. Give elements stable `id`s: questions and comments point at
them, and a reshape keeps the focused field and scrolled boxes by id. A
standalone jelly has a Comment button 10px from the bottom right; if a fixed
control sits there, move it with
`:root { --jelly-comment-inset: auto 10px 78px auto; }` (CSS `inset` order).

## Look before you show

```bash
jelly look plan                     # desktop + phone, light + dark: outline, problems, one side-by-side png
jelly try plan --click "#add" --type "#title=Tea" --press "#title=Enter"   # use it; nothing is saved
jelly look plan --compare v12       # what your change did since v12; --save NAME keeps a version longer
jelly look plan --text              # one width, no screenshot; --width 390 for one width with a png
jelly look plan --poster            # save a screenshot as the jelly page's thumbnail
jelly focus plan                    # open it full size on their screen; `jelly focus` closes it
```

Run `jelly look` before you focus a jelly or ask about it, read its png, and fix
every `error` and `!` line. Problems come with a selector you can paste into
`try` and `--at`. Check buttons and fields with `try`, not a browser driver:
your clicks there are never recorded as the person. After a change, `--compare`
against the version you started from marks problems you made as `new:`.

While you rebuild a jelly the person is looking at, say so: `jelly working plan
"rebuilding the header"`, then `jelly done plan`.

## Ask the person

One question, on the part of a jelly it is about:

```bash
jelly ask plan "Which hero?" --option bold="Bold gradient" --option quiet="Quiet white|Swaps to a plain hero" \
  --recommend quiet --why "The button stands out more" --at "#hero" --wait --timeout 300
# blocks; prints: a1 answered: quiet · "keep the button blue". Exit 2 on timeout.
jelly ask plan "Check the footer links" --seen --at "#footer"   # they answer "seen" or a word
jelly withdraw plan a1                                     # when it no longer applies
```

When the choice is easier to make by seeing each option, make an ask page: one
or more questions, each with two to four options side by side and a live
preview of each.

```bash
jelly ask sharing --spec spec.json --json    # prints the round and a `since` token
jelly wait sharing --since seq:57 --json     # the next answer; repeat from its seq for the rest
```

```json
{ "title": "Sharing a jelly", "context": "Why this matters, in a short paragraph.",
  "questions": [{ "id": "host", "question": "Where should shared jellies live?",
    "recommend": "relay", "why": "One sentence on why",
    "options": [{ "id": "relay", "label": "A small hosted relay", "does": "One or two sentences.",
      "gains": ["…"], "costs": ["…"], "effort": "M · two days",
      "preview": "<!doctype html>… any HTML: a mockup, an animation, a chart …" }] }] }
```

Make every preview real and different; each runs live in a sandbox. Each answer
event names its question (`args.title`), round (`args.group`, e.g. `round-1`),
`args.option` and `args.text`. An answer in words such as "none of these" means
ask a better round. Asking the same name again starts a new round and withdraws
open questions. When you acted on the answers: `jelly call sharing applied
'{"note":"Built in 3f2a1c"}'`.

## Hear the person

```bash
jelly inbox                  # what they did in this project since you last asked
jelly attention              # what still waits on them
jelly comments               # their comments on parts of your jellies: your to-do list
jelly reply plan c3 "Split it into two steps." --resolve   # done; without --resolve the thread stays open
jelly wait plan --since seq:42 --timeout 60 --json         # block until someone else changes it
```

A question the person asks about a whole jelly arrives like a comment, marked
"on the whole jelly"; answer it with `jelly reply`, and they read it in that jelly's header.

`ask`, `call` and `reshape` print `since seq:N`; pass it to `wait
--since` to catch anything after your change. `wait` exits 2 on timeout. In a
script, add `--json` to any command instead of parsing its text.

If several agents share the jelly page, set `JELLY_AS=<your name>` so each has
its own inbox and its changes are attributed. `--mine` on `inbox`, `attention`
or `comments` shows only your jellies; the plain inbox is where you hear the
person act on a jelly another agent made.

## Recipes

Starting points for jellies that are easy to get wrong, built into `jelly`: a
view, verbs, an example state and a guide. Read the guide, make the jelly with
your own state, and change the view and verbs as the job needs.

```bash
jelly recipes                                  # each recipe and when to use it
jelly recipes plan                             # its guide: the state it expects, its verbs
jelly new plan --recipe plan --state my.json --about "Plan: …"
jelly recipes plan --out /tmp/plan             # its files, to edit; then jelly new plan --from /tmp/plan
```

- `review`: you changed code and the person must judge it. What they
  asked for, wired to the changes, before and after, approve or push back.
- `plan`: work with more than three steps. A live track they can
  reorder, veto or gate while you work.
- `triage`: ten to a hundred items to agree on. You sort them into
  lanes; they correct the ones you were unsure of.
- `map`: how a system fits together. Layers, nodes, wires and traces
  that play step by step.
- `scope`: a tradeoff between many parts. Items in or out, levers, and a
  date and capacity bar that move as they play.
- `timeline`: why something happened. Evidence on a time axis,
  competing explanations, and they say which they believe.
- `monitor`: something that runs for minutes or hours. Live charts fed by
  `jelly call`, with pause, stop and limits your loop obeys.
- `quiz`: several people answer in rounds against a clock while you run it.
  Join by name, vote, reveal, scores; you write later rounds from the answers.

## More

```bash
jelly ls                         # this project's jellies; --all for every project
jelly find three issues          # the person named a jelly loosely: which one, and its state
jelly serve --temp               # a throwaway daemon (free port, fresh home); use its URL as JELLY_URL
jelly pull plan ./plan           # the full jelly into a folder: view.html, verbs.js, state.json, assets/
jelly dev plan ./plan            # reshape the jelly on every save in that folder
jelly asset plan shot.png demo.mp4   # files beside the jelly; the view uses assets/shot.png
jelly asset plan --rm shot.png       # remove one
jelly note plan "Cards move next→doing→done; the person decides."   # for the next agent
jelly share plan                 # one read-only HTML file, state and assets baked in; prints where
jelly share plan --link          # a read-only link anyone can open, e.g. on their phone; again = update it
jelly unshare plan               # the link stops working
jelly design                     # the repo's DESIGN.md; views can link /p/<project>/design.css
jelly pin plan                   # keep it; unpinned jellies archive after a quiet week
jelly rm plan
```

A jelly's full name is `project:name`; the project is the git repo you run in
(or `--project`). Short names work inside the same project, in the CLI and in
`jelly.other`. From outside, use the full name.

Live data: verbs have no network, so push it in from your turn, a script or a
loop with `jelly call`.
