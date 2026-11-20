# jelly relay

Live share links for jellies, on your own Cloudflare account. `jelly share plan
--to ana@example.com` puts the jelly on your relay and prints a link for Ana:
she opens it on any device and uses the jelly (clicks, answers, comments), and
you hear her in `jelly inbox` as `person:ana@example.com`. Nobody else's relay
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
jelly unshare plan --to bo@example.com  # Bo's link stops; his open page closes
jelly unshare plan                      # every link stops; the final state is at home
```

Shares last 30 days (`--days 1..365`). jelly sends no email: pass the link on.

## How it works

- Each shared jelly is one Durable Object. It holds the view, verbs, state,
  comments and asks, runs verbs in QuickJS (the daemon's engine, as
  WebAssembly) with the same 1 s limit, and keeps an event log.
- While shared, the relay's copy is the live one. The daemon forwards local
  changes (`call`, `set`, `reshape`, `ask`, `withdraw`, `reply`, `resolve`) to
  it, and mirrors every change back over a WebSocket, so `inbox`, `wait` and
  `look` work as before. It keeps working while your laptop sleeps; the daemon
  catches up when it reconnects.
- `undo`, `move`, `rm` and a new `ask --spec` round are refused while shared.

## What it guarantees

- A link is 44 random characters: the share's id, then a secret of about 131
  bits. The relay stores only a SHA-256 hash of each secret, so its storage
  never holds a working link.
- Only the holder of the publish secret (in `$JELLY_HOME/relay.json`) can
  publish, reshape, mint or revoke. Links can call verbs, answer and comment,
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
