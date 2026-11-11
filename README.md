# jelly

Allow your agent to show you what it means instead of telling you: a prototype you
can click, a choice with the options side by side, a plan, a dashboard, a 3D
render, a question you answer in one tap. You use it, comment on any part of it,
and your agent hears everything you did. jelly runs on your machine and works with
any agent that has a shell.

## For agents

jelly gives you a place to put jellies in front of the person you work for: small
live pages you write in minutes that they use, answer and shape with you. Reach for
it whenever a picture, a prototype, a comparison or a choice would land better than
prose, and whenever you need them to choose.

- **Show anything.** A jelly is a view (any HTML), state (JSON) and verbs (the JS
  functions that change state). Prototypes, three.js, charts, forms, animations.
  No component kit, no ceiling.
- **Hear their answer.** Every click is a verb call with an author. `jelly ask` and
  `jelly look` put a question on the exact element it is about. Comments attach
  to elements. You get all of it from `jelly inbox` (or a hook at the start of your
  turn), or block on `jelly wait`.
- **Get choices made.** `jelly ask --spec` lays out options with live previews,
  gains, costs and your recommendation. They pick, add a note, or add their own question.
- **See what you made.** `jelly see NAME` prints the page as text, with everything
  waiting on them and what they did lately. `--png` when layout matters.
- **Keep working on it together.** Reshape while it runs and state stays. Write
  state yourself with `jelly set`. Undo the last change. Notes for the next agent. One page per project. Share as one file.

```bash
jelly ls                                         # list jellies; starts the daemon on first use
jelly new plan --view view.html --verbs verbs.js --about "One line: why this exists"
jelly focus plan                                 # put it in front of them
jelly ask plan "Which layout?" --option dense="Dense" --option roomy="Roomy" \
  --recommend roomy --why "Easier to scan on a phone" --at "#grid"
jelly inbox                                      # what they did since you last looked
jelly see plan                                   # what is on screen, as text
```

The full guide is [`skill/SKILL.md`](skill/SKILL.md). Install it as a skill so every
session knows it: `ln -s "$PWD/skill" ~/.claude/skills/jelly`.

## For the person: install

```bash
cargo install --path .
jelly ls               # starts on first use: http://127.0.0.1:7878
```

The first client command starts a detached daemon if needed, waits for it, and
prints one startup line on stderr. Data lives in `$JELLY_HOME` or `~/.jelly`, just
as with `jelly serve`. No login service or extra install step is needed.

`jelly serve` still runs in the foreground for manual use and startup diagnostics.
For an isolated run: `jelly serve --port 7879 --home /tmp/jelly-test`, then use
`jelly --url http://127.0.0.1:7879 ls`. On-demand startup applies only to the default
URL; other `--url` / `JELLY_URL` targets must already be running.

To let agents hear you without asking, add `jelly inbox --hook` as a Claude Code
`UserPromptSubmit` hook. It starts the default daemon silently, suppresses startup
errors, and prints nothing when there is nothing new. Outside a project it still
attempts startup, then exits successfully without inbox output.

## How it works

A local daemon holds every jelly and serves them on 127.0.0.1 only, without
authentication: anything that can reach the port can run verbs. Verbs run one at a
time in QuickJS with a 1 second, 64 MB limit and no network, which keeps every
change attributable and undoable. Live data is pushed in from outside with
`jelly call`. Contributors: read [`AGENTS.md`](AGENTS.md).
