# jelly internals

Contracts between the CLI, the daemon and the runtime. Read this when you change
jelly itself; to use jelly, read [`skill/SKILL.md`](../skill/SKILL.md).

## Startup

The first client command starts a detached daemon if needed, waits for it, and
prints one startup line on stderr. Data lives in `$JELLY_HOME` or `~/.jelly`, just
as with `jelly serve`; on Windows `~/.jelly` is `%USERPROFILE%\.jelly`. No login
service or extra install step is needed.

`jelly serve` still runs in the foreground for manual use and startup diagnostics.
Unreadable or malformed persisted jellies prevent startup rather than serving a
partial store. Run `jelly serve` to see the affected file and error.
For an isolated run: `jelly serve --temp` starts a throwaway daemon on a free port
with a fresh home, prints `jelly on URL` first, and removes the home when stopped;
then use `jelly --url URL ls`. On-demand startup applies only to the default
local endpoint, including `http://localhost:7878` and equivalent URL spellings;
other `--url` / `JELLY_URL` targets must already be running.

## Updating

`src/update.rs` holds what the daemon, `jelly update`, `jelly doctor` and
`jelly hear --hook` share. The daemon asks `JELLY_UPDATE_URL` (default: npm's
`jellyware/latest`; `off` never asks) at start and once a day, and answers
`GET /api/version` with `{version, started_ms, home, latest, newer, install,
commands, updating, failed}`. `install` is `npm` when the daemon's binary sits under
`node_modules/@getjellyware/`, else `source`, and then `commands` are the two
to run in the checkout. `JELLY_NPM` names another npm binary.
`.cargo/config.toml` sets `JELLY_UPDATE_URL=off` for `cargo test` and `cargo
run`, so tests never ask npm; `tests/update.rs` points it at a local stub.

`POST /api/restart` replaces the daemon process with the binary now at its
path, with the same arguments (Unix only; a `--temp` daemon comes back with a
new home and port). `started_ms` changes with every start: `jelly update` waits
for that, and the jelly page reloads on it. `POST /api/update` is the page's
"Update now": the daemon runs the same install as `jelly update` and restarts,
or keeps the reason in `failed`. `jelly hear --hook` remembers per session, in
`hook-told.json` in the home, which jelly's skill the session read and when it
was last checked.

## Output and closed pipes

When piping into `head` or `grep -q`, stdout BrokenPipe ends the command quietly.
Finite commands finish their changes before printing. `events` and `dev`
stop when an output write detects closure; while idle, they keep waiting. `hear
--wait` keeps its event selection and timeout behavior, detecting closure only when it
prints a qualifying event. `serve` continues serving when stdout closes.

## Inbox

`jelly hear --hook` starts the default daemon silently, suppresses startup
errors, and prints nothing when there is nothing new. Outside a project it still
attempts startup, then exits successfully without inbox output. Once startup
succeeds, a daemon that does not answer the inbox request within a second, reader
identification, decoding and daemon read or progress-commit failures appear in
hook output as `jelly hear failed: ...`. Failed reads do not advance progress.

Claude's `CLAUDE_CODE_SESSION_ID` environment variable keeps one inbox reader
across commands and resumed conversations. JSON `session_id` identifies hook
invocations only. To share that progress with ordinary commands, supply the same
conversation ID through `hear --session-id` or `JELLY_SESSION_ID` (or use the
same explicit `--as` / `JELLY_AS` name for both).

Inbox identity uses explicit `--as` / `JELLY_AS` first, then
`hear --session-id` / `JELLY_SESSION_ID`, then `CLAUDE_CODE_SESSION_ID`, then
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
An optional `"mine": "<name>"` keeps only events on jellies that name owns (made,
or last shaped, under it); `jelly hear --mine` sends it. A filtered read keeps
its own progress, apart from the same reader's unfiltered one, so it never moves
an unfiltered read past events.
Omitting `project` or using an empty string reads all projects. The response is
`{ "events": [...], "through": 123 }`; events include `text`. The first read
returns every event from the oldest one stamped within the last hour (a stamp
after now does not count), and stores the position before it as the reader's, so
uncommitted events remain available on retry even after that hour passes. It
reads the whole log to find that event, because stamps are not in log order: a
change mirrored from a share keeps the relay's time, so a share that catches up
logs older stamps after newer ones. The scan holds the store lock, so on a full
log a verb sent during a reader's first read waits for it (tens of milliseconds).
After decoding and successfully writing and
flushing output, the CLI sends POST `/api/inbox/commit` with the same project and
reader plus the returned `through` position. Commits only move that reader's
progress forward and leave events newer than `through` unread. Lost or undecodable
read responses do not consume events. Ambiguous output, failed acknowledgements
or client crashes may replay events.

