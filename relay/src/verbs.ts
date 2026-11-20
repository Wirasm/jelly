// Runs a jelly's verbs exactly as the daemon does (src/verbs.rs): the same
// QuickJS engine, the same wrapper script, a fresh runtime per call, 1 s and
// 64 MB limits. Workers cannot eval, so QuickJS runs as WebAssembly, and the
// .wasm is imported as a module because a Worker cannot fetch it at runtime.

import { newQuickJSWASMModuleFromVariant, newVariant, type QuickJSWASMModule } from "quickjs-emscripten-core";
import baseVariant from "@jitl/quickjs-wasmfile-release-sync";
// By path, not the package's "./wasm" export: wrangler picks the wasm loader by file name.
import wasmModule from "../node_modules/@jitl/quickjs-wasmfile-release-sync/dist/emscripten-module.wasm";

const variant = newVariant(baseVariant, { wasmModule });
let loading: Promise<QuickJSWASMModule> | undefined;
export const engine = () => (loading ??= newQuickJSWASMModuleFromVariant(variant));

/** A reason to refuse a change, shown to whoever asked for it. */
export class Refusal extends Error {}

function script(src: string, body: string): string {
  return `(function() {
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
  const deadline = Date.now() + 1000;
  rt.setInterruptHandler(() => Date.now() > deadline);
  const ctx = rt.newContext();
  try {
    const result = ctx.evalCode(code);
    if (result.error) {
      const e = ctx.dump(result.error);
      result.error.dispose();
      throw new Refusal(Date.now() > deadline
        ? "verb ran longer than 1s and was stopped"
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
