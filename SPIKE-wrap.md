# Spike: jelly on top of a running app

**Verdict: proven.** `jelly wrap NAME http://localhost:5173` puts jelly on the real Sasha dev server (SvelteKit on Vite). Agents ask questions pinned to real elements, `jelly see` reads the real page, the person's answers come back through the inbox, and an agent-written overlay previews copy variants live without touching the app's code. This branch is a spike, not a PR.

## What was run

Against the Sasha dev server already running from `sasha/.worktrees/af-qa` (`vite dev` on 5173), with a throwaway daemon:

```bash
jelly serve --port 7889 --home /tmp/h-wrap
jelly --url http://127.0.0.1:7889 --project sasha wrap app http://localhost:5173 --port 7901
jelly --url http://127.0.0.1:7889 see sasha:app --png
jelly --url http://127.0.0.1:7889 ask sasha:app "Which headline should the hero lead with?" \
  --option keep=… --option outcome=… --option time=… --recommend outcome --why … --at "#hero-title"
```

| Check | Result |
|---|---|
| Sasha renders through the proxy | yes, screenshot identical to direct |
| `jelly see` outline of the real page | yes, after fixing `display: contents` (below) |
| Question card pinned below `#hero-title` | yes |
| Answer clicked on the real page reaches `jelly inbox` | yes ("answered a2 → outcome") |
| Svelte hydration | clean (0 mismatches), after the two fixes below |
| Vite HMR WebSocket through the proxy | yes, "[vite] connected." |
| jelly's own WebSocket under `/__jelly` | yes |
| Overlay variants (the jelly's view, run on the app) | yes, hero swapped to the picked headline, panel flips live |

Nothing in Sasha was clicked except jelly's own card and panel, so its dev backend saw only page loads.

## How it works

- `POST /api/wrap` opens a second port. `/__jelly/*` on it is jelly's normal router; everything else is proxied to the app (reqwest, uncompressed so HTML can be edited).
- HTML responses get the jelly boot data plus a loader before `</body>`. The app's CSP header is dropped, and `Location` headers pointing at the app are rewritten.
- WebSocket upgrades are piped to the app with the same subprotocol (`vite-hmr`), via tokio-tungstenite.
- The runtime takes a `base` (`/__jelly`) for every route it calls.
- The wrapped jelly's own view is injected after the runtime as an overlay, so `jelly reshape app --view overlay.html` changes what the agent lays over the real app.
- `jelly see` renders `http://localhost:PORT/?agent=1` for a wrapped jelly. `localhost`, not `127.0.0.1`, so cookies for `localhost:5173` apply.

## Found on the way

1. **`see` skipped SvelteKit apps entirely.** SvelteKit mounts into `<div style="display: contents">`, which has no box, so the outline walker treated the whole app as hidden. It now walks through `display: contents`. This is a real `see` bug for any jelly view that uses it; worth its own PR.
2. **Injecting at the top of `<head>` broke Svelte hydration**, which re-rendered the page client-side. Injecting before `</body>` is fine.
3. **Placing cards before hydration also broke it.** The runtime put the question card inside Svelte's DOM before Svelte hydrated. The spike loads the runtime 300 ms after `load`; the runtime then sets up immediately instead of waiting for `DOMContentLoaded` and posts its first snapshot at once.
4. **CLI argument clash.** A subcommand positional named `url` silently replaced the global `--url`, so the CLI posted to the app instead of the daemon. Named `app` now.

## What a real version needs

- **Lifecycle.** Wraps are in memory only, gone on daemon restart. Persist them, add `jelly unwrap`, and show a friendly page when the app is down.
- **Hydration.** Wait for the framework properly instead of a 300 ms timer. One option is the first idle period after `load`, plus re-placement through the existing MutationObserver.
- **Streaming.** Request bodies are buffered (64 MB cap); responses other than HTML already stream.
- **Edges not tested:** OAuth redirects, absolute links to `localhost:5173` inside pages, service workers, and non-Vite dev servers (Next, webpack). `archon serve` not tried yet.
- **`jelly try` on a wrapped app has side effects.** Clicks reach the real dev backend; say so in the output.
- **Security.** The app's scripts share an origin with `/__jelly` and can call jelly's API. That is acceptable for your own app on your own machine, but tie it into the #66 work, and never wrap an untrusted site.
- **Phone width.** `see --width 390` uses `/see-frame/` and does not know about wraps yet.
- **Overlay API.** The spike runs the jelly's view as raw HTML. A small helper (`jelly.overlay.text(selector, value)`, highlights) would make variants one-liners, and turning a picked variant into a real code change is the agent's next step.

Rough size for a real version: about a week, most of it the lifecycle and the edges above.

# Spike 2: `jelly shot`, the whole app for an agent in one call

**Verdict: proven, with two things to fix before it is real (speed and noise).**

```bash
jelly wrap site http://localhost:5173
jelly shot site                      # / and the pages it links to, at 1280 and 390
jelly shot site --route /demo --widths 1280 --text
```

One command returned all 7 Sasha pages (`/`, `/blog`, `/sv`, `/login`, `/demo`, `/about-this-interview`, `/privacy`) at desktop and phone width, full height, as text. The full output is in `spike-wrap/shot-sasha.txt` (39 KB brief; 78 KB with `--text`). Per page it gives:

- **Every heading and control with a selector to paste into `try` or `--at`:** `header a[href="/login"]`, `#email`, `form button[type=submit]`. The demo form comes out as `textbox "Full name" = "" #name`, `email "Work email" = "" #email`, and so on, so an agent can fill it with `jelly try` without exploring.
- **Shared parts printed once** ("on every page except /sv, /demo"), since a translated page is a variant, not a reason to repeat the header.
- **Problems the browser measured:** page errors, failed requests (status 400 and up), content wider than the screen, controls off the screen or covered by another element, tap targets under 24px at phone width, fields without labels, images without `alt`, and text clipped by `overflow`. Problems most pages share are listed once.

## What it found on Sasha

- No page errors, no failed requests, nothing wider than the screen at 390px, and nothing off-screen or covered.
- **Tap targets at phone width:** the header "Sign in" link is 45×21, and footer links (Privacy, Terms, DPA, Svenska, Say hello) are 20px tall. WCAG 2.5.8 asks for 24px unless spacing compensates, so these are worth a look, not certain bugs.

## False positives found and removed on the way

All three came from apps hiding things on purpose; real apps will produce more:

- Screen-reader-only text (`sr-only`, a 1×1px box) looked like clipped text.
- The anti-spam honeypot field (`tabindex=-1`, inside `aria-hidden`) looked like a covered control.
- 18px checkboxes with labels looked too small, although the label is the tap target.

## Before it is real

- **Speed:** 61 s for 14 renders, one after another. Each render is a fresh Chrome (about 1.8 s), and a jelly holds one snapshot at a time, so renders of one wrapped app cannot overlap yet. Keying snapshots by route and width would allow parallel renders, giving roughly 10 s.
- **Noise:** the tap-size check should allow WCAG's spacing exception. Repeated items should be deduped by label and kind, not by selector, which changes per page when uniqueness changes.
- **`--json`**, so an agent can filter problems by kind without parsing text.
- **Routes behind a login:** need the named-session idea from `try`.
- **Plain jellies:** `shot` works on wrapped apps; the same flags should work on any jelly (one route, two widths) for the same problem checks.