A reader's position only spares it a replay of the last hour, so the daemon
never refuses to start over one. An entry in `inbox-readers.json` that does not
parse, or a file that is not JSON, is dropped, and that reader is new on its next
read. A file that cannot be read at all (a directory, no permission) fails reads,
commits and startup. Each entry keeps `read_ms`, when its reader last started or
committed a read (an entry without it counts as read now). Whenever the file is
written, entries idle for 30 days are dropped, so it does not grow with every
conversation that ever ran `jelly hear`. A reader that is only behind keeps its
position: after a trim it hears every event still in the log after it.

The route returns every event. The CLI folds what it prints (`inbox_lines` in
`main.rs`): the calls of one verb by one person on one jelly become one line,
at the last call, with the count and that call's args (`Event::heard`).

## Event log

The daemon refuses startup if an existing event log cannot be read, contains
malformed events or is nonempty and missing its final newline. This prevents
reusing positions already passed by inbox readers or corrupting the final record
on the next append. A daemon that starts without the log numbers new events
after the highest position committed in `inbox-readers.json`, so readers still
hear them; it refuses startup if that file exists and cannot be read.

`~/.jelly/events.jsonl` is trimmed once it passes 8 MiB, to its newest 4 MiB plus up
to 1 MiB of your older events, so the inbox and `jelly look` still have them. A trim
keeps every event's position, and inbox reads go back only as far as the reader's
position. `jelly look` uses the last change and last eight person events per jelly,
kept in memory during startup validation and updated on append and trim; it does
not scan the log on each request. An inbox read starts at the end of the log and
stops at the reader's position, so it never parses the history before that; only a
new reader's first read parses the whole log.

## Send and waiting

A jelly counts what people did since the last `send` in `unsent` (comments,
answers, verb calls, undos; `store::record`). A verb call the view made with
`jelly.do(verb, args, { quiet: true })` is left out: its `do` event carries
`quiet: true`, which the daemon, the relay and the runtime all skip when they
count. Nothing else about a quiet call changes. The runtime shows the count on its
Send button, or a view draws its own (`data-send`, `jelly.person.send()`). `send`
sets the count to zero and is the only event that ends `GET /api/wait`, which is
all `jelly hear --wait` does, unless `every=true` (`--every`). The wait reads
`events.jsonl` after the `since` position, and the live bus only wakes it, so a
reader that falls behind still hears the Send. Without `since` (no `--since`)
the wait takes the daemon's newest event under the store lock, which every
event is numbered and logged under, and reads from the end of the log, so it
starts without reading history. `jelly` is a full name: the CLI resolves a
short one in its project before it asks. The relay keeps the
same count for a shared jelly.

Two other listeners wake an agent on Send, and read Sends the same way. The
daemon's `--wake` task (`server/wake.rs`) follows the log from its end, woken by
the live bus. The Pi extension (`assets/pi-extension.ts`) runs one
`jelly hear --wait --json` at a time, each `--since` the Send the last returned.

## Who is looking

Each open page tells the daemon, over its WebSocket, which jelly it shows and
whether its tab is in front (`{"open": name, "visible": bool}`), on connect and
on `visibilitychange`. Agent renders (`look`, `try`) do not. The daemon keeps
this in memory by socket and drops it when the socket closes. `look`, `ask` and
`show` report it; the jelly page counts for every jelly. The page's hover
preview of a row is a live frame of that jelly and reports itself too, but only
ever beside the page, which already counts. People on a share link are not counted.

The jelly page has a "Notify me" button while the browser has not been asked.
Once allowed, the page shows a browser notification for an agent's ask when its
tab is not in front; clicking it brings the tab forward on that jelly. The page
reports `notifies` with its presence, so `jelly ask` can tell the agent
"their browser showed a notification" instead of "tell the person". It works
only while a jelly page is open in some tab, and not on share links.

