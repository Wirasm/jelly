# jelly relay

Share links for jellies, on your own Cloudflare account. `jelly share plan --link`
publishes a read-only snapshot here and prints an unguessable link; anyone with
it can open the jelly on any device. Nobody else's relay is involved: each
person runs their own.

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
jelly share plan --link            # read-only link, works 30 days (--days 1..365)
jelly share plan --link            # again: same link, updated to the jelly as it is now
jelly unshare plan                 # the link stops working
```

## What it guarantees

- Links are 22 random characters (about 131 bits). The relay stores only a
  SHA-256 hash of each, so its storage never holds a working link.
- Only the holder of the publish secret (in `$JELLY_HOME/relay.json`) can
  publish or revoke.
- Shared pages run sandboxed in an opaque origin, send no referrer, and are
  marked `noindex`; `robots.txt` disallows everything.
- Links expire. After `unshare`, other Cloudflare locations can serve a cached
  copy for up to a minute.
- Anyone with a link can read everything in that jelly's state.

Live links (clicks and answers that reach your agent), per-person links and
PIN rooms are the next phases of #108.
