# jelly relay

Live share links for jellies, on your own Cloudflare account. `jelly share plan
--to ana@example.com` puts the jelly on your relay and prints a link for Ana:
she opens it on any device and uses the jelly (clicks, answers, comments), and
you hear her in `jelly hear` as `person:ana@example.com`. Nobody else's relay
is involved: each person runs their own.

## Set up (once)

You need a Cloudflare account (the free plan is enough) and Node.

```bash
cd relay
npm install
npx wrangler login
npm run deploy                     # prints https://jelly-relay.<you>.workers.dev
jelly relay https://jelly-relay.<you>.workers.dev
# jelly relay prints one more command: run it to give the relay your publish secret
```

A custom domain is optional: add one to the `jelly-relay` Worker in the
Cloudflare dashboard (Workers → jelly-relay → Domains), then run `jelly relay`
again with that URL and set the new secret.

## Use

```bash
jelly share plan --to ana@example.com   # Ana's link; again prints the same one
jelly share plan --to bo@example.com    # Bo gets his own
jelly share plan --link                 # view only: anyone with it watches, nobody acts
jelly share quiz --pin                  # a PIN for a room on a call (below)
jelly unshare quiz --pin                # the PIN stops; people already in stay
jelly unshare plan --to bo@example.com  # Bo's links stop; his open pages close
jelly unshare plan                      # every link stops; the final state is at home
```

Shares last 30 days (`--days 1..365`). jelly sends no email: pass the link on.

## PIN rooms

Nobody can type a 44-character link off a shared screen. `jelly share quiz
--pin` prints a 6-digit PIN and `https://<your relay>/pin`. Each person opens
that page and types the PIN with their email. They land in the jelly on their
own link, exactly as if you had run `--to` with their email: you hear them as
`person:<email>`, and `jelly unshare quiz --to <email>` stops them. The same
email on another device is the same person. Their link keeps working after the
PIN ends, until the share does.

- A PIN lets people join for 3 hours (`--hours 1..24`). `--pin` again makes a
  new PIN and the old one stops.
- 10 wrong PINs, from anyone, lock every PIN live on the relay: a wrong guess
  is a guess at all of them. That bounds a guesser's chance at 10 in a million
  per PIN. You hear the lock in `jelly hear`; run `--pin` again for a new one.
- One IP gets 5 wrong tries per 10 minutes. That is fewer than 10, so one
  person or script cannot lock a room alone. Right PINs never count.
- Expired, ended and locked PINs all answer the same "does not work", and
  every guess passes the throttle before any lookup.
- The email is whatever the person types; the PIN proves only that they were
  told it. Anyone with the PIN can join under any email, someone else's too.

## What the relay counts

The relay counts its own use, by day (UTC), so you can see whether sharing is
used. No client is involved, and it keeps no IP, no email and nothing from a
jelly. `GET /api/counts` with the publish secret returns the last 30 days that
had anything, oldest first, and the sum of every day counted:

```bash
curl -fsS -H "Authorization: Bearer $(jq -r .secret ~/.jelly/relay.json)" "$(jq -r .url ~/.jelly/relay.json)/api/counts"
```

```json
{
  "days": [{ "day": "2026-11-23", "shares": 2, "returning_shares": 1, "opens": 4, "rooms": { "1": 1, "3": 1 } }],
  "total": { "shares": 2, "returning_shares": 1, "opens": 4, "rooms": { "1": 1, "3": 1 }, "sharers": 1 }
}
```

- `shares`: jellies shared that day. Another link to a jelly that is already
  shared is not a new share.
- `returning_shares`: the shares made with a publish secret that had shared
  before. The relay tells sharers apart by a hash of that secret, and a relay
  has one secret, so `sharers` is 1 until you run `jelly relay URL` again: the
  new secret counts as a new sharer.
- `opens`: links opened that day. A link opened again the same day counts once,
  so a page that reconnects does not add to it. A link preview that only
  fetches the page does not count: the page has to connect.
- `rooms`: how many shares had that many people connected at once, at their
  peak that day. `{"1": 1, "3": 1}` is one share one person looked at and one
  where three were in together; rooms of 3 or more is the sum from `"3"` up. A
  person with two pages open is one person. You are not counted, only the
  people on links, and each page on the view-only link is one person.

Besides the daily numbers the relay stores the hash of the secret, and for
each share its id with its peak for each day.

### See them in a jelly

The `monitor` recipe charts numbers that are pushed to it, and a feed pushes the
totals every hour (it needs `curl` and `jq`):

```bash
cat > counts.json <<'EOF'
{"title": "Relay counts", "about": "What my relay has seen.", "status": "ready", "headline": "rooms3", "control": {"seq": 0},
 "metrics": [{"id": "rooms3", "label": "Rooms of 3 or more", "better": "higher"}, {"id": "shares", "label": "Shares made", "better": "higher"},
             {"id": "returning", "label": "Shares by a returning sharer", "better": "higher"}, {"id": "opens", "label": "Links opened", "better": "higher"}]}
EOF
jelly make relay-counts --recipe monitor --state counts.json --about "What my relay has seen"
jelly feed relay-counts push --every 3600 -- sh -c '
  curl -fsS -H "Authorization: Bearer $(jq -r .secret ~/.jelly/relay.json)" "$(jq -r .url ~/.jelly/relay.json)/api/counts" |
  jq "{points: [.total | {m: \"shares\", v: .shares}, {m: \"returning\", v: .returning_shares}, {m: \"opens\", v: .opens},
       {m: \"rooms3\", v: ([.rooms | to_entries[] | select((.key | tonumber) >= 3) | .value] | add // 0)}]}"'
```

Each chart is a total over time, so a line that climbs is the trend. `rooms3`
counts a share once for every day three or more people were in it. The
recipe's Pause and Stop buttons do nothing here: no run is listening.

## How it works

- Each shared jelly is one Durable Object. It holds the view, verbs, state,
  comments and asks, runs verbs in QuickJS (the daemon's engine, as
  WebAssembly) with the same 1 s limit, and keeps an event log.
- While shared, the relay's copy is the live one. The daemon forwards local
  changes (`call`, `set`, `shape`, `ask`, `withdraw`, `reply`, `resolve`) to
  it, and mirrors every change back over a WebSocket, so `inbox`, `wait` and
  `look` work as before. It keeps working while your laptop sleeps; the daemon
  catches up when it reconnects.
- `undo`, `move`, `rm` and a new `ask --spec` round are refused while shared.

`npm test` runs the relay locally and checks what it counts; `npm run check`
typechecks it. Neither deploys anything.

## What it guarantees

- A link is 44 random characters: the share's id, then a secret of about 131
  bits. The relay stores only a SHA-256 hash of each secret, so its storage
  never holds a working link. A PIN is stored as an HMAC keyed with the relay's
  secret, since a plain hash of 6 digits is reversed in a moment.
- Only the holder of the publish secret (in `$JELLY_HOME/relay.json`) can
  publish, shape, mint or revoke. Links can call verbs, answer and comment,
  nothing else; view-only links can do none of that.
- The view runs in a sandboxed iframe with an opaque origin and reaches the
  jelly only through `postMessage`, so it cannot read the link, the socket or
  another jelly. Pages send no referrer, are marked `noindex` and are not cached;
  `robots.txt` disallows everything.
- A revoked link stops at once, and its open pages close.
- Anyone with a link can read everything in that jelly's state.

## Not on a share

`jelly.other()` (other jellies), the jelly page around it, the agent's
"working" banner, and the project's `design.css`. Assets are served by share
id, so someone who kept a revoked link's page source can still fetch them.
