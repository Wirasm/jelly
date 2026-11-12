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

The full guide is [`skill/SKILL.md`](skill/SKILL.md). Install it as a skill so every
session knows it: `ln -s "$PWD/skill" ~/.claude/skills/jelly`.

## For the person: install

```bash
cargo install --path .
jelly serve            # http://127.0.0.1:7878, data in ~/.jelly
```

To let agents hear you without asking, add `jelly inbox --hook` as a Claude Code
`UserPromptSubmit` hook. It prints nothing when there is nothing new or no daemon
is running. A daemon that does not answer within a second, reader identification,
decoding and daemon read or progress-commit failures appear in hook output as
`jelly inbox failed: ...`. Failed reads do not
advance progress. Claude's
`CLAUDE_CODE_SESSION_ID` environment variable keeps one inbox reader across
commands and resumed conversations. JSON `session_id` identifies hook invocations
only. To share that progress with ordinary commands, supply the same conversation
ID through `inbox --session-id` or `JELLY_SESSION_ID` (or use the same explicit
`--as` / `JELLY_AS` name for both).

Inbox identity uses explicit `--as` / `JELLY_AS` first, then
`inbox --session-id` / `JELLY_SESSION_ID`, then `CLAUDE_CODE_SESSION_ID`, then
`session_id` from hook stdin. Other agents should export a stable, distinct
conversation token as `JELLY_SESSION_ID`. Without one, the invoking parent
process's PID and start time identify the reader for that process's lifetime;
a new parent starts a new reader. Shared names or conversation tokens intentionally
share progress. Event attribution still defaults to `agent`.

Progress is stored in the daemon's home (`inbox-readers.json`), per reader and
project, and survives daemon restarts. A new reader sees the last hour; later
reads use event positions. Upgrading leaves old client `inbox/` timestamps unused,
so each new reader can replay the recent hour. Use a matching CLI and daemon.
The non-consuming POST `/api/inbox` accepts `{ "project": "...", "reader": ... }`
with reader forms `{ "kind": "named", "name": "..." }`,
`{ "kind": "conversation", "id": "..." }`, or
`{ "kind": "parent", "pid": 123, "start_time": 456 }` (Unix seconds).
Omitting `project` or using an empty string reads all projects. The response is
`{ "events": [...], "through": 123 }`; events include `text`. The first read
registers a fixed one-hour starting window, so uncommitted events remain available
on retry even after that hour passes. After decoding and successfully writing and
flushing output, the CLI sends POST `/api/inbox/commit` with the same project and
reader plus the returned `through` position. Commits only move that reader's
progress forward and leave events newer than `through` unread. Lost or undecodable
read responses do not consume events. Ambiguous output, failed acknowledgements
or client crashes may replay events. GET `/api/inbox?project=...&since=...` remains
a non-consuming timestamp query returning an array.

The daemon refuses startup if an existing event log cannot be read, contains
malformed events or is nonempty and missing its final newline. This prevents
reusing positions already passed by inbox readers or corrupting the final record
on the next append.

`~/.jelly/events.jsonl` is trimmed once it passes 8 MiB, to its newest 4 MiB plus up
to 1 MiB of your older events, so the inbox and `jelly see` still have them. A trim
keeps every event's position, and inbox reads go back only as far as the reader's
position.

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
