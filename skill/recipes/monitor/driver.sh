#!/usr/bin/env bash
# The loop that does the work, pushes what it measures into the monitor jelly,
# and obeys the person between steps. Copy it and replace `step` and `setup`.
#
#   driver.sh [NAME] [TOTAL]      JELLY=path/to/jelly to use another binary
#
# This one measures a real thing: how long `jelly ask NAME … --at SELECTOR`
# takes now that the daemon checks the anchor, and whether the reply is right.
set -u
M=${1:-monitor}; TOTAL=${2:-40}; J=${JELLY:-jelly}
verb() { "$J" do "$M" "$@" >/dev/null; }
read_state() { "$J" state "$M" "$1" 2>/dev/null | tr -d '"'; }   # empty when nothing is there

# ---- obeying the person: call this before every step -----------------------
# Acknowledges everything they asked, waits while paused, returns 1 on stop.
obey() {
  local seq want acked
  while :; do
    seq=$(read_state /control/seq); acked=$(read_state /control/acked/seq); want=$(read_state /control/requested/want)
    if [ "${seq:-0}" -gt "${acked:-0}" ]; then
      verb ack "{\"seq\":$seq,\"note\":\"between steps, at $DONE of $TOTAL\"}"
    fi
    case "$want" in
      stop) return 1 ;;
      pause) sleep 1 ;;     # look again in a second
      *) return 0 ;;
    esac
  done
}

# ---- the work: replace these two --------------------------------------------
setup() {
  T=$(mktemp -d)
  cat > "$T/view.html" <<'V'
<!doctype html><h1 id="title">Launch plan</h1><ol id="list"><li>Write it</li><li>Ship it</li></ol>
V
  echo 'const verbs = {};' > "$T/verbs.js"
  "$J" make monitor-target --view "$T/view.html" --verbs "$T/verbs.js" --replace --about "What the monitor's example run asks about" >/dev/null
  DAEMON=$(pgrep -f "serve --port $(sed 's/.*://' <<<"${JELLY_URL:-:7878}")" | head -1)
  LAT=(); ALL=(); FAILS=0; WRONG=0
}
# Milliseconds since the epoch; `date +%s%N` is GNU only.
now_ms() { perl -MTime::HiRes=time -e 'printf "%d", time * 1000'; }
median() { printf '%s\n' "$@" | sort -n | sed -n "$(( ($# + 1) / 2 ))p"; }
# One step. Sets MS, RESULT (pass, slow or fail), LABEL, WANT and REPLY.
step() {
  local at start out id
  # The ask's `anchor` says whether the daemon found that element on the page.
  if (( $1 % 2 )); then at="#list"; WANT="matched"; else at="#nope"; WANT="missing"; fi
  start=$(now_ms)
  out=$("$J" ask monitor-target "Run $1: which first?" --option a=Write --option b=Ship --at "$at" --json 2>&1)
  MS=$(( $(now_ms) - start ))
  REPLY=$(jq -r '.anchor // "not checked: \(.anchor_unchecked)"' <<<"$out" 2>/dev/null) || REPLY=$out
  id=$(jq -r '.args.id // empty' <<<"$out" 2>/dev/null)
  [ -n "$id" ] && "$J" unask monitor-target "$id" >/dev/null 2>&1
  # The person's limit decides what counts as slow: moving it steers the run.
  LIMIT=$(read_state /metrics/0/threshold/value)
  if [ "$REPLY" != "$WANT" ]; then RESULT=fail
  elif [ -n "$LIMIT" ] && [ "$MS" -gt "${LIMIT%.*}" ]; then RESULT=slow
  else RESULT=pass; fi
  LABEL="run $1 · --at $at · $MS ms"
}

