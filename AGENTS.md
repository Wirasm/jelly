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

- State changes only in `Store`: a verb, `set`, `undo`, `make --replace`, or a
  change mirrored from the relay. Every one goes through `commit` or `record`,
  which saves it and logs an event naming who made it; verb calls and sets also
  keep an undo step. Do not add a way to write state that skips the event, or a
  new kind of edit that skips the undo step: undo, the inbox and attribution
  depend on it.
- A change a shared jelly's relay can make too is an `op::Change`, made through
  `server::apply`: in the store, or on the relay while the jelly is shared. The
  relay makes it again in `relay/src/ops.ts`; `tests/ops.json` says what both
  must do, and both test suites run it. Change the fixture with the change.
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
- A project has no id of its own. It exists while a jelly is in it, or while its
  folder `~/.jelly/projects/<project>/` does (`jelly project NAME`, or a
  DESIGN.md). Renaming one moves every jelly in it (`Store::move_project`), so
  their ids change; do not add a second name or a display label for a project.
- Assets live in `~/.jelly/assets/<id>/`, outside state and undo. On Windows the folder
  is `project+name`, because `:` is not allowed in a file name (`disk::name`). Views are
  served at `/j/<id>/` so `assets/<file>` resolves; keep that trailing slash.
- Never put network access in verbs. Live data is pushed in with `jelly do`, or by a feed.
- No request chooses a command. The daemon runs the person's own commands only
  as feeds, from `feeds.json` in the home, which the CLI writes to disk (`jelly
  feed`). Never add an HTTP route that sets or changes a feed: the port has no
  authentication, so that route would let any page run a command. Besides the
  daemon's own restart and the browser for renders, two routes start a fixed
  command. `/api/update` runs `npm install -g jellyware@latest`, then `jelly
  setup`, and the request chooses nothing about it. `/api/wake` runs `codex
  queue --thread ID --message TEXT` once, on the next Send in the jelly or
  project the request names. Of the command line, the request gives only ID, which must be letters,
  digits and `-` and not start with `-`, so it never reads as a flag; the binary
  (`codex`, or `JELLY_CODEX` in the daemon's environment) and TEXT are jelly's
  (`src/server/wake.rs`).
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

`.github/workflows/check.yml` runs both on every pull request, with
`-D warnings` for clippy, and the relay's `npm ci && npm run check && npm test`.
`rust-toolchain.toml` names the one Rust that builds, tests and releases jelly,
here and in both workflows; move it forward in a pull request of its own.

The person uses the released jelly day to day (`npx jellyware setup`), with its
daemon on port 7878 and its skill in `~/.claude/skills/jelly`. While you work on
jelly itself, that skill and that `jelly` can be a release behind this
checkout: read `skill/SKILL.md` here, and run what you built, not `jelly`.

To keep a daemon of your build around, use a wrapper that gives it its own home
and port, so it never opens the person's jellies:

```sh
#!/bin/sh
export JELLY_HOME="$HOME/.jelly-dev" JELLY_URL="http://127.0.0.1:7879"
bin=/path/to/this/checkout/target/debug/jelly
# jelly starts its daemon by itself only on the default port.
if [ "$1" != serve ] && ! curl -s -o /dev/null "$JELLY_URL/api/version"; then
  nohup "$bin" serve --port 7879 >"$HOME/.jelly-dev.log" 2>&1 &
  for _ in 1 2 3 4 5 6 7 8 9 10; do curl -s -o /dev/null "$JELLY_URL/api/version" && break; sleep 0.3; done
fi
exec "$bin" "$@"
```

Saved as `jelly-dev` on the PATH, `jelly-dev look plan` uses the build in
`target/debug`. After `cargo build`, `jelly-dev doctor` says when the daemon
still runs the old build; stop it and the next command starts the new one.

For a one-off run or a test, use a throwaway daemon with an empty home:
`jelly serve --temp` picks a free port and a fresh home, prints `jelly on URL` first,
and removes the home when stopped. Pass that URL with `--url` or `JELLY_URL`.

## Release

Nobody tags or bumps a version by hand. Once a night,
`.github/workflows/release.yml` publishes what was merged to `development`
since the last release to npm and GitHub, one patch above the latest release,
and tags it `vX.Y.Z`. It runs `check.yml` first and publishes nothing when
that fails. A night where nothing that ships changed (`src`, `assets`,
`skill`, `npm`, `build.rs`, the Cargo files) releases nothing. A merged fix is
therefore not out until the next night; when it cannot wait, run the workflow
now: `gh workflow run release.yml --ref development`. Do not push a tag. The
version a binary reports comes from that tag, not from `Cargo.toml`: a build
from a checkout says `0.1.4` at the release commit and `0.1.4+3` three commits
later. For a new minor or major, set `version` in `Cargo.toml`; the next
release uses it. The relay is deployed separately: `relay/README.md`.
