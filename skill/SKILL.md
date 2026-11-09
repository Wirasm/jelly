---
name: jelly
description: Show things to the person instead of writing walls of text. Use jelly whenever a picture, a prototype, a live dashboard, a plan, a diff, a comparison or a choice between options would explain better than prose, and whenever you need the person to decide something. Also use it to read what the person did or said in a jelly, to reshape one while it runs, or when they say "show me" or "jelly".
---

# Jelly

jelly is how you show the person things. You can build anything in a jelly in minutes: a clickable
prototype, an animated explainer, a 3D scene, a live dashboard pulling real data, a
plan with dependencies drawn out, a side-by-side of options. The person sees it at
http://127.0.0.1:7878 and answers by clicking, picking and pinning comments.

**Show, then let them react.** Never hand the person an empty question with an input
field. Do the thinking: lay out the options, what each gains and costs, what each
would look like, and which you recommend. The person's job is to react, not to
write your spec.

Check the daemon first: `jelly ls`. If it cannot be reached, ask the person to run
`jelly serve`; do not start it yourself unless they asked.

## Ask for a decision: `jelly ask`

For any choice, use a decision jelly. One spec can hold one question or several
related ones. Every option shows its gains and costs; the person picks, adds a
note to any question, and can add their own questions or context at the end.

```json
{
  "title": "Open questions about sharing",
  "context": "Why these matter, in two short paragraphs.",
  "questions": [
    { "id": "host", "question": "Where should shared jellies live?", "context": "...",
      "recommend": "worker", "why": "One sentence on why",
      "options": [
        { "id": "worker", "title": "A small hosted relay", "summary": "One or two sentences.",
          "gains": ["..."], "costs": ["..."], "effort": "M · two days",
          "preview": "<!doctype html>... any HTML: a mockup, an animation, a chart, a 3D scene ..." }
      ] }
  ]
}
```

A single question can skip the wrapper: `{"question", "context", "recommend", "why", "options"}`.

```bash
jelly ask sharing --spec spec.json        # same name again = a new round, old one kept
jelly wait sharing                        # or let the inbox tell you next turn
jelly state sharing /status               # open | answered | applied
jelly state sharing /questions/0/answer   # {kind: pick|none, option, by, at_ms} or null
jelly state sharing /questions/0/note     # {text, by, at_ms} or null: context or a condition
jelly state sharing /added                # questions or context the person added for you
jelly call sharing applied '{"note":"Built in 3f2a1c"}'   # when you have acted on it
```

- Make every preview real and different. It runs live in a sandbox: scripts, CSS
  animation, canvas, WebGL and CDN imports all work. Two to four options.
- Read the notes and the added items, not only the picks. "None of these" comes
  with a note saying what is missing: ask again with a better round.

## Put something in front of the person: attention

Attention is everything you need from the person. It shows on the jelly itself,
next to the element it is about, and in the person's attention queue on the jelly
page, where they can answer in one click.

```bash
# An ask: wants an answer. Options are id=label or id=label|what it does.
jelly ask plan "Which hero direction?" --option bold="Bold gradient" \
  --option quiet="Quiet white|Swaps to a plain hero" --recommend quiet \
  --why "Calmer, and the CTA stands out more" --at "#hero"
jelly ask plan "Anything missing before I ship?"          # free-text answer
# A look: wants a glance. They answer "seen" or with a word back.
jelly look plan "Check the footer links" --at "#footer"
jelly attention                       # what is still waiting on the person
jelly withdraw plan a2                # it no longer applies
jelly state plan                      # answers are in `jelly show plan` under attention
```

Prefer an ask on the jelly it is about over a separate ask jelly. Use
`jelly ask NAME --spec` (above) when there are several related questions with
previews and tradeoffs. Never ask a bare question you could have answered
yourself: recommend an option and say why.

## While you change a jelly

```bash
jelly working plan "rebuilding the header"   # the person sees it and waits
jelly reshape plan --view v2.html
jelly done plan
jelly focus plan                             # open it full size on their screen
```

Look before you show: screenshot the jelly (for example with playwright) and
check it, ideally with a reviewer that sees only the screenshots, before you
focus it or ask about it. Add `assets/poster.png`, a frame that tells the story
on its own, and the jelly page uses it as the thumbnail.

## See a jelly the way you need to

```bash
jelly see plan                 # what it is, verbs, state shape, what waits on the person,
                               # what they did lately, and what is on screen as text
jelly see plan --width 390     # rendered at phone width
jelly see plan --png           # also a screenshot path, when layout matters
jelly note plan "Cards move next→doing→done; Rasmus decides what moves to doing."
```

Start every session on an existing jelly with `jelly see`. Check your own work
with it before you focus a jelly or ask about it. Leave a `jelly note` for the next
agent when you learn something about the jelly that is not in its view.

