#!/usr/bin/env python3
"""One-off: bring a jelly home from before the November 2026 rename up to date.

    python3 scripts/migrate-home-2026-11.py HOME [--write]

Without --write it only reports what it would change. Stop the daemon first and
keep a copy of HOME: an older binary cannot read a migrated home.

What changes:
- jellies/*.json: `attention` is `asks` (a `look` is an ask with `seen`,
  `withdrawn` is `unasked`), `pinned` is `kept`, and undo history is dropped,
  because it holds views written for the old runtime.
- Views: `jelly.call` is `jelly.do`, and the names from before `jelly.person`
  and `jelly.other` (`jelly.answer`, `jelly.items`, `jelly.onAttention`,
  `jelly.comments`, `jelly.read`, `jelly.watch`, ...) become today's.
- The jelly page and ask pages get the bundled view and verbs; they keep their state.
- events.jsonl: event kinds get their new names.

Delete this script once every home you have is migrated.
"""
import glob
import json
import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KINDS = {"call": "do", "new": "make", "reshape": "shape", "pin": "keep",
         "attention": "ask", "withdraw": "unask", "project": "move"}

# Order matters: longer names first, and nothing here matches its own output.
VIEW = [
    (r"\bjelly\.call\b", "jelly.do"),
    (r"(\bjelly\.other\([^()]*\))\.watch\(", r"\1.onState("),
    (r"\bjelly\.onAttention\(", "jelly.person.onAsk("),
    (r"\bjelly\.onAttend\(", "jelly.person.onOpen("),
    (r"\bjelly\.itemsAt\(([^()]*)\)", r"jelly.person.asks.filter((a) => a.anchor?.selector === (\1) && !a.unasked).reverse()"),
    (r"\bjelly\.items\b", "jelly.person.asks"),
    (r"\bjelly\.attention\b", "jelly.person.asks.filter((a) => !a.answer && !a.unasked)"),
    (r"\bjelly\.answer\(", "jelly.person.answer("),
    (r"\bjelly\.(?:comments|feedback)\b", "jelly.person.comments"),
    (r"\bjelly\.comment\(", "jelly.person.comment("),
    (r"\bjelly\.reply\(", "jelly.person.reply("),
    (r"\bjelly\.resolve\(", "jelly.person.resolve("),
    (r"\bjelly\.viewer\b", "jelly.person.name"),
    (r"\bjelly\.by\b", "jelly.person.by"),
    (r"\bjelly\.read\(([^()]*)\)", r"jelly.other(\1).state"),
    (r"\bjelly\.watch\(\s*([^,()]+),\s*", r"jelly.other(\1).onState("),
    (r"\bjelly\.(?:focus|show)\(([^()]*)\)", r"jelly.other(\1).open()"),
    (r"\.withdrawn\b", ".unasked"),
]


def bundled(name):
    with open(os.path.join(REPO, "assets", name), encoding="utf-8") as f:
        return f.read()


def migrate_view(view):
    for pattern, to in VIEW:
        view = re.sub(pattern, to, view)
    return view


def migrate_ask(a):
    a = dict(a)
    if a.pop("kind", None) == "look":
        a["seen"] = True
    if "detail" in a:
        a["context"] = a.pop("detail")
    if "withdrawn" in a:
        a["unasked"] = a.pop("withdrawn")
    answer = a.get("answer")
    if isinstance(answer, dict) and "withdrawn" in answer:
        a["unasked"] = {"by": answer.get("by", "agent"), "at_ms": answer.get("at_ms", 0)}
        a["answer"] = None
    for o in a.get("options") or []:
        if "title" in o:
            o["label"] = o.pop("title")
        if "summary" in o:
            o["does"] = o.pop("summary")
    return a


def migrate_jelly(entry, notes):
    j = entry["jelly"]
    name = j["name"]
    if "asks" in j and "kept" in j:
        return None  # already migrated
    j["asks"] = [migrate_ask(a) for a in j.pop("attention", [])]
    if "feedback" in j:
        j["comments"] = j.pop("feedback")
    j["kept"] = j.pop("pinned", False)
    if name == "page":
        j["view"], j["verbs"] = bundled("page.html"), bundled("page.verbs.js")
        j["state"]["shown"] = j["state"].pop("focus", None)
    elif 'id="more-send"' in j["view"] and 'id="qs"' in j["view"]:
        j["view"], j["verbs"] = bundled("ask.html"), bundled("ask.verbs.js")
    else:
        view = migrate_view(j["view"])
        if 'name="jelly-attention"' in view:
            notes.append(f"{name}: draws its own asks with the old inline switch; mark each with data-ask or they show twice")
        if view != j["view"]:
            j["view"] = view
            j["view_rev"] += 1
    return {"jelly": j, "steps": [], "history_from": j["version"]}


def main():
    args = [a for a in sys.argv[1:] if a != "--write"]
    write = "--write" in sys.argv
    if len(args) != 1 or not os.path.isdir(os.path.join(args[0], "jellies")):
        sys.exit(__doc__)
    home = args[0]
    if os.path.exists(os.path.join(home, "daemon.lock")):
        try:
            import fcntl
            with open(os.path.join(home, "daemon.lock")) as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            sys.exit("a daemon is running on this home; stop it first")

    notes, changed, views = [], 0, 0
    for path in sorted(glob.glob(os.path.join(home, "jellies", "*.json"))):
        with open(path, encoding="utf-8") as f:
            entry = json.load(f)
        before = entry["jelly"]["view"]
        migrated = migrate_jelly(entry, notes)
        if migrated is None:
            continue
        changed += 1
        views += migrated["jelly"]["view"] != before
        if write:
            with open(path + ".tmp", "w", encoding="utf-8") as f:
                json.dump(migrated, f, ensure_ascii=False)
            os.replace(path + ".tmp", path)

    events = os.path.join(home, "events.jsonl")
    renamed = 0
    if os.path.exists(events):
        with open(events, encoding="utf-8") as f:
            lines = f.read().split("\n")
        # An event's own kind is the first one on its line: `kind` comes before `args`.
        kind = re.compile(r'"kind":"(%s)"' % "|".join(KINDS))
        out = []
        for line in lines:
            new = kind.sub(lambda m: '"kind":"%s"' % KINDS[m.group(1)], line, count=1)
            renamed += new != line
            out.append(new)
        if write:
            with open(events + ".tmp", "w", encoding="utf-8") as f:
                f.write("\n".join(out))
            os.replace(events + ".tmp", events)

    print(f"{'migrated' if write else 'would migrate'} {changed} jellies ({views} views rewritten), {renamed} events renamed")
    for n in notes:
        print("note:", n)


if __name__ == "__main__":
    main()
