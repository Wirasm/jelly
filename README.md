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
jelly ls                                         # is the daemon up? (the person runs `jelly serve`)
jelly new plan --view view.html --verbs verbs.js --about "One line: why this exists"
jelly focus plan                                 # put it in front of them
jelly ask plan "Which layout?" --option dense="Dense" --option roomy="Roomy" \
  --recommend roomy --why "Easier to scan on a phone" --at "#grid"
jelly inbox                                      # what they did since you last looked
jelly see plan                                   # what is on screen, as text
```

When piping into `head` or `grep -q`, stdout BrokenPipe ends the command quietly.
Finite commands finish their changes before printing. `events`/`watch` and `dev`
stop when an output write detects closure; while idle, they keep waiting. `wait`
keeps its event selection and timeout behavior, detecting closure only when it
prints a qualifying event. `serve` continues serving when stdout closes.

The full guide is [`skill/SKILL.md`](skill/SKILL.md). Install it as a skill so every
session knows it: `ln -s "$PWD/skill" ~/.claude/skills/jelly`.

## For the person: install

```bash
cargo install --path .
jelly serve            # http://127.0.0.1:7878, data in ~/.jelly
```

To let agents hear you without asking, add `jelly inbox --hook` as a Claude Code
`UserPromptSubmit` hook. It prints nothing when there is nothing new.

## How it works

A local daemon holds every jelly and serves them on 127.0.0.1 only, without
authentication: anything that can reach the port can run verbs. Verbs run one at a
time in QuickJS with a 1 second, 64 MB limit and no network, which keeps every
change attributable and undoable. Live data is pushed in from outside with
`jelly call`. Contributors: read [`AGENTS.md`](AGENTS.md).

Browser snapshots belong to the current served view. The daemon supplies ownership
evidence in view boot data and the bundled runtime echoes it when posting a
snapshot. Missing evidence is rejected; stale evidence returns 409 and the runtime
reloads. After upgrading the daemon, reload older open pages and update tokenless
snapshot callers before they can contribute snapshots again. This is an internal
daemon/runtime protocol, not authentication.
