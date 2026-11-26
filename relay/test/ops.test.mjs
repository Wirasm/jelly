// tests/ops.json against the relay's changes (src/ops.ts), with verbs run by
// the relay's own verbs.ts on QuickJS. cargo test runs the same cases
// against the daemon's store (src/op.rs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { newQuickJSWASMModuleFromVariant } from "quickjs-emscripten-core";
import baseVariant from "@jitl/quickjs-wasmfile-release-sync";
import * as ops from "../src/ops.ts";
import { run, Refusal } from "../src/verbs.ts";

const fixtures = JSON.parse(readFileSync(new URL("../../tests/ops.json", import.meta.url), "utf8"));
const mod = await newQuickJSWASMModuleFromVariant(baseVariant);
const runVerb = (...a) => run(mod, ...a);

/** What both sides put in an event: absent verb, args and source are null, an absent quiet is false. */
const made = (ev) => ({ kind: ev.kind, by: ev.by, verb: ev.verb ?? null, args: ev.args ?? null, source: ev.source ?? null,
  quiet: ev.quiet ?? false, version: ev.version, view_rev: ev.view_rev });

/** Drops every at_ms from comments and asks: when a change happened is not part of what it does. */
const untimed = (v) => Array.isArray(v) ? v.map(untimed)
  : v && typeof v === "object" ? Object.fromEntries(Object.entries(v).filter(([k]) => k !== "at_ms").map(([k, x]) => [k, untimed(x)])) : v;

for (const c of fixtures.cases) {
  test(c.name, () => {
    let doc = { name: "t:j", about: "", view: "", verbs: "const verbs = {}", state: {}, version: 1, view_rev: 1, comments: [], asks: [], unsent: 0, ...c.jelly };
    c.steps.forEach((step, i) => {
      const at = `step ${i + 1}`;
      const { by, ...op } = step.change;
      // As Live does: the change is made on the jelly in memory, which goes back to the saved one when it is refused.
      const saved = structuredClone(doc);
      try {
        const ev = ops.apply(doc, op, by, 1, runVerb);
        assert.equal(step.refused, undefined, `${at}: made the change, but should refuse it`);
        assert.deepEqual(made(ev), made(step.event), `${at}: event`);
        assert.equal(ev.unsent, doc.unsent, `${at}: the event carries the count toward Send`);
      } catch (e) {
        if (!(e instanceof Refusal)) throw e;
        assert.equal(e.message, step.refused, `${at}: refused`);
        doc = saved;
      }
      for (const [k, want] of Object.entries(step.jelly ?? {})) {
        assert.deepEqual(k === "comments" || k === "asks" ? untimed(doc[k]) : doc[k], want, `${at}: jelly ${k}`);
      }
    });
  });
}
