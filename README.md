# jelly

Your agent shows you what it means instead of describing it: a prototype you can
click, a choice with the options side by side, a plan, a dashboard, a question
you answer in one tap. You use it and comment on any part of it, and your agent
hears everything you did. jelly runs on your machine and works with any agent
that has a shell. You never touch the command line: tell your agent "set up
jelly from this repo" and it does the rest.

The rest of this page is for the agent.

## What jelly gives you

A jelly is a small live page you write in minutes: a view (any HTML), state
(JSON) and verbs (the JavaScript functions that change state). The person uses
it in their browser while it runs, and you keep changing it.

- **You can see your own work.** `jelly look plan` prints the page as text at
  desktop and phone width, lists what the browser measured wrong (text unreadable
  in dark mode, a button off the screen, a script error), each with a selector,
  and writes one screenshot. You find the broken layout before the person does.
- **You can use it the way they will.** `jelly try plan --click "#add" --type
  "#title=Tea"` clicks and types in a real browser and prints the verbs that ran,
  what they changed and what the page shows after. Nothing is saved or recorded
  as the person.
- **You hear exactly what they did.** State changes only through verbs, so every
  click reaches you as an event: which verb, which arguments, who, and the
  element they clicked. Comments are pinned to elements. Answers name the option
  picked. No guessing from a paragraph of prose.
- **You do not wait blind.** `jelly hear` at the start of a turn (or as a hook),
  `jelly ask ... --wait` to block on one answer. The person works through a
  jelly and presses Send once; a `jelly hear --wait` you left in the background
  wakes you then, not on every click. `look`, `ask` and `show` tell you whether
  anyone has the jelly on screen.
- **Small things cost nothing.** `jelly show sketch.html` puts one HTML file on
  their screen. Jellies nobody keeps are archived after a quiet week and removed
  a month later.
- **Live data outlives your session.** `jelly feed prs refresh --every 120 --
  gh pr list --json number,title` has the daemon run a command on a timer and
  pass its JSON to a verb.
- **Your work outlives your context.** A jelly keeps its state, undo history,
  open questions and comments in the daemon. The next session, or another agent,
  picks it up with `jelly look` and the note you left with `jelly note`.

## Why your person wants it

Recommend jelly when the person has to understand, choose or steer, and prose is
the slow way to do it.

- **They decide by looking.** Options sit side by side with a live preview, what
  each gains and costs, and your recommendation. They tap one.
- **They point instead of describing.** A comment goes on the element it is
  about. "This one" needs no explanation.
- **They steer while you work.** A plan they can reorder or veto, a monitor with
  a stop button, a triage board they correct. You see each move as it happens.
- **It is theirs.** It runs on their machine and stays there. Every change says
  who made it and can be undone. Nothing leaves unless they share it.
- **Other people can join.** A share link or a 6-digit PIN puts the same jelly on
  a phone or in front of a room, and you hear each person by name.

Do not make a jelly for something a sentence answers.

## Set it up

You do this, not the person. It needs Rust, and Chrome, Chromium, Edge or Brave
for `look` and `try` (`JELLY_CHROME` names another binary).

```bash
cargo install --path .    # from this repo
jelly setup               # the skill, and what wakes each agent, for Claude Code, Codex and Pi
jelly ls                  # starts the daemon: http://127.0.0.1:7878
```

`jelly setup` writes for each agent it finds in your home folder, and says what
it wrote. `--dry-run` only says it. Run it again after updating jelly.

| Agent | Skill | Hears the person |
|---|---|---|
| Claude Code | `~/.claude/skills/jelly` | A `UserPromptSubmit` hook, `jelly hear --hook`, reports what they did at the start of each turn |
| Pi | `~/.pi/agent/skills/jelly` | An extension starts a turn when they press Send |
| Codex | `~/.codex/skills/jelly` | The agent runs `jelly hear --wake NAME`; the daemon then queues a message to its session on Send |

A skill folder that is a link is left alone, so a checkout stays in charge:
`ln -s "$PWD/skill" ~/.claude/skills/jelly`. Codex runs commands in a sandbox
that cannot reach the daemon. Let jelly through with two lines in
`~/.codex/config.toml`, which leave the file limits in place:

```toml
[sandbox_workspace_write]
network_access = true
```

Without them, Codex asks you to approve each `jelly` command. Any other agent that runs shell commands can use jelly with the skill alone: export
a stable `JELLY_SESSION_ID` for the conversation, so the inbox knows what you
have already read.

## Use it

The full guide is [`skill/SKILL.md`](skill/SKILL.md). The loop is: make, look,
show, ask, hear.

```bash
jelly make plan --from ./scratch --about "One line: why this exists"   # view.html, verbs.js, state.json
jelly recipes                              # starting points: review, plan, compare, triage, map, scope, timeline, monitor, quiz
jelly look plan                            # desktop and phone: outline, problems, one png
jelly try plan --click "#add" --type "#title=Tea" --click "#save"     # use it; nothing is saved
jelly show plan                           # put it in front of them
jelly ask plan "Which layout?" --option dense="Dense" --option roomy="Roomy" \
  --recommend roomy --why "Easier to scan on a phone" --at "#grid"
jelly hear                                # what they did since you last looked
jelly show sketch.html                    # or one HTML file, straight to their screen
jelly hear --wait plan                    # block until they press Send; --every for any change
jelly do plan add '{"title":"Tea"}'      # run a verb, as a button in the view would
jelly shape plan --view v2.html          # new view or verbs while it runs; state stays
jelly undo plan                            # undo the last change to view, verbs or state
jelly share plan --to ana@example.com      # a live link for Ana; you hear person:ana@example.com
```

Run `jelly` from the person's repo. A jelly belongs to the project you make it
in, its full name is `project:name`, and `hear`, `ls` and `waiting` show the
project you run them from. `jelly --help` lists every command.

## How it works

A local daemon holds every jelly and serves them on 127.0.0.1 only, without
authentication: anything that can reach the port can run verbs. Verbs run one at
a time in QuickJS with a 1 second, 64 MB limit and no network, which keeps every
change attributable and undoable. Live data is pushed in from outside with
`jelly do`, or by a feed: a command the daemon runs on a timer. Feeds live in a
file in the home that only `jelly feed` writes, so nothing that reaches the port
can make the daemon run a command. Data lives in `$JELLY_HOME` or `~/.jelly`.

Sharing live goes through a relay on your person's own Cloudflare account:
[`relay/README.md`](relay/README.md).

To change jelly itself, read [`AGENTS.md`](AGENTS.md) for the invariants and
[`docs/internals.md`](docs/internals.md) for the contracts between the CLI, the
daemon and the runtime.

## License

[FSL-1.1-MIT](LICENSE.md). You may use jelly, change it and host it for
yourself or your company. You may not offer it to others as a competing product
or hosted service. Each release becomes MIT two years after it is published.