# ---- the loop ----------------------------------------------------------------
setup
DONE=0
verb log "{\"text\":\"started: $TOTAL asks, odd runs at #list (there), even runs at #nope (not there)\"}"
for i in $(seq 1 "$TOTAL"); do
  if ! obey; then
    verb finish "{\"status\":\"stopped\",\"headline\":\"Stopped at $DONE of $TOTAL\",\"text\":\"Median $( [ $DONE -gt 0 ] && median "${ALL[@]}" || echo 0 ) ms over $DONE asks, $FAILS wrong.\"}"
    exit 0
  fi
  [ "$i" -eq 1 ] && verb phase '{"id":"warm"}'
  [ "$i" -eq 6 ] && verb phase '{"id":"measure"}'
  step "$i"; DONE=$i
  MED=$( [ ${#LAT[@]} -gt 0 ] && median "${LAT[@]}" || echo "$MS" )
  rss=$(ps -o rss= -p "$DAEMON" 2>/dev/null | awk '{printf "%.1f", $1 / 1024}')
  browsers=$(pgrep -f -- "${BROWSER_MATCH:---headless=new}" | wc -l | tr -d ' ')
  verb push "{\"points\":[{\"m\":\"latency\",\"v\":$MS},{\"m\":\"browsers\",\"v\":${browsers:-0}},{\"m\":\"memory\",\"v\":${rss:-0}}],
             \"items\":[{\"r\":\"$RESULT\",\"label\":\"$LABEL\"}],\"done\":$DONE,\"total\":$TOTAL}"
  # Flag what the person should see, once each: the first wrong reply (the
  # rest go to the log), and a level shift (then measure against the new level).
  if [ "$RESULT" = fail ]; then
    FAILS=$(( FAILS + 1 ))
    if [ "$WRONG" -eq 0 ]; then
      verb flag "$(printf '{"title":"Run %s: the reply was wrong","text":"It answered in %s ms, about an anchor that %s. Later wrong replies are in the log.","metric":"latency","v":%s,"show":{"labels":["should say","said"],"before":"%s","after":"%s"}}' \
        "$i" "$MS" "$( (( i % 2 )) && echo exists || echo "does not exist")" "$MS" "$WANT" "${REPLY//\"/\\\"}")"
    fi
    WRONG=1
    verb log "{\"text\":\"run $i: wrong reply in $MS ms: ${REPLY//\"/\\\"}\",\"level\":\"bad\"}"
  fi
  if [ ${#LAT[@]} -ge 5 ] && { [ "$MS" -gt $(( MED * 3 )) ] || [ $(( MS * 3 )) -lt "$MED" ]; }; then
    verb flag "$(printf '{"title":"Run %s took %s ms","text":"The %s runs before it had a median of %s ms.","metric":"latency","v":%s,"show":{"labels":["the runs before","run %s"],"before":"%s ms","after":"%s ms\\n%s"}}' \
      "$i" "$MS" "${#LAT[@]}" "$MED" "$MS" "$i" "$MED" "$MS" "${REPLY//\"/\\\"}")"
    verb log "{\"text\":\"run $i: $MS ms, the median before it was $MED ms\",\"level\":\"warn\"}"
    LAT=()   # a new level: compare what follows against it
  elif (( i % 5 == 0 )); then
    verb log "{\"text\":\"$i of $TOTAL · median $MED ms · last $MS ms\"}"
  fi
  ALL+=("$MS")
  LAT+=("$MS")
done
verb phase '{"id":"wrap"}'
MED=$(median "${ALL[@]}"); MAX=$(printf '%s\n' "${ALL[@]}" | sort -n | tail -1); MIN=$(printf '%s\n' "${ALL[@]}" | sort -n | head -1)
verb log "{\"text\":\"finished: median $MED ms, fastest $MIN ms, slowest $MAX ms, $FAILS wrong\"}"
if [ "$FAILS" -gt 0 ]; then HEAD="$FAILS of $TOTAL replies were wrong"; else HEAD="Median $MED ms"; fi
verb finish "{\"status\":\"$( [ "$FAILS" -gt 0 ] && echo failed || echo done )\",\"headline\":\"$HEAD\",\"text\":\"$TOTAL asks with --at: median $MED ms, fastest $MIN ms, slowest $MAX ms. Before the anchor check an ask took about 4 ms.\"}"
