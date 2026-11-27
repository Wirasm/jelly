// Runs a jelly's verbs as the daemon does (src/verbs.rs): the same QuickJS
// engine (engine.ts), the same wrapper script, a fresh runtime per call, a
// 64 MB limit.
//
// The daemon stops a verb after 1 s. A Worker's clock stands still while code
// runs (Date.now() is the time of the last I/O), so the relay counts steps
// instead: it counts calls to QuickJS's interrupt handler, which runs as
// JavaScript calls functions and turns loops. Both say the same when they stop one.

import type { QuickJSWASMModule } from "quickjs-emscripten-core";

/** Interrupt handler calls a verb gets: roughly 25 million turns of an empty
 *  loop. How long that takes depends on the work in each turn and the machine,
 *  so this bounds work, not time. Work inside a builtin (JSON.stringify,
 *  JSON.parse) is not counted, so a loop doing heavy builtin work each turn
 *  can still run until the platform's CPU limit resets the object. */
const STEPS = 5_000;

/** A reason to refuse a change, shown to whoever asked for it. */
export class Refusal extends Error {}

function script(src: string, body: string): string {
  return `(function() { 'use strict';
  var __v;
  try { __v = (function() {
${src}
;return typeof verbs === 'undefined' ? undefined : verbs; })(); }
  catch (e) { return JSON.stringify({error: 'verbs did not load: ' + e}); }
  if (!__v || typeof __v !== 'object') return JSON.stringify({error: 'verbs must declare \`const verbs = { ... }\`'});
  ${body}
})()`;
}

function evaluate(mod: QuickJSWASMModule, code: string): unknown {
  const rt = mod.newRuntime();
  rt.setMemoryLimit(64 << 20);
  let steps = 0;
  rt.setInterruptHandler(() => ++steps > STEPS);
  const ctx = rt.newContext();
  try {
    const result = ctx.evalCode(code);
    if (result.error) {
      const e = ctx.dump(result.error);
      result.error.dispose();
      throw new Refusal(steps > STEPS
        ? "verb ran too long and was stopped: does it loop forever?"
        : `verbs did not load: ${e?.name ?? "Error"}: ${e?.message ?? "unknown error"}`);
    }
    const out = JSON.parse(ctx.getString(result.value));
    result.value.dispose();
    if (typeof out.error === "string") throw new Refusal(out.error);
    return "ok" in out ? out.ok : null;
  } finally {
    ctx.dispose();
    rt.dispose();
  }
}

/** Runs one verb and returns the new state. Synchronous on purpose: see `Live`. */
export function run(mod: QuickJSWASMModule, src: string, verb: string, state: unknown, args: unknown, by: string, at_ms: number): unknown {
  const v = JSON.stringify(verb);
  return evaluate(mod, script(src, `var f = Object.prototype.hasOwnProperty.call(__v, ${v}) ? __v[${v}] : undefined;
  if (typeof f !== 'function') return JSON.stringify({error: 'no verb ' + ${v} + '; this jelly has: ' + Object.keys(__v).join(', ')});
  var state = ${JSON.stringify(state)}, args = ${JSON.stringify(args ?? null)}, ctx = ${JSON.stringify({ by, at_ms })};
  try { var r = f(state, args, ctx); return JSON.stringify({ok: r === undefined ? state : r}); }
  catch (e) { return JSON.stringify({error: e instanceof Error && e.name === 'Error' ? e.message : String(e)}); }`));
}
