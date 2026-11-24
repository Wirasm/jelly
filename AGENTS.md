# jelly: agent notes

Read `README.md` for what jelly is and `docs/internals.md` for the contracts between
the CLI, the daemon and the runtime. Vocabulary: **jelly** (one live app: view, state,
verbs), **jelly page** (the internal jelly, id `page`, that lays out the others),
**ask** (a question for the person, with its answer), **verb**, **shape** (swap a
running jelly's view or verbs), **inbox** (what the person did that a reader has not
heard yet; `jelly hear` reads it), **event**. A command, its route, its event kind and
its store function share one word: make, do, shape, keep, ask, unask. Do not call a jelly an app, widget, canvas or artifact. One name per
thing: no aliases for commands, flags or runtime fields.

## Invariants

- Agents show, people react. An agent lays out options with tradeoffs, previews and
  a recommendation; the person picks, says why, or comments. Never hand the person
  an empty input field.

- State changes only through verbs. Do not add an API that writes state directly;
  undo, events and attribution all depend on it.
- Verbs run one at a time under the store lock in `server.rs`.
- Shape and create refuse verbs that do not load (`verbs::check`).
- An event's `state` field goes to live listeners only, never into `events.jsonl`.
- Browsers get events over one WebSocket per page (`/api/ws`). Do not give views
  `EventSource`s: each one holds an HTTP connection, and a page with a few jellies
  hits the browser's six-per-host limit and stalls every request.
- Everything the runtime adds to a view lives in one `<jelly-ui>` element beside
  `<body>`, never among the view's elements. The runtime does not touch a view's layout.
- An ask is data. The view draws it (`data-ask="<id>"`), or the runtime shows it in
  its stack of questions. Do not add layout rules for where a card goes.
- Comments (`comments`) and asks (`asks`) live beside state, not in it. Undo does not touch them.
- A jelly's `version` changes only with its view, verbs or state. Comments, asks and
  answers do not bump it; an event's `seq` counts everything.
- A jelly's id is `project:name`; the project is the part before the colon. Short
  names resolve in the CLI (current project) and the runtime (the view's project).
- Assets live in `~/.jelly/assets/<id>/`, outside state and undo. On Windows the folder
  is `project+name`, because `:` is not allowed in a file name (`disk::name`). Views are
  served at `/j/<id>/` so `assets/<file>` resolves; keep that trailing slash.
- Never put network access in verbs. Live data is pushed in with `jelly do`, or by a feed.
- The daemon runs commands only from `feeds.json` in the home, which the CLI writes
  to disk (`jelly feed`). Never add an HTTP route that sets or changes a feed: the
  port has no authentication, so that route would let any page run a command.
  `/api/update` is the one route that starts a command, and the request chooses
  nothing about it: `npm install -g jellyware@latest`, then `jelly setup`.
- A change by `feed` is not someone using the jelly: it leaves `updated_ms` and
  `archived` alone, so a fed jelly nobody opens still goes quiet.
- `send` is the person handing the turn back. `jelly hear --wait` returns on it and
  on nothing else unless `--every` is given. Do not make other events wake a wait.
- `assets/jelly.css` is the one source of jelly's own look. The daemon puts it
  inside the runtime it serves, so views, shared pages and exported files all
  carry it. The runtime's controls, the jelly page, the ask page and the recipes
  take their colours from its `--jy-*` tokens; do not add colour values to them.
  The file must never contain a closing style tag: views put it in a style element.
- jelly's look is for jelly's parts only. A view gets the tokens only by asking
  (`<html class="jy">`); nothing from jelly.css may style a view that did not.
- Recipes are frames and starting points, not a component library. A recipe that
  holds the person's product (`compare`) gives the product's screens no CSS.

## Check

```bash
cargo test
cargo clippy
```

For a manual run, use a throwaway daemon so your real jellies are untouched:
`jelly serve --temp` picks a free port and a fresh home, prints `jelly on URL` first,
and removes the home when stopped. Pass that URL with `--url` or `JELLY_URL`.

## Release

Nobody releases by hand. Every merge to `development` that changes what ships
(`src`, `assets`, `skill`, `npm`, `build.rs`, the Cargo files) is published to
npm and GitHub by `.github/workflows/release.yml`, one patch above the latest
release, and tagged `vX.Y.Z`. Do not bump the version or push a tag. The
version a binary reports comes from that tag, not from `Cargo.toml`: a build
from a checkout says `0.1.4` at the release commit and `0.1.4+3` three commits
later. For a new minor or major, set `version` in `Cargo.toml`; the next
release uses it. The relay is deployed separately: `relay/README.md`.
