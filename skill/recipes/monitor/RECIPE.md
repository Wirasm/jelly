# monitor

**Use when** you start something that runs for minutes or hours (a benchmark, an
eval, a migration, a crawl, a test loop) and the person should be able to glance
at it and pause, stop or steer it without typing to you. The first screen is a
picture: one number, how far along, which phase, a square per item, a chart per
metric.

```bash
cp -r <skill>/recipes/monitor /tmp/monitor     # then edit start.json and driver.sh there
jelly new monitor --from /tmp/monitor --state /tmp/monitor/start.json --about "Run: <what is running>"
jelly see monitor                              # fix every `!` and `error` line first
jelly focus monitor
/tmp/monitor/driver.sh monitor 40 &            # the run: it pushes, and obeys the person
```

`state.json` and `driver.sh` are a real run, kept as the example: 40 asks with
`--at`, timing each and checking the reply. It caught a real bug (the first 18
answered in 4 ms from a stale snapshot, 8 of them wrongly) and the chart shows
the step where the check started working. That bug is fixed since; the same run
now gives 0 wrong of 40 with a median of 721 ms. `start.json` is the same run
before it began: start from that one.

The view and verbs are a starting point. Change them when the run needs
something else: a table of the worst items, a map, a picture per anomaly.

## State

| Key | What goes in it |
| --- | --- |
| `title`, `about` | What is running, and one line on why. |
| `status` | `ready`, then `running`, `paused`, `stopping`, and at the end `done`, `failed` or `stopped`. The verbs set it. |
| `total`, `done` | How many steps there are and how many are finished. Leave `total` out when you cannot know. |
| `phases[]`, `phase` | `id` and `label` for each phase, in order, and the id of the current one. |
| `headline` | The id of the metric shown in large type. |
| `metrics[]` | `id`, `label`, `unit`, `better` (`lower` or `higher`), and optionally a starting `threshold` (`{"value", "by": "agent", "at_ms": 0, "seq": 0}`). `push` fills `points`, `n`, `sum`, `min`, `max`, `last`, `over`. |
| `items[]`, `tally` | One `{r, label}` per finished item, `r` being `pass`, `slow` or `fail`. The last 300 are kept; `tally` counts all. |
| `anomalies[]` | What you flagged: `id`, `title`, `text`, `metric`, `v`, `t`, `show`, and the person's `verdict`. |
| `log[]` | The last 40 lines: `t`, `text`, `level` (`warn`, `bad` or none). |
| `control` | `seq` counts the person's requests. `requested` is the latest pause, run or stop. `acked` is the last one the driver saw. |
| `result` | `headline` and `text`, set by `finish`. |

## Rules that make it worth their time

- **Push small and often.** One `push` per step with its points, its item and
  `done`. Each metric keeps 240 points and averages older ones in pairs, so a
  run of any length stays a small state. Statistics (`min`, `max`, mean) cover
  every point ever pushed.
- **Obey between steps, and say so.** The person's buttons do not touch your
  process: they write a request into `control`. Read it before every step and
  `ack` it. Until you do, the page says "the run has not seen that yet", which
  is the truth. A step that takes minutes should check inside the step too.
- **Let their limit mean something.** Read the threshold each step and use it:
  here it decides what counts as `slow`. A limit the run ignores is decoration.
- **Flag each thing once.** A level shift is one flag, then measure against the
  new level. Twenty flags for one cause is a wall of text again. Put the rest in
  the log.
- **Show the anomaly.** Give a flag a `show`: `{"labels", "before", "after"}` as
  terminal text, or two images with `"kind": "image"`. "Should say / said" beats
  a sentence about it.
- **Finish honestly.** `finish` with `failed` when the run found failures, and
  put the finding in `headline`, not "done". Stop pushing after it: `push` is
  refused.
- **Push every step, not every millisecond.** Undo keeps what each push changed,
  within twice the state, but the daemon still rewrites the jelly's file on every
  call.

## The driver

`driver.sh` is the whole loop. Its shape, with the work left out:

```bash
call() { jelly call "$M" "$@" >/dev/null; }
read_state() { jelly state "$M" "$1" 2>/dev/null | tr -d '"'; }   # empty when nothing is there

obey() {   # before every step: acknowledge, wait while paused, return 1 on stop
  while :; do
    seq=$(read_state /control/seq); acked=$(read_state /control/acked/seq); want=$(read_state /control/requested/want)
    [ "${seq:-0}" -gt "${acked:-0}" ] && call ack "{\"seq\":$seq,\"note\":\"between steps, at $DONE of $TOTAL\"}"
    case "$want" in stop) return 1 ;; pause) sleep 1 ;; *) return 0 ;; esac
  done
}

for i in $(seq 1 "$TOTAL"); do
  obey || { call finish '{"status":"stopped","headline":"Stopped at '"$DONE"' of '"$TOTAL"'"}'; exit 0; }
  step "$i"; DONE=$i                               # your work: sets MS, RESULT, LABEL
  limit=$(read_state /metrics/0/threshold/value)   # their limit steers the run
  call push "{\"points\":[{\"m\":\"latency\",\"v\":$MS}],\"items\":[{\"r\":\"$RESULT\",\"label\":\"$LABEL\"}],\"done\":$DONE,\"total\":$TOTAL}"
done
call finish '{"status":"done","headline":"Median '"$MED"' ms"}'
```

Other verbs the driver uses: `phase '{"id":"measure"}'`, `log '{"text":"…","level":"warn"}'`,
`flag '{"title":"…","text":"…","metric":"latency","v":724,"show":{…}}'`.

## Hearing them and answering

Everything the person does arrives in `jelly inbox` as `pause`, `resume`, `stop`,
`threshold` and `judge` calls, and the driver sees it in `control` without you.

- **Pause, resume, stop**: the driver handles them. If you are not running a
  driver (you push by hand each turn), read `jelly state monitor /control` at
  the start of every turn and `ack` what you see.
- **A moved limit** (`threshold`): the driver reads it on its next step. The line
  says "not seen by the run yet" until the next `ack`.
- **"Look into this"** on an anomaly (`judge`): that one is for you, not the
  driver. Investigate, then answer where they asked: comment on the card
  (`#a-<id>`), or put a question on it with
  `jelly ask monitor "…" --option … --recommend … --at "#a-<id>"`.
- **"Expected"**: stop flagging that kind.
- Questions about a chart go on the chart: `--at "#m-<metric id>"`.
