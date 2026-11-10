# map

**Use when** the person needs to see how a system fits together, or you are about
to change something and want them to see where it sits. It answers "what is
there, what talks to what, and what happens when…?" The person walks a path
through the picture instead of reading a description of it.

```bash
cp -r <skill>/recipes/map /tmp/map            # then write your own state.json there
jelly new map --from /tmp/map --about "How <the system> works"
jelly see map                                 # fix every `!` and `error` line first
jelly focus map
```

The example `state.json` and `assets/` are a real map: jelly itself, with two
traces and the change from commit ef60c62 laid over it. Replace both with your own.

The view and verbs are a starting point. Change them when the system needs
something else: columns instead of rows, a timeline, numbers on the wires.

## State

| Key | What goes in it |
| --- | --- |
| `title`, `about` | The system in a few words, and one line on its shape. |
| `layers[]` | `id`, `label`. Top to bottom, in the order the person should read them: who uses it first, what is on disk last. |
| `nodes[]` | `id`, `layer`, `label` (two or three words: it is a box on the map), `kind` (`person`, `browser`, `code`, `store`, `file`, `process`), `what` (one or two lines), `files[]` (path and the function names to read), `risk` (what to be careful of; leave out when nothing). The order within a layer is the order on the row. |
| `edges[]` | `from`, `to`, `label` (what flows: a call, a route, a file write). Point it the way the thing travels. |
| `traces[]` | `id`, `title` ("A person clicks a button"), `steps[]`. Each step: `at` (a node id), `say` (one line), `cite` (file and function), `show`. |
| `traces[].steps[].show` | A picture for the step: `{"kind", "before", "after", "caption", "labels"}`. `terminal` (the default) takes command output, `image` takes assets (`assets/shot.png`), `html` runs a page live. Leave `before` out when there is only one thing to show. A list shows several. |
| `change` | `title`, `note`: the change laid over the map. Set it with the `touch` verb, which also marks the nodes. |
| `flags[]` | Written by the person's `flag` verb. Start with `[]`. |

## Rules that make it worth their time

- **Read the code first, then draw.** Every box and wire is a claim. Put the file
  and function in `files` and `cite` so the person, or the next agent, can check
  it. A map with one invented wire is worse than no map.
- **Fewer boxes.** Twenty is a lot. Draw what the person needs for this question,
  not every module. One box per thing that has a name people already use.
- **Layers are a story.** Order them so most wires run downward and neighbours sit
  near each other; reorder the nodes in a layer until the long wires are few.
  Look at a screenshot before you show it (`jelly see map --png`).
- **A trace is the reason to open this.** Give at least one: a real path a real
  action takes, eight to fifteen steps, one short line each. Step through it
  yourself in the code while you write it. Where two steps have no wire between
  them the token hops; add the wire if the hop is a real call.
- **Show what happened at the steps that matter.** Run the command and paste its
  output as the step's `show`, or screenshot the result. Most steps need none.
- **Lay the change over it.** When you are about to change something, or just did,
  call `touch` with the nodes it reaches and one line each. The person sees what
  is touched, what sits next to it, and what is left alone.
- **Say what is risky.** `risk` is where you put the invariant that breaks things
  when ignored. It shows as a warning on the node.
- **Ask on the node.** `jelly ask map "…" --option … --recommend … --at "#n-<node id>"`
  puts the question under that node's row, and a `?` on the node.

## Hearing them and answering

The person taps boxes, walks traces and reacts. It arrives in `jelly inbox` as
`flag` (`wrong`, `more` or `careful`, on a node, with a note), `unflag`, and
`understood` (they reached the end of a trace and it made sense).

```bash
# "This is wrong": check the code. If they are right, fix the node as you answer.
jelly call map answer '{"flag":"f1","note":"You are right: browsers use the WebSocket.","fix":{"what":"…"}}'
# "Tell me more" or "Careful here": answer in a line or two, or add a trace that shows it.
jelly call map answer '{"flag":"f2","note":"Each call gets a fresh engine, so nothing leaks between verbs."}'
jelly call map add_trace @trace.json     # {"id","title","steps":[{"at","say","cite","show"}]}
# Lay a planned change over the map, or take it off.
jelly call map touch '{"title":"Per-agent inbox cursors","note":"…","nodes":[{"id":"cli","note":"…"}]}'
jelly call map untouch
```

New boxes, wires and layers go in with `jelly set map --at /nodes/- '{…}'`. A
flag stays open until you `answer` it; the person can take it back until then.