## Work from files

```bash
jelly pull plan ./plan         # view.html, verbs.js, state.json, assets/
jelly dev plan ./plan          # reshapes on every save; open views reload
```

The jelly stays the source of truth, so scratch folders can come and go.

## Use the project's design

If the repo has a DESIGN.md (github.com/google-labs-code/design.md), jelly picks
it up when you run `jelly new` there. `jelly design` prints it; its tokens are CSS
variables at `/p/<project>/design.css` (`--color-primary`, `--rounded-md`, …):

```html
<link rel="stylesheet" href="/p/archon/design.css">
```

Use it when it helps the jelly feel like the product; ignore it when it doesn't.

## Make any other jelly

Write three files in a scratch directory, never in the person's repo.

`verbs.js` declares `const verbs = { ... }`. Each verb is `(state, args, { by, at_ms })`;
it changes `state` in place or returns a new state, and throws to refuse.

`view.html` is any page. `window.jelly` exists before your scripts run:

```js
jelly.onState((state, event) => render(state));  // now, and after every change by anyone
jelly.call("verb", { ...args });                   // runs a verb; rejects with its error
jelly.watch("other", (state) => ...);              // another jelly's state, live
```

Keep data in state and change it through verbs, never in page variables or
localStorage, so you and the person see the same thing. A read-only page can use
`const verbs = {};`.

```bash
jelly new plan --view view.html --verbs verbs.js --state state.json
jelly reshape plan --view view2.html     # new view or verbs, same state; open views reload
jelly undo plan                          # last change of any kind, by anyone
jelly pin plan                           # keep it; unpinned jellies are throwaway
jelly rm plan
```

## Hear the person

Comments are how the person asks for changes. They Option-click any element of any
jelly and pin a comment to it. Answer on the same pin.

If the person has the inbox hook installed, what they did since your last turn
arrives at the start of your turn ("The person acted on your jellies…"). Act on
it. Without the hook, run `jelly inbox` yourself, or block with `jelly wait`.

```bash
jelly inbox                         # what the person did in this project since you last asked
jelly wait                          # blocks until the person does anything, in any jelly
jelly comments                      # open comments on every jelly: element selector, quote, thread
jelly reply plan f3 "Split it into two steps. Reshaped v4." --resolve   # done: closes the thread
jelly reply plan f3 "Which date, the 12th or the 19th?"                 # asking: stays open
```

- Do what the comment asks (usually a `jelly reshape` or a verb call), then reply
  saying what you changed. Use `--resolve` when it is done. If the person replies
  on a resolved thread, it opens again and `jelly wait` wakes you.
- Give elements stable `id`s so comments still point at the right thing after a reshape.
- `jelly wait NAME` waits on one jelly. `jelly watch [NAME]` streams every event.
  Events say `by` and which state pointers changed.
- Set `JELLY_AS=<your name>` when several agents share one jelly page.

## Projects and names

A jelly is named `project:name`. The project defaults to the git repo you run `jelly`
from (or `--project`, `JELLY_PROJECT`). Use short names: `jelly new plan` in the
archon repo makes `archon:plan`, and `jelly call plan …` there finds it. From
anywhere else, use the full `archon:plan`. Inside a view, `jelly.show("plan")` and
`jelly.read("plan")` mean this project's plan. The person sees each project on its
own page, `/p/<project>`. `jelly project NAME OTHER` moves a jelly.

Unpinned jellies that nobody touches for a week are archived: hidden, not deleted.
Any change brings one back, as does `jelly restore NAME`. Pin what the person should keep.

## Images, video, models, data

Do not inline big files as base64. Add them as assets and refer to them by a
relative path:

```bash
jelly asset plan shot.png demo.mp4 model.glb     # stored beside the jelly, not in state
```
```html
<img src="assets/shot.png">   <!-- or jelly.asset("shot.png") from script -->
```

## Live data

Verbs have no network. To keep a dashboard live, push data in from outside, from
your own turn, a script or a cron job: `jelly call dash add '{"t":…, "v":…}'`.
The view can also fetch public APIs itself in the browser.

## Share it

`jelly share NAME` writes `NAME.html`: one read-only file, with state and assets
baked in, that opens in any browser. Send that file, or publish it with your
artifact tool if you have one. Clicks that would change state do nothing in it.

## The jelly page

New jellies appear at the bottom of the person's stream under your `--about`
line, so always write one: it is the why. Pinned jellies sit on the board beside
the stream; pin what the person should keep (`jelly pin`). To put a jelly in front
of the person full size:

```bash
jelly focus plan
```

Inside a view, `jelly.show("other")` does the same, for links between jellies.

## Limits

A verb runs at most 1 second and has no file, network or process access. The view
has the full browser. The daemon listens on 127.0.0.1 only.
