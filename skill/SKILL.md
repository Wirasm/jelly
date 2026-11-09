---
name: jelly
description: Show things to the person instead of writing walls of text. Use jelly whenever a picture, a prototype, a live dashboard, a plan, a diff, a comparison or a choice between options would explain better than prose, and whenever you need the person to choose. Also use it to read what the person did or said in a jelly, to change one while it runs, or when they say "show me" or "jelly".
---

# jelly

jelly is how you show the person things. A jelly is a small live page you write in
minutes: a clickable prototype, an animated explainer, a 3D scene, a dashboard, a
plan, options side by side. The person sees it on the jelly page
(http://127.0.0.1:7878), uses it, answers your questions and comments on any part
of it. You hear all of it.

**Show, then let them react.** Never hand the person an empty question. Do the
thinking: lay out the options, what each gains and costs, what each looks like,
and which you recommend. Their job is to react, not to write your spec.

Check the daemon first: `jelly ls` (this project's jellies; `--all` for every project). If it cannot be reached, ask the person to run
`jelly serve`; do not start it yourself unless they asked.

## Make a jelly

Write three files in a scratch folder, never in the person's repo:

- `view.html`: any page. `window.jelly` exists before your scripts run.
- `verbs.js`: `const verbs = { name(state, args, { by, at_ms }) { … } }`. Verbs are
  the only way state changes; a button and you call the same verb. Change `state`
  in place or return a new one; throw to refuse. `const verbs = {};` for a page
  that only shows.
- `state.json`: the starting state.

```bash
jelly new plan --view view.html --verbs verbs.js --state state.json --about "One line: why this exists"
jelly reshape plan --view v2.html    # new view or verbs, same state; open pages reload
jelly call plan add '{"title":"…"}'  # run a verb yourself; '@args.json' or - for big args
jelly set plan --at /title '"Better title"'   # write state yourself, no verb needed
jelly set plan --at /draft --rm      # remove a key; `jelly set plan @state.json` replaces it all
jelly undo plan                      # undo the last change to view, verbs or state
jelly pin plan                       # keep it; unpinned jellies archive after a quiet week
jelly rm plan
```

In the view:

```js
jelly.onState((state, event) => render(state)); // now, and after every change by anyone
jelly.call("add", { title });                   // runs a verb; rejects with its error
jelly.read("other"); jelly.watch("other", fn);  // another jelly's state
jelly.focus("other");                           // open another jelly
jelly.asset("shot.png");                        // URL of a file added with jelly asset
jelly.itemsAt("#thumbs")[0]?.answer?.option;    // what they picked in a question about #thumbs
```

Keep data in state, not in page variables or localStorage, so you and the person
see the same thing. Give elements stable `id`s: questions and comments point at them.

## Show it

```bash
jelly working plan "rebuilding the header"   # the person sees it and waits
jelly done plan
jelly see plan                 # check your work: the page as text, plus what waits on the person
jelly see plan --width 390     # at phone width; add --png when layout matters
jelly see plan --full          # screenshot the whole page, not just the first screen
jelly focus plan               # open it full size on their screen
```

Always look before you show: run `jelly see`, and fix what is off before you focus
the jelly or ask about it. `see` lists the errors your page threw (`error` lines)
and questions whose `--at` matches nothing (`!` lines): fix both. Clear your own
test clicks with `jelly set` before the person uses the jelly. Add `assets/poster.png`, a frame that tells the story on
its own, and the jelly page uses it as the thumbnail.

## Ask the person: attention

Attention is everything you need from the person. Each question shows as a card
in the page itself, right below the element `--at` points to (or at the top), and
in their attention dock on the jelly page. They answer in place; an answered card
shrinks to one line they can change.

One question on an existing jelly, about one part of it:

```bash
jelly ask plan "Which hero direction?" --option bold="Bold gradient" \
  --option quiet="Quiet white|Swaps to a plain hero" --recommend quiet \
  --why "Calmer, and the button stands out more" --at "#hero"
jelly ask plan "Anything missing before I ship?"     # answered in words
jelly look plan "Check the footer links" --at "#footer"   # they answer "seen" or with a word
```

Several related questions, with options laid out side by side, make an ask page:

```bash
jelly ask sharing --spec spec.json   # asking again starts a new round; open questions are withdrawn
```

```json
{
  "title": "Sharing a jelly with a colleague",
  "context": "Why these matter, in a short paragraph.",
  "questions": [
    { "id": "host", "question": "Where should shared jellies live?", "context": "…",
      "recommend": "relay", "why": "One sentence on why",
      "options": [
        { "id": "relay", "label": "A small hosted relay", "does": "One or two sentences.",
          "gains": ["…"], "costs": ["…"], "effort": "M · two days",
          "preview": "<!doctype html>… any HTML: a mockup, an animation, a chart, a 3D scene …" }
      ] }
  ]
}
```

- Every question is an attention item, answered with an option, words, or both.
  "None of these" is an answer in words: read it, and ask a better round.
- Make every preview real and different. It runs live in a sandbox: scripts, CSS
  animation, canvas, WebGL and CDN imports all work. Two to four options.
- Recommend one, and say why. If you could have decided yourself, do not ask.
- `jelly withdraw plan a2` when a question no longer applies.
- `jelly call sharing applied '{"note":"Built in 3f2a1c"}'` when you acted on the answers.

## Hear the person

If the person has the inbox hook, what they did since your last turn arrives at
the start of your turn ("The person acted on your jellies…"). Act on it. Otherwise:

```bash
jelly inbox                  # what they did in this project since you last asked
jelly attention              # what still waits on them
jelly see plan               # includes their answers and recent actions
jelly wait [plan]            # block until someone other than you changes something
jelly events [plan]          # stream every event as JSON lines
```

Comments: the person can Option-click any element of any jelly and say what should
change. Answer on the same thread, and change the jelly if they asked for it:

```bash
jelly comments
jelly reply plan c3 "Split it into two steps." --resolve   # done: closes the thread
jelly reply plan c3 "Which date, the 12th or the 19th?"    # asking back: stays open
```

Set `JELLY_AS=<your name>` when several agents share one jelly page.

## Projects and names

A jelly is named `project:name`. The project is the git repo you run `jelly` in
(or `--project`, `JELLY_PROJECT`). Use short names: `jelly new plan` in the archon
repo makes `archon:plan`, and `jelly call plan …` there finds it. Elsewhere use the
full name. Inside a view, `jelly.read("plan")` means this project's plan. The
person sees each project on its own page, `/p/<project>`. `jelly move NAME OTHER`
moves a jelly; `jelly restore NAME` brings back an archived one.

## Files, assets, design, sharing

```bash
jelly note plan "Cards move next→doing→done; the person decides what moves to doing."  # for the next agent
jelly pull plan ./plan           # view.html, verbs.js, state.json, assets/
jelly dev plan ./plan            # changes the jelly on every save
jelly asset plan shot.png demo.mp4 model.glb   # beside the jelly; use as assets/shot.png
jelly design                     # the project's DESIGN.md, if the repo has one
jelly share plan                 # one read-only HTML file, state and assets baked in
```

A repo's DESIGN.md (github.com/google-labs-code/design.md) becomes CSS variables at
`/p/<project>/design.css` (`--color-primary`, `--rounded-md`, …). Use it when it
helps the jelly feel like the product. Do not inline big files as base64; add them
as assets.

## Live data

Verbs have no network. To keep a dashboard live, push data in from outside, from
your own turn, a script or a cron job: `jelly call dash add '{"t":…,"v":…}'`. The
view can also fetch public APIs itself in the browser.

## Limits

A verb runs at most 1 second and has no file, network or process access. The view
has the full browser. The daemon listens on 127.0.0.1 only and has no login.
