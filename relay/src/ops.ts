// The changes a shared jelly takes, made to the jelly in memory: no storage,
// no sockets, no clock of its own. `Live` in index.ts saves and sends what
// this makes. The daemon's store makes the same changes to a jelly that is
// not shared (src/op.rs, src/store.rs); tests/ops.json says what each must
// do, and relay/test/ops.test.mjs runs it here.
import { Refusal } from "./verbs.ts";

export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };
export interface Reply { text: string; by: string; at_ms: number }
export interface Comment { id: string; anchor: Json; text: string; by: string; at_ms: number; resolved: boolean; replies: Reply[] }
/** An ask as the daemon built and checked it (`store::ask_item`), with the fields this file reads. */
export interface Ask { id: string; seen?: boolean; title: string; options?: { id: string }[]; group?: string | null; answer?: Json; unasked?: Json; [k: string]: Json | undefined }
export interface Doc { name: string; about: string; view: string; verbs: string; state: Json; version: number; view_rev: number; comments: Comment[]; asks: Ask[]; unsent?: number }
/** One change as it happened. `unsent` is the jelly's count toward Send after it. */
export interface Ev { seq: number; at_ms: number; kind: string; by: string; verb?: string; args?: Json; source?: Json; quiet?: true; version: number; view_rev: number; unsent: number }

/** A change, as the daemon sends it (`op::Op`) or a link's page asks for it. */
export type Op =
  | { op: "do"; verb: string; args?: Json; source?: Json; quiet?: boolean }
  | { op: "set"; pointer?: string; value?: Json; remove?: boolean }
  | { op: "shape"; view?: string | null; verbs?: string | null; about?: string | null }
  | { op: "ask"; item: Ask }
  | { op: "unask"; id: string }
  | { op: "answer"; id: string; option?: string | null; text?: string }
  | { op: "comment"; anchor?: Json; text?: string }
  | { op: "reply"; id: string; text?: string; resolve?: boolean }
  | { op: "resolve"; id: string }
  | { op: "send" }
  /** The PIN of the jelly's room locked after `tries` wrong tries. The relay's own. */
  | { op: "lock"; tries: number };

/** Runs one verb on a copy of the state and returns the new state, or throws a Refusal. */
export type RunVerb = (verbs: string, verb: string, state: Json, args: Json, by: string, at_ms: number) => Json;

/** Rust's `str::trim`: the characters with Unicode's White_Space property. JavaScript's
 *  `trim` also takes U+FEFF and leaves U+0085, so the two would store different text. */
const SPACE = new Set([0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004,
  0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000]);
/** A loop from each end, not a regex: `/\s+$/` backtracks from every space of a
 *  long inner run, and a link could send one to stall the jelly for everyone. */
export function trim(s: unknown): string {
  const t = String(s ?? "");
  let from = 0, to = t.length;
  while (from < to && SPACE.has(t.charCodeAt(from))) from++;
  while (to > from && SPACE.has(t.charCodeAt(to - 1))) to--;
  return t.slice(from, to);
}

/** `store::is_person`: "person", or "person:<name>" with a name. */
export const isPerson = (by: string) => by === "person" || (by.startsWith("person:") && by.length > "person:".length);

/** `store::counts_toward_send`: a person's verb call (unless quiet), comment,
 *  reply, resolve, answer or undo adds one to the count; `send` sets it to zero. */
export const countsTowardSend = (kind: string, by: string, quiet: boolean) =>
  isPerson(by) && ["do", "comment", "reply", "resolve", "answer", "undo"].includes(kind) && !quiet;

/** An array index as RFC 6901 writes it: no sign, no leading zero. */
const INDEX = /^(0|[1-9]\d*)$/;
const unescape = (k: string) => k.replaceAll("~1", "/").replaceAll("~0", "~");

/** `store::set_at`: puts `value` at a JSON pointer, or removes what is there. */
export function setAt(root: Json, pointer: string, value: Json | undefined): Json {
  if (pointer === "") return value === undefined ? {} : value;
  if (!pointer.startsWith("/")) throw new Refusal(`${JSON.stringify(pointer)} is not a JSON pointer; start it with /`);
  const cut = pointer.lastIndexOf("/");
  const parentPath = pointer.slice(0, cut);
  const key = unescape(pointer.slice(cut + 1));
  let parent: Json | undefined = root;
  for (const raw of parentPath.split("/").slice(1)) {
    const k = unescape(raw);
    parent = Array.isArray(parent) ? (INDEX.test(k) ? parent[Number(k)] : undefined)
      : parent && typeof parent === "object" && Object.hasOwn(parent, k) ? parent[k] : undefined;
    if (parent === undefined) throw new Refusal(`nothing at ${parentPath || "/"}`);
  }
  if (Array.isArray(parent)) {
    const i = key === "-" ? parent.length : INDEX.test(key) ? Number(key) : NaN;
    if (Number.isNaN(i)) throw new Refusal(`${JSON.stringify(key)} is not an array index`);
    if (value !== undefined && i <= parent.length) parent[i] = value;
    else if (value === undefined && i < parent.length) parent.splice(i, 1);
    else throw new Refusal(`${pointer} is past the end of the array`);
  } else if (parent && typeof parent === "object") {
    if (value !== undefined) parent[key] = value;
    else if (Object.hasOwn(parent, key)) delete parent[key];
    else throw new Refusal(`nothing at ${pointer}`);
  } else throw new Refusal(`${parentPath} is neither an object nor an array`);
  return root;
}

