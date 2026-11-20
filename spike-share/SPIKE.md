# Spike: a shared jelly in a Durable Object (#108 phase 0)

**Verdict: proven.** A jelly's own view and verbs run unchanged in one Cloudflare Durable Object, with several people live on it. Ran locally with `wrangler dev`; nothing deployed.

## What was run

The quiz recipe (`skill/recipes/quiz`), its view, verbs and state untouched:

| Check | Result |
|---|---|
| Verbs run in QuickJS (the daemon's engine, compiled to WebAssembly) | yes; refusals read exactly as in the daemon ("only the quizmaster runs the rounds", "you already answered") |
| Broken verbs refused at create | yes, same message as the daemon (`unexpected token in expression: ';'`) |
| Runaway verb (`while (true) {}`) | stopped at 1,050 ms, "verb ran longer than 1s and was stopped"; the next call works |
| 30 calls one after another | ~1 ms each in the object |
| 30 calls at once | all 30 kept, versions unique (after the fix below) |
| Two WebSocket viewers | both saw the same 66 updates in the same order |
| Two browsers, named players, the real quiz view | both see all four players live |

## Found on the way

- **Workers can't `eval`, so agent verbs need an engine.** QuickJS as WebAssembly works and matches the daemon exactly. The `.wasm` must be imported as a module (`import m from "./quickjs.wasm"`), because Workers cannot fetch it at runtime. The bundle is ~100 KB gzip-friendly JS plus the wasm.
- **An `await` between reading state and saving it loses updates.** A Durable Object lets other requests in at every `await`. With the engine load awaited mid-call, 30 parallel calls kept only 4 (37 rounds instead of 63). Loading the engine first and then reading, running and saving with no `await` makes each call atomic. That replaces the daemon's store lock.

## What phase 2 needs on top

- The sandboxed view: a separate origin or `CSP sandbox` and `postMessage`, so one jelly's script cannot read another's tokens.
- View and answer tokens, the PIN, expiry and `unshare`.
- The daemon's side: `jelly share --link` uploads, local commands forward, and events mirror back to the inbox.
- Attention, comments and assets in the object, not just state and verbs.
