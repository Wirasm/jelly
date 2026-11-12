#!/bin/sh
# Run in a disposable container with its own network and PID namespaces.
set -eu
[ -f /.dockerenv ] || { echo 'requires a disposable Docker container' >&2; exit 1; }
bin=$(realpath "$1")
root=$(mktemp -d)
trap 'pkill -x jelly || true; rm -rf "$root"' EXIT
unset JELLY_URL JELLY_PROJECT JELLY_AS
cd "$root"
client() {
    env HOME="$root" JELLY_HOME="$root/data" "$bin" "$@" >out 2>err
}
stop() {
    pkill -x jelly
    # The listener must close before the next cold start.
    while curl -sf http://127.0.0.1:7878/api/jellies >/dev/null; do sleep 0.05; done
}
for endpoint in default http://localhost:7878/ HTTP://127.1:7878 http://localhost.:7878 http://2130706433:7878 'http://[::ffff:127.0.0.1]:7878'; do
    if [ "$endpoint" = default ]; then
        client --project test ls --json
    elif [ "$endpoint" = http://localhost:7878/ ]; then
        client --url "$endpoint" --project test ls --json
    else
        JELLY_URL="$endpoint" client --project test ls --json
    fi
    [ "$(cat out)" = '[]' ]
    [ "$(wc -l <err)" -eq 1 ]
    grep -q '^jelly: daemon started at ' err
    client --project test ls --json
    [ "$(cat out)" = '[]' ]
    [ ! -s err ]
    stop
done
env HOME="$root" "$bin" --project test ls --json >out 2>err
[ "$(cat out)" = '[]' ]
grep -q '^jelly: daemon started at ' err
[ -f "$root/.jelly/jellies/surface.json" ]
stop
for project in '' test; do
    client --project "$project" inbox --hook
    [ ! -s out ] && [ ! -s err ]
    curl -sf http://127.0.0.1:7878/api/jellies >/dev/null
    stop
done
mkdir -p "$root/data/jellies"
printf '{' > "$root/data/jellies/broken.json"
if client ls --json; then echo 'partial store served' >&2; exit 1; fi
[ ! -s out ]
grep -q 'run `jelly serve` for details' err
! grep -q 'daemon started' err
client --project test inbox --hook
[ ! -s out ] && [ ! -s err ]
if client serve; then echo 'manual serve accepted broken data' >&2; exit 1; fi
grep -q 'broken.json' err
[ "$(cat "$root/data/jellies/broken.json")" = '{' ]
echo 'production cold-client checks passed'
