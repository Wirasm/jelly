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
jelly takedown <link or share id>       # a reported share, by its link: gone from the relay (below)
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

## Shared pages on their own domain

A view is HTML written by whoever shared the jelly. The relay shows it in a
sandboxed iframe, so it cannot read the link, the socket or anything stored
for the relay's host. But the address bar shows the relay's host, and the view
can draw anything, a fake login form included: it may submit forms and open
windows. Browsers' phishing lists flag what a page shows, by host and often by
the whole registered domain. So one bad view on `app.example.com` can get
`example.com` flagged, with the site, the docs and everything else on it.
Assets (`/a/<share id>/assets/<file>`) are files from the same person, served
from the same host.

Today that person is you: a relay has one publish secret, so only your daemon
can put a view on it. Once other people's jellies are on your relay, serve the
pages from a domain that holds nothing else. Use a separate registered domain,
not a subdomain of the one you care about.

1. Add the domain to the same Cloudflare account as the relay (Add a domain,
   then point its nameservers at Cloudflare).
2. Add it to the Worker: Workers → jelly-relay → Settings → Domains & Routes →
   Add → Custom domain. Cloudflare makes the DNS record and the certificate.
   Keep the domain the daemon publishes to; the Worker now answers on both.
3. Tell the relay, in `relay/`. It is stored on the Worker like the publish
   secret, so no domain name is in this repo and a later deploy keeps it:

   ```bash
   echo https://pages.example | npx wrangler secret put SHARE_ORIGIN
   ```

From then on:

- `jelly share plan --to ana@example.com` and `--link` print
  `https://pages.example/j/<44 characters>`, and `--pin` prints
  `https://pages.example/pin`. Nothing changes in `jelly relay` or
  `relay.json`: the daemon still publishes to the first domain, and the relay
  tells it where links live (`share_origin` in its answers).
- `/j/...`, `/pin` and `/a/...` are served on `pages.example` only. On any
  other host of the relay they answer 308 to the same path there, so links
  sent before the change still open. A page that was open during the change
  needs a reload.
- `/api/...` works on either host, with the publish secret as before.

Without `SHARE_ORIGIN` nothing changes: pages are served on the relay's own
host. To go back, `npx wrangler secret delete SHARE_ORIGIN`.

## Reports and takedown

Give people a way to report a page, in `relay/`:

```bash
echo 'mailto:abuse@example.com?subject=Report%20share%20' | npx wrangler secret put REPORT_URL
```

Every link page then has a quiet "Report" link beside "Made with jelly", in
the relay's own strip under the view. The relay adds the share id to the end
of `REPORT_URL`, so end it where the id belongs: a mail subject as above, or
`https://example.com/report?share=`. The share id is the first 22 characters
after `/j/`. It names the share and opens nothing, so a report never carries a
working link. Without `REPORT_URL` there is no Report link. `/pin` has none:
it shows nothing a sharer wrote.

To take a share down, with the link or the share id from the report:

```bash
jelly takedown https://pages.example/j/<44 characters>
jelly takedown <share id>
```

It needs only `relay.json` (the relay's URL and publish secret), not the jelly
and not a running daemon, so it works for a share another machine made. It
calls `DELETE /api/live/<share id>`, the route `jelly unshare NAME` uses. The
relay deletes the share: every link and asset answers 404 at once, open pages
close with "This jelly is no longer shared", and its PIN stops. The daemon
that shared it hears that the share ended and keeps the jelly as a local one.
The share stays down: its id is random and nothing can publish to it again
without the publish secret.

What is not here: blocking the person who shared it from sharing again. A
relay has one publish secret, so there is nobody to block but you. That
belongs with per-person publishing, before a relay hosts strangers.

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
  "total": { "shares": 2, "returning_shares": 1, "opens": 4, "rooms": { "1": 1, "3": 1 }, "sharers": 1, "came_back": 0 }
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
- `came_back` (in `total` only): shares that had three or more people in on
  two different days. This is the number jelly steers by. A meeting that makes
  a fresh share each week does not count yet: the relay only knows a share, not
  the team behind it.

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
- Every page the relay serves (a link, `/pin`, a link that no longer works)
  says "Made with jelly", linking to getjellyware.com. On a link it is a strip
  under the view, in the relay's page: the view cannot remove or restyle it,
  and it covers nothing in the view. Pages on your own daemon do not have it.

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
- A revoked link stops at once, and its open pages close. A share that was
  unshared or taken down is deleted from the relay, and its id never works again.
- Anyone with a link can read everything in that jelly's state.

## Not on a share

`jelly.other()` (other jellies), the jelly page around it, the agent's
"working" banner, and the project's `design.css`. Assets are served by share
id, so someone who kept a revoked link's page source can still fetch them.
