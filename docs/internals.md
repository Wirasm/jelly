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

## Output and closed pipes

When piping into `head` or `grep -q`, stdout BrokenPipe ends the command quietly.
Finite commands finish their changes before printing. `events` and `dev`
stop when an output write detects closure; while idle, they keep waiting. `wait`
keeps its event selection and timeout behavior, detecting closure only when it
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
conversation ID through `inbox --session-id` or `JELLY_SESSION_ID` (or use the
same explicit `--as` / `JELLY_AS` name for both).

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
An optional `"mine": "<name>"` keeps only events on jellies that name owns (made,
or last shaped, under it); `jelly hear --mine` sends it. A filtered read keeps
its own progress, apart from the same reader's unfiltered one, so it never moves
an unfiltered read past events.
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

## Event log

The daemon refuses startup if an existing event log cannot be read, contains
malformed events or is nonempty and missing its final newline. This prevents
reusing positions already passed by inbox readers or corrupting the final record
on the next append.

`~/.jelly/events.jsonl` is trimmed once it passes 8 MiB, to its newest 4 MiB plus up
to 1 MiB of your older events, so the inbox and `jelly look` still have them. A trim
keeps every event's position, and inbox reads go back only as far as the reader's
position. `jelly look` uses the last change and last eight person events per jelly,
kept in memory during startup validation and updated on append and trim; it does
not scan the log on each request. Initial one-hour inbox windows and timestamp
queries use an in-memory timestamp index to read only matching event byte ranges,
then return them in sequence order, even when the wall clock moves backwards.

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

After that, a new bundled jelly page reaches an existing home with
`jelly shape page --view assets/page.html`. This replaces the page's view and
keeps its state.