/** Makes `op` on `d`, by `by` at `now`, and returns its event without a seq.
 *  A Refusal can come after `d` changed: the caller keeps the jelly as it was. */
export function apply(d: Doc, op: Op, by: string, now: number, run: RunVerb): Omit<Ev, "seq"> {
  let args: Json | undefined, verb: string | undefined, source: Json | undefined;
  let bump = false;
  const ask = (id: string) => d.asks.find((a) => a.id === id) ?? (() => { throw new Refusal(`${d.name} has no ask ${id}`); })();
  const comment = (id: string) => d.comments.find((c) => c.id === id) ?? (() => { throw new Refusal(`${d.name} has no comment ${id}`); })();
  switch (op.op) {
    case "do":
      d.state = run(d.verbs, op.verb, d.state, op.args ?? null, by, now);
      verb = op.verb; args = op.args ?? null; source = op.source ?? undefined; bump = true;
      break;
    case "set":
      d.state = setAt(structuredClone(d.state), op.pointer ?? "", op.remove ? undefined : op.value);
      args = { pointer: op.pointer ?? "" }; bump = true;
      break;
    case "shape":
      if (op.view == null && op.verbs == null && op.about == null) throw new Refusal("shape needs a new view, new verbs or a new about");
      if (op.about != null) d.about = trim(op.about);
      if (op.view != null || op.verbs != null) {
        d.view = op.view ?? d.view; d.verbs = op.verbs ?? d.verbs;
        d.view_rev++; bump = true;
      }
      break;
    case "ask": {
      const item: Ask = { ...op.item, id: `a${d.asks.length + 1}` };
      d.asks.push(item);
      args = item as Json;
      break;
    }
    case "unask":
      ask(op.id).unasked = { by, at_ms: now };
      args = op.id;
      break;
    case "answer": {
      const item = ask(op.id);
      if (item.unasked) throw new Refusal(`${op.id} was unasked`);
      const option = op.option ?? null;
      if (option !== null && !item.options?.some((o) => o.id === option)) throw new Refusal(`${JSON.stringify(option)} is not an option of ${op.id}`);
      const t = trim(op.text);
      if (!item.seen && option === null && !t) throw new Refusal("this ask needs an option or some text");
      item.answer = { option, text: t, by, at_ms: now };
      args = { id: op.id, seen: !!item.seen, title: item.title, option, text: t, group: item.group ?? null };
      break;
    }
    case "comment": {
      const t = trim(op.text);
      if (!t) throw new Refusal("empty comment");
      const id = `c${d.comments.length + 1}`;
      d.comments.push({ id, anchor: op.anchor ?? null, text: t, by, at_ms: now, resolved: false, replies: [] });
      args = { id, anchor: op.anchor ?? null, text: t };
      break;
    }
    case "reply": {
      const t = trim(op.text);
      if (!t) throw new Refusal("empty reply");
      const c = comment(op.id);
      c.replies.push({ text: t, by, at_ms: now });
      c.resolved = op.resolve === true;
      args = { id: op.id, text: t, resolved: c.resolved, anchor: c.anchor, comment: c.text };
      break;
    }
    case "resolve":
      comment(op.id).resolved = true;
      args = op.id;
      break;
    case "send":
      args = d.unsent ?? 0;
      break;
    case "lock":
      args = { tries: op.tries };
      break;
    default:
      throw new Refusal(`no such change: ${(op as { op: unknown }).op}`);
  }
  if (bump) d.version++;
  const quiet = op.op === "do" && op.quiet === true;
  d.unsent = op.op === "send" ? 0 : (d.unsent ?? 0) + (countsTowardSend(op.op, by, quiet) ? 1 : 0);
  const ev: Omit<Ev, "seq"> = { at_ms: now, kind: op.op, by, version: d.version, view_rev: d.view_rev, unsent: d.unsent };
  if (verb !== undefined) ev.verb = verb;
  if (args !== undefined) ev.args = args;
  if (source !== undefined) ev.source = source;
  if (quiet) ev.quiet = true;
  return ev;
}