## The jelly stylesheet

`assets/jelly.css` holds jelly's tokens (`--jy-*`, each a `light-dark()` pair) and
a few parts (`jy-bar`, `jy-chip`, `jy-btn`, `jy-action`, `jy-tag`, `jy-dot`,
`jy-mark`). `server.rs` replaces the `"__JELLY_CSS__"` placeholder in
`runtime.js` with the file's text, and that composed runtime is what
`/runtime.js` and a share get, and does the same in `snapshot.js`, which an
export gets in the runtime's place. The runtime adds it to the page's head
before first paint. Tokens are set on `.jy` and `:host`: `<jelly-ui class="jy">`
always has them, a view has them with `<html class="jy">`, and a shadow root with
the text of `jelly.css` (`window.jelly.css`) in its own style element. The
runtime forces `jy-dark` or `jy-light` on its own parts from the view's
background, so they match the page they sit on.

## Shares on the jelly page

`GET /api/shares` lists every shared jelly with its people, and when it ends as
the relay last said (`expires_at`), from the daemon's own record (`people_from`
is `record`), which is what the jelly page asks for
every 20 seconds. With `?here=true`, which the page sends only while a share's
people are showing (its Who button), the daemon asks the relay for each share
(`GET /api/live/<share>/people`) who holds a link, when each last opened one
(`opened_ms`, set when a link's page connects), how many of their pages are
open now (`here`) and when the share ends. If the relay does not answer, the
people come from the record. Links are made on
the relay's `share_origin` when its answer has one (the relay's `SHARE_ORIGIN`),
else on the relay's own URL. The jelly page shows
this under "Shared now", with the time each share has left, and stops a link or
a whole share with the same routes `jelly unshare` uses.

## The jelly page's list

The page draws every jelly as one line under its project, from
`GET /api/jellies`, `GET /api/projects` (empty projects too) and `GET /api/asks`.
What waits on the person (an open ask, or a reply to their comment) is first,
the newest change on top, and `n` opens the next one at its ask. Each jelly in
the list carries `archives_ms`: when `Store::sweep` will archive it, by the one
rule in `Jelly::archives_ms`, or null when it will not (kept, archived, or an
ask or comment still open). The page marks the last two days before that.
It asks again 120 ms after the last of a burst of events, and at least once a
second while events keep coming, so a jelly that never pauses cannot hold it back.

Remove on the page takes the row away at once and sends the `DELETE` ten
seconds later, so the person can undo it; a second remove, or leaving the page,
sends it at once. `jelly rm` deletes at once. Projects are made and removed
through `/api/projects`, the routes `jelly project` uses. The page has no
rename: agents in a repo use the project named after it, so a rename would cut
them off from their jellies. `jelly project OLD --move NEW` does it on purpose.

At startup the daemon replaces the stored jelly page with the bundled one when
they differ, so a home made by an older binary gets the new page.

## Feeds

