# jelly

Agents share anything with their humans visually, and ask for the human's input
in a way the human understands.

jelly renders jellyware: small live apps that agents make in minutes, such as a
clickable prototype, a decision with live previews, a 3D render, an animation, a
dashboard or a form. The person uses them, and the agent hears what they did.

A **jelly** is a view (HTML), its state (JSON) and the verbs that change that state
(JavaScript), held together by a local daemon under one name. Jellies appear on the
jelly page in the browser, one page per project. An agent makes one from the CLI,
the person uses it, and either side can call its verbs, read its state, or reshape
it while it runs.

```
agent ── jelly CLI ──┐                ┌── jelly page (browser)
                     ├── jelly daemon ┤      ├── work
person ── clicks ────┘   view, state, │      ├── design
                         verbs, events└──    └── ...
```

## Why it's built this way

- **Verbs are the only way to change state.** A button and an agent's `jelly call`
  run the same verb, one at a time, inside the daemon. That one rule gives events,
  attribution (`by`), history and undo, with no merge logic in any page.
- **No file per jelly, no server per jelly.** The agent never manages paths, ports
  or data files. It names a jelly and talks to it.
- **Reshape keeps state.** New view or verbs swap in while the jelly runs; open views
  reload and pick up the same state. Verbs that fail to load are refused.
- **The jelly page is a jelly too.** It is a stream of every unpinned jelly in the
  order agents made them, each under the agent's one line of why, and a board with
  your attention queue and pinned jellies. `jelly focus NAME` opens one full size.
- **Attention is everything an agent needs from you.** An *ask* wants an answer, a
  *look* wants a glance. Both sit on the jelly next to what they are about and in
  your attention queue, where one click answers them.
- **One page per project.** A jelly is named `project:name`, the project being by
  default the git repo the agent ran `jelly` in. Names only need to be unique in a
  project, and a short name means "in this project". `/p/<project>` shows only that
  project's jellies.
- **Quiet jellies archive themselves.** An unpinned jelly nobody touched for a week
  leaves the jelly page, unless it still waits on the person. Any change brings it back.
- **Comments are the ask.** Option-click any part of any jelly to pin a comment. The
  agent answers on the same pin, by reshaping, replying or both.

## Use

```bash
cargo install --path .
jelly serve                       # http://127.0.0.1:7878, data in ~/.jelly
jelly open
jelly watch                       # every change, as JSON lines
```

The repo ships no jellies. Agents make them while they work: they write a view,
verbs and starting state for what the moment needs, and run `jelly new`. Give an
agent `skill/SKILL.md` so it knows how, for example
`ln -s "$PWD/skill" ~/.claude/skills/jelly`.

Two things come built in, because every agent needs them:

- **`jelly ask`** opens a decision: options with gains, costs, effort and a live
  preview each, plus the agent's recommendation. The person picks, likes, dislikes,
  notes or sends them back; the agent waits with `jelly wait`.
- **Comments anywhere:** Option-click any element of any jelly to pin a comment.
  Agents read them with `jelly comments`.

- **Assets:** `jelly asset NAME shot.png` stores files beside a jelly, outside its
  state and undo. Views use them as `assets/shot.png`.
- **Share:** `jelly share NAME` writes one read-only HTML file, with state and
  assets baked in, that opens anywhere.
- **Inbox:** `jelly inbox --hook`, run as a Claude Code `UserPromptSubmit` hook, tells
  the agent at the start of each turn what the person did in this project's jellies.

`jelly --help` lists the rest.

## Dogfooding

jelly is built with jelly. The agents working on it keep a `work` jelly (the board:
next, doing, done, and how we work) and a `design` jelly (rules, decisions, open
questions) on the jelly page. Those live in `~/.jelly`, not in this repo, and the
agents reshape them as the work changes.

## Layout

| Path | What |
| --- | --- |
| `src/store.rs` | Jellies, verbs-only state changes, undo, events, persistence |
| `src/verbs.rs` | Runs verbs in QuickJS with a 1 s and 64 MB limit |
| `src/server.rs` | HTTP API, events, view injection, assets, snapshots, inbox, archiving |
| `src/main.rs` | The CLI |
| `assets/runtime.js` | `window.jelly`, injected into every view |
| `assets/snapshot.js` | The read-only `window.jelly` inside a shared snapshot |
| `assets/surface.*` | The jelly page (an internal jelly, id `surface`) |

Data lives in `~/.jelly` (or `JELLY_HOME`): one JSON file per jelly with its undo
history, `assets/<jelly>/` for its files, and `events.jsonl` with every change.

## Not built yet

- Hosted sharing: a link a colleague can use and answer, not only a read-only file.
- Verbs with access beyond their own state. Live data is pushed in from outside
  with `jelly call` instead.

The daemon binds to 127.0.0.1 only and has no authentication. Anything that can
reach the port can call verbs and reshape jellies.