`jelly feed` writes `feeds.json` in the home of the daemon it talks to, which
`/api/version` names; the daemon reads it every second and never writes it,
and no HTTP route touches it. A feed belongs to a jelly by its full name, so
`jelly move` and `jelly project --move` rewrite the file to take the feed
along, dropping a feed left under the new name, and `jelly rm` removes it.
They read the file before they change the jelly, so a file they cannot read
stops them first. A move or removal on the jelly page cannot touch the file:
`jelly feed` lists such a feed as not running. A due feed's command runs
without a shell in the folder the feed was made in, for at most 60 seconds,
one run at a time per jelly. Its standard output, parsed as JSON, becomes the
arguments of the verb, called as `feed`. How the last run went is kept in memory
and shown by `jelly look`. An archived jelly's feed does not run, and `jelly
look` says it rests.

## Pull requests

`jelly pr` runs in the CLI, not the daemon: `src/pr.rs` builds the comment from
the jelly as `GET /api/jellies/{name}` returns it (asks, answers, comments),
the view-only link from `GET /api/shares`, and the outline from the `look`
route, and leaves the outline out, with a line on stderr, when the page does
not render. It posts through the GitHub CLI; `JELLY_GH` names another binary.
`gh pr view [PR] --json url,comments` finds the pull request and its comments,
`gh pr comment URL --body-file -` adds one, and `gh api graphql` with
`updateIssueComment` replaces one. `--dry-run` runs only the first.

The comment's first line is `<!-- jelly project:name -->`. A comment that has
it and was written by the GitHub user `gh` is logged in as is the one to
replace, so a jelly has one comment per pull request and per user.

What a person wrote (an answer's words, a comment, a reply, a name) goes into a
code span, where GitHub reads no markdown or HTML and an `@name` notifies
nobody. What the agent wrote (the about line, questions, option labels) is
plain text with `<` and `&` escaped. A person is named by the name they set or
by the part of their email before the `@`: the full address stays off GitHub.
A person's own share link is never posted, because it lets its holder act as
them. There is no screenshot: GitHub has no API for uploading an image to a
comment.

## Pull

`jelly pull` gets asset names and bytes from the selected daemon, regardless of
its home or the client's home. GET `/api/jellies/{name}/assets` returns a sorted
JSON array of filenames, `[]` for an existing jelly without assets, or a JSON 404
error for an unknown jelly. Directory enumeration errors return a JSON 500 error.
Bytes come from `/j/{name}/assets/{file}`. Pull reports
success only after every listed file is written; inventory or download failures
return an error and may leave a partial destination folder.

## Snapshots

Browser snapshots belong to the current served view. The daemon supplies ownership
evidence in view boot data and the bundled runtime echoes it when posting a
snapshot. Missing evidence is rejected; stale evidence returns 409 and the runtime
reloads. After upgrading the daemon, reload older open pages and update tokenless
snapshot callers before they can contribute snapshots again. This is an internal
daemon/runtime protocol, not authentication.

## Agent renders

`look`, `try` and an ask's anchor check run headless Chrome and drive it over
the DevTools protocol (`run_browser_at` and `drive` in `src/server/look.rs`).
Each render loads the page, waits for its `load` event, then gives it 2.5 s of
virtual time, as `--virtual-time-budget` did: the clock stops while requests are
pending, so data the page fetches arrives before its timers run out. The page's
runtime posts its snapshot in that time, and with `png` the daemon takes the
screenshot after it.

A page that keeps a connection open (an EventSource, a streaming fetch, an app
in an iframe that does either) stops the clock for good, and a frame that
streams forever never fires `load`. So a render waits at most 3 s of real time
for `load` and 3 s for the budget. If the budget is not spent by then, it runs
the 2.5 s in 500 ms steps at about real speed, so the page's own requests still
get answers between steps, and then takes what is on screen.

Chrome uses the machine's GPU, or SwiftShader (its software GPU) where there is
none, so WebGL views get a context either way. Do not force SwiftShader with
`--use-angle=swiftshader`: on a machine with a GPU it holds the clock back a
second or more per render, and under load long enough for renders to hit
their 20 s limit.

`look` at one width runs a light and a dark render side by side, and takes the
png in the light one; only `--full` needs a third render, at the height the
light one measured. `try --png` takes its png in the render that does the
steps. A render that fails (no browser, a crash, the 20 s limit) prints a
`failed` line that names what was not checked, such as dark mode at 1280px,
with `problems` in `--json` holding it too. It is not a `!` line, because agents
change the page for those. A light render whose screenshot fails keeps the
snapshot the page sent; only the png is missing. `--compare` reports nothing
as new or fixed unless all four of its renders sent a snapshot, and its
`new_problems` and `fixed_problems` are then null.

## Upgrading a home

jelly keeps no compatibility paths: a daemon reads only the current shape of a
home. A home from before the November 2026 rename (`attention`, `pinned`,
`jelly.call`, event kinds `call`, `new`, `reshape`) is brought up to date once,
with the daemon stopped and a copy of the home kept:

```bash
python3 scripts/migrate-home-2026-11.py ~/.jelly           # reports what it would change
python3 scripts/migrate-home-2026-11.py ~/.jelly --write
```

It renames the stored fields and event kinds, rewrites old runtime names in
views, gives the jelly page and ask pages the bundled view, and drops undo
history, which holds views written for the old runtime.
