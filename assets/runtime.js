// Injected into every jelly view as `window.jelly`, grouped by what it is about.
//
// This jelly:
//   jelly.state                 current state (read only; change it with verbs)
//   jelly.onState(fn)           fn(state, event) now and after every change, from anyone
//   jelly.call(verb, args)      run a verb; resolves to the event, rejects with the reason
//   jelly.asset(file)           URL of a file added with `jelly asset` (or just use "assets/<file>")
//   jelly.events(fn)            fn(event) for every event of every jelly; returns unsubscribe
//
// Other jellies ("plan" means plan in this project):
//   jelly.other(name).state     a promise of its state
//   jelly.other(name).watch(fn) fn(state, event) now and after its changes; returns unsubscribe
//   jelly.other(name).open()    put it on screen: full size on the jelly page, else navigate
//
// The person:
//   jelly.person.name           this browser's name, "" if unset; assign to rename (kept per browser)
//   jelly.person.by             what this browser writes as: "person", or "person:<name>" once named
//   jelly.person.asks           every ask on this jelly, answered and withdrawn ones too;
//                               open ones have no .answer and no .withdrawn; .answer.option is the pick
//   jelly.person.onAsk(fn)      fn() now and whenever asks or comments change
//   jelly.person.onOpen(fn)     fn(ask) when the person opens an ask from the dock; returns unsubscribe
//   jelly.person.comments       comment threads on this jelly
//   jelly.person.answer(id, { option, text }), .comment(anchor, text), .reply(id, text), .resolve(id)
//
//   jelly.who(by)               "you" for this browser, a person's name, or the agent's name
//   jelly.isPerson(by)          true for "person" and "person:<name>"
//
// Comments anywhere: hold Option (Alt) and click any element, or press the
// Comment button, to pin a comment to it. The agent gets a `comment` event that
// names the element. Give elements an `id` so comments survive a reshape.
// Agents answer on the pin (`jelly reply`); the thread shows when you click it.
//
// On a share link (`jelly share --to`) the same runtime runs, with a few
// differences: jelly.other() is not available, jelly.person.name is whom the
// link is for and cannot be changed, and a view-only link refuses changes.
//
// A reshape or an undo that changes the view reloads the page. State lives in
// the daemon, so nothing is lost; scroll and the focused field (by id) carry
// over. On the jelly page the new version loads unseen and replaces the old
// one once it has drawn.
(() => {
  const boot = window.__JELLY__;
  // On a share link the view runs in a sandboxed frame with no origin of its
  // own. The relay's page around it holds the link's connection; requests and
  // events pass through that page by postMessage.
  const share = boot.share;
  const pending = new Map();
  let asked = 0;
  const viaShare = (op, body) => new Promise((ok, no) => {
    pending.set(++asked, { ok, no });
    parent.postMessage({ jelly: "rpc", id: asked, op, body }, "*");
  });
  // Errors the page hits, so `jelly look` can tell the agent its page is broken.
  const errors = [];
  let snapT = null;
  // `jelly look` renders with ?agent=1. Jellies framed inside that render (the
  // jelly page's tiles) are agent renders too.
  const rendered = (key) => new URLSearchParams(location.search).has(key) ||
    (() => { try { return parent !== window && new URLSearchParams(parent.location.search).has(key); } catch { return false; } })();
  const agentMode = rendered("agent");
  // `jelly look` also renders with the dark color scheme (and `&dark`) to check
  // contrast there; that render's snapshot is kept apart from the light one.
  const darkRender = agentMode && rendered("dark");
  // `jelly try` renders carry steps to do as a person would. Their verb calls run
  // against a throwaway copy of the state in the daemon, so nothing is saved.
  const trial = agentMode ? boot.try : undefined;
  const noteError = (msg) => { if (errors.length < 10 && msg && !errors.includes(msg)) { errors.push(String(msg).slice(0, 300)); postSnapshot(); } };
  addEventListener("error", (e) => noteError(e.error?.stack?.split("\n").slice(0, 2).join(" ") || e.message || (e.target?.src || e.target?.href ? `could not load ${e.target.src || e.target.href}` : "")), true);
  addEventListener("unhandledrejection", (e) => noteError(`unhandled rejection: ${e.reason?.message || e.reason}`));
  const consoleError = console.error.bind(console);
  console.error = (...a) => { noteError(a.map((x) => x?.stack || x?.message || String(x)).join(" ")); consoleError(...a); };
  let state = boot.state;
  let version = boot.version;
  let comments = boot.comments || [];
  let attention = boot.attention || [];
  const attentionSubs = [];
  const attendSubs = new Set();
  let working = boot.working || null;
  const subs = [];
  const enc = encodeURIComponent;
  const api = `/api/jellies/${enc(boot.name)}`;
  // `jelly look` renders post their snapshot to their own slot instead.
  const screenUrl = boot.screenUrl || `${api}/screen`;
  // Names are unique per project: a short name means a jelly in this one.
  const project = boot.name.includes(":") ? boot.name.split(":")[0] : "";
  const full = (name) => (name.includes(":") || name === "page" || !project ? name : `${project}:${name}`);

  function apply(ev) {
    if (ev.version <= version || ev.state === undefined) return;
    version = ev.version;
    state = ev.state;
    for (const fn of subs) {
      try { fn(state, ev); } catch (e) { console.error(e); }
    }
  }

  // The element behind a person's verb call, so the agent hears "clicked Pick B".
  let lastUsed = null;
  for (const type of ["pointerdown", "keydown"]) {
    addEventListener(type, (e) => {
      const el = e.target?.closest?.("button, a, input, select, textarea, [data-verb], [role=button], label") || e.target;
      if (el?.nodeType === 1 && !el.closest(".jf-pop, .jf-marker, .jf-ask")) lastUsed = { el, at: Date.now() };
    }, true);
  }
  function source() {
    if (!lastUsed || Date.now() - lastUsed.at > 2000) return undefined;
    const el = lastUsed.el;
    // For a field, name the field, not what was typed in it (that is in the args).
    const field = el.matches("input, textarea, select");
    const text = field ? el.getAttribute("aria-label") || el.placeholder || el.name || "a field" : el.innerText || el.getAttribute("aria-label") || "";
    return { selector: selectorFor(el), text: text.trim().replace(/\s+/g, " ").slice(0, 80) };
  }

  // A person may name themselves, so two people on one jelly can tell who did
  // what. The name is per browser (every jelly shares this origin) and read on
  // each write, so renaming on the jelly page applies to open jellies at once.
  const viewer = () => { if (share) return share.who; try { return localStorage.getItem("jelly.viewer") || ""; } catch { return ""; } };
  const me = () => (share ? share.by : viewer() ? `person:${viewer()}` : "person");
  const isPerson = (by) => by === "person" || String(by).startsWith("person:");
  const who = (by) => (by === me() ? "you" : isPerson(by) ? by.slice(7) || "someone" : by);

  async function post(op, body) {
    // Nobody is at an agent render, so nothing it does may be recorded as the person.
    if (agentMode) throw new Error("jelly look only looks; this change was not made");
    if (share) return viaShare(op, body);
    const r = await fetch(`${api}/${op === "answer" ? `attention/${enc(body.id)}/answer` : op}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...body, by: me() }),
    });
    const j = await r.json();
    if (!r.ok) throw new Error(j.error || r.statusText);
    return j;
  }

  // What a reload keeps. Views are arbitrary pages, so only what can be found
  // again in the new version carries over: the page's scroll, scrolled boxes
  // with an id, and the focused element by id, with its text and selection.
  const TEXT = ["text", "search", "url", "email", "tel", "password", "number"];
  const carry = {
    take() {
      const f = document.hasFocus() && document.activeElement?.id ? document.activeElement : null;
      const typed = f && (f.localName === "textarea" || (f.localName === "input" && TEXT.includes(f.type)));
      return {
        x: scrollX, y: scrollY,
        boxes: [...document.querySelectorAll("[id]")].filter((el) => el.scrollTop || el.scrollLeft).map((el) => [el.id, el.scrollTop, el.scrollLeft]),
        focus: f && { id: f.id, type: f.type, value: typed ? f.value : null, sel: typed ? [f.selectionStart, f.selectionEnd, f.selectionDirection] : null },
      };
    },
    put(kept) {
      if (!kept) return;
      for (const [id, top, left] of kept.boxes) { const el = document.getElementById(id); if (el) { el.scrollTop = top; el.scrollLeft = left; } }
      const f = kept.focus && document.getElementById(kept.focus.id);
      if (f) {
        f.focus({ preventScroll: true });
        if (kept.focus.value !== null && f.type === kept.focus.type && f.value !== kept.focus.value) f.value = kept.focus.value;
        try { if (kept.focus.sel) f.setSelectionRange(...kept.focus.sel); } catch {} // number and email fields have no selection
      }
      scrollTo(kept.x, kept.y);
    },
  };
  // The jelly page reads and writes this on the frames it swaps.
  window.__jellyCarry = carry;
  const carryKey = `jelly:carry:${boot.name}`;
  let swapAsked = false;
  function reload() {
    if (share) return parent.postMessage({ jelly: "rpc", op: "reload" }, "*");
    try {
      // The jelly page swaps in a fresh copy of this frame once that has drawn.
      // Asked once: later events must not restart a swap that is loading.
      if (frameElement?.hasAttribute("data-jelly-swap")) {
        if (!swapAsked) parent.postMessage({ type: "jelly:reload" }, location.origin);
        swapAsked = true;
        return;
      }
      // A page of its own carries scroll and focus through the reload. The browser
      // keeps the old page on screen until the new one paints.
      if (window === top) sessionStorage.setItem(carryKey, JSON.stringify(carry.take()));
    } catch {}
    location.reload();
  }
  try {
    const kept = JSON.parse(sessionStorage.getItem(carryKey));
    sessionStorage.removeItem(carryKey);
    if (kept) addEventListener("load", () => carry.put(kept));
  } catch {}

  // One WebSocket per page carries every event; handlers filter by jelly.
  const listeners = new Set();
  let dropped = false;
  if (share) addEventListener("message", (e) => {
    const m = e.data;
    if (e.source !== parent || !m) return;
    if (m.jelly === "reply") {
      const p = pending.get(m.id);
      pending.delete(m.id);
      if (p) m.ok ? p.ok(m.value) : p.no(new Error(m.error));
    }
    if (m.jelly === "event") for (const fn of listeners) {
      try { fn(m.ev); } catch (err) { console.error(err); }
    }
  });
  else (function connect() {
    const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/ws`);
    ws.onopen = () => { if (dropped) reload(); };
    ws.onmessage = (m) => {
      const ev = JSON.parse(m.data);
      if (ev.kind === "resync") return reload();
      for (const fn of listeners) {
        try { fn(ev); } catch (e) { console.error(e); }
      }
    };
    ws.onclose = () => { dropped = true; setTimeout(connect, 1000); };
  })();

  listeners.add((ev) => {
    // A trial shows its own copy of the state, and `look --compare` an earlier
    // version; real changes meanwhile do not reach either.
    if (ev.jelly !== boot.name || trial || boot.past) return;
    if (ev.kind === "remove") {
      document.body.innerHTML = '<p style="font:14px system-ui;padding:16px;opacity:.7">This jelly was removed.</p>';
      return;
    }
    if (ev.kind === "new" || ev.view_rev !== boot.viewRev) { reload(); return; }
    if (ev.kind === "comment" || ev.kind === "resolve" || ev.kind === "reply") refreshThreads().catch(console.error);
    if (ev.kind === "attention" || ev.kind === "answer" || ev.kind === "withdraw") refreshThreads().catch(console.error);

    if (ev.kind === "working") { working = ev.args ? { what: ev.args, by: ev.by } : null; drawWorking(); }
    apply(ev);
  });

  const events = (fn) => { listeners.add(fn); return () => listeners.delete(fn); };
  const readState = async (name) => {
    if (share) throw new Error("other jellies are not available on a shared link");
    const r = await fetch(`/api/jellies/${enc(name)}/state`);
    if (!r.ok) throw new Error((await r.json()).error);
    return r.json();
  };

  window.jelly = {
    name: boot.name,
    about: boot.about,
    get state() { return state; },
    onState(fn) { subs.push(fn); fn(state, null); },
    async call(verb, args = {}) {
      const ev = trial ? await trialCall(verb, args) : await post("call", { verb, args, source: source() });
      apply(ev);
      return ev;
    },
    asset: (file) => (share ? `assets/${enc(file)}` : `/j/${enc(boot.name)}/assets/${enc(file)}`),
    events,
    other(name) {
      name = full(name);
      return {
        name,
        get state() { return readState(name); },
        watch(fn) {
          let seen = 0;
          readState(name).then((s) => fn(s, null), (e) => console.warn(e.message));
          return events((ev) => {
            if (ev.jelly === name && ev.state !== undefined && ev.version > seen) { seen = ev.version; fn(ev.state, ev); }
          });
        },
        open() {
          if (share) return;
          if (embedded) parent.postMessage({ type: "jelly:show", name }, location.origin);
          else location.href = `/j/${enc(name)}/`;
        },
      };
    },
    person: {
      get name() { return viewer(); },
      set name(name) {
        if (share) return;
        name = String(name ?? "").trim().replace(/\s+/g, " ").slice(0, 40);
        try { name ? localStorage.setItem("jelly.viewer", name) : localStorage.removeItem("jelly.viewer"); } catch {}
      },
      get by() { return me(); },
      get asks() { return attention; },
      onAsk(fn) { attentionSubs.push(fn); fn(); },
      onOpen(fn) { attendSubs.add(fn); return () => attendSubs.delete(fn); },
      get comments() { return comments; },
      /** An option id, text, or both. A seen ask takes nothing ("seen"). */
      answer: (id, { option, text } = {}) => post("answer", { id, option, text: text || "" }),
      comment: (anchor, text) => post("comment", { anchor, text }),
      reply: (id, text) => post("reply", { id, text }),
      resolve: (id) => post("resolve", { id }),
    },
    who,
    isPerson,
  };

  // ---- comments: markers on the elements they are about ------------------------

  // Views place Comment via --jelly-comment-inset inherited from root or body.
  const css = `
    .jf-btn { position: fixed; inset: var(--jelly-comment-inset, auto 10px 10px auto); z-index: 2147483000; font: 12px -apple-system, sans-serif;
      background: #1a2021; color: #d6dedc; border: 1px solid #2f383a; border-radius: 999px; padding: 5px 11px; cursor: pointer; opacity: .85; }
    .jf-btn.on { background: #e0a24f; color: #1b2221; border-color: #e0a24f; opacity: 1; }
    .jf-hover { outline: 2px dashed #e0a24f !important; outline-offset: 2px; cursor: crosshair !important; }
    .jf-marker { position: absolute; z-index: 2147482999; width: 20px; height: 20px; border-radius: 50% 50% 0 50%; background: #e0a24f; color: #1b2221;
      font: 600 11px -apple-system, sans-serif; display: grid; place-items: center; cursor: pointer; box-shadow: 0 2px 6px rgba(0,0,0,.3); }
    .jf-marker.agent { background: #46c4b8; }
    .jf-marker.done { opacity: .55; }
    .jf-pop { color-scheme: light; position: absolute; z-index: 2147483001; width: 260px; background: #fff; color: #1b2221; border: 1px solid #e2e7e5; border-radius: 10px;
      box-shadow: 0 14px 34px -10px rgba(0,0,0,.45); padding: 8px; font: 12px -apple-system, sans-serif; }
    .jf-pop .jf-on { font: 10px ui-monospace, Menlo, monospace; color: #66726f; margin-bottom: 5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .jf-pop textarea { width: 100%; box-sizing: border-box; min-height: 54px; font: 12px -apple-system, sans-serif; border: 1px solid #e2e7e5; border-radius: 6px; padding: 5px; resize: vertical; }
    .jf-pop .jf-row { display: flex; gap: 6px; justify-content: flex-end; margin-top: 6px; align-items: center; }
    .jf-pop button { font: 12px -apple-system, sans-serif; color: #1b2221; border: 1px solid #e2e7e5; background: #fff; border-radius: 6px; padding: 3px 9px; cursor: pointer; }
    .jf-pop button.jf-send { background: #0f7b74; color: #fff; border-color: #0f7b74; }
    .jf-pop .jf-by { font: 10px ui-monospace, monospace; color: #b26a12; }
    .jf-pop .jf-thread { display: grid; gap: 6px; margin-bottom: 6px; max-height: 260px; overflow-y: auto; }
    .jf-pop .jf-msg { border-left: 2px solid #e0a24f; padding: 1px 0 1px 7px; }
    .jf-pop .jf-msg.agent { border-color: #46c4b8; }
    .jf-pop .jf-msg.agent .jf-by { color: #0f7b74; }
    .jf-pop .jf-text { white-space: pre-wrap; margin-top: 1px; }
    .jf-pop .jf-done { font-size: 11px; color: #66726f; }
    .jf-work { position: fixed; left: 50%; top: 10px; transform: translateX(-50%); z-index: 2147483000; background: #141a19; color: #e8eeec; border-radius: 999px; padding: 6px 14px 6px 12px; font: 12.5px -apple-system, sans-serif; display: flex; gap: 8px; align-items: center; box-shadow: 0 8px 24px -8px rgba(0,0,0,.5); }
    .jf-work i { width: 8px; height: 8px; border-radius: 50%; background: #46c4b8; animation: jf-blink 1s infinite; } @keyframes jf-blink { 50% { opacity: .3; } }
    `;
  let commenting = false, hovered = null, pop = null;
  const btn = document.createElement("button");
  btn.className = "jf-btn";
  btn.textContent = "Comment";
  btn.title = "Pin a comment to any part of this jelly (or hold Option and click)";

  // Everything the runtime adds to a view. Selectors name view elements only.
  const OURS = ".jf-btn, .jf-pop, .jf-marker, .jf-ask, .jf-work";
  const isOurs = (el) => el.closest(OURS);
  function selectorFor(el) {
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && n !== document.body; n = n.parentElement) {
      if (n.id) { parts.unshift("#" + CSS.escape(n.id)); break; }
      let i = 1;
      for (let s = n.previousElementSibling; s; s = s.previousElementSibling) if (s.tagName === n.tagName && !isOurs(s)) i++;
      parts.unshift(`${n.tagName.toLowerCase()}:nth-of-type(${i})`);
    }
    return parts.join(" > ") || "body";
  }

  // On the jelly page, the page's header owns the Comment button.
  const embedded = !share && window.parent !== window;
  function setMode(on) {
    commenting = on;
    btn.classList.toggle("on", on);
    btn.textContent = on ? "Click a part to comment · Esc" : "Comment";
    if (!on && hovered) { hovered.classList.remove("jf-hover"); hovered = null; }
    if (embedded) parent.postMessage({ type: "jelly:commenting", name: boot.name, on }, location.origin);
  }
  addEventListener("message", (e) => {
    if (e.origin === location.origin && e.data?.type === "jelly:comment") setMode(!!e.data.on);
  });

  function openPop(x, y, html) {
    pop?.remove();
    pop = document.createElement("div");
    pop.className = "jf-pop";
    pop.innerHTML = html;
    document.body.appendChild(pop);
    const w = document.documentElement.clientWidth;
    pop.style.left = Math.max(8, Math.min(x, w - 276)) + scrollX + "px";
    pop.style.top = y + scrollY + 10 + "px";
    return pop;
  }

  function startComment(el, x, y) {
    const anchor = { selector: selectorFor(el), quote: (el.innerText || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 80) };
    const p = openPop(x, y, `<div class="jf-on"></div><textarea placeholder="What should change here?"></textarea>
      <div class="jf-row"><button class="jf-cancel">Cancel</button><button class="jf-send">Send</button></div>`);
    p.querySelector(".jf-on").textContent = anchor.quote ? `on "${anchor.quote}"` : `on ${anchor.selector}`;
    const ta = p.querySelector("textarea");
    ta.focus();
    const send = () => jelly.person.comment(anchor, ta.value).then(() => { p.remove(); pop = null; }, (e) => alert(e.message));
    p.querySelector(".jf-send").onclick = send;
    p.querySelector(".jf-cancel").onclick = () => { p.remove(); pop = null; };
    ta.onkeydown = (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } if (e.key === "Escape") { p.remove(); pop = null; } };
  }

  let markerEls = [];
  const DAY = 86400000;
  const lastAt = (f) => Math.max(f.at_ms, ...(f.replies || []).map((r) => r.at_ms));
  // Open threads, plus resolved ones from the last day so the person sees the answer.
  // A comment without a selector is about the whole jelly: it has no element to
  // pin to, and the jelly page shows it in the jelly's header instead.
  const visible = () => comments.filter((f) => f.anchor?.selector && (!f.resolved || Date.now() - lastAt(f) < DAY));
  function thread(f, x, y) {
    const msgs = [{ by: f.by, text: f.text, at_ms: f.at_ms }, ...(f.replies || [])];
    const p = openPop(x, y, `<div class="jf-on"></div><div class="jf-thread"></div>
      <textarea placeholder="${f.resolved ? "Not done? Say what is still off" : "Reply"}"></textarea>
      <div class="jf-row"><button class="jf-close">Close</button>${f.resolved ? "" : '<button class="jf-resolve">Resolve</button>'}<button class="jf-send">Reply</button></div>`);
    p.querySelector(".jf-on").textContent = f.anchor?.quote ? `on "${f.anchor.quote}"` : `on ${f.anchor?.selector || "the jelly"}`;
    const t = p.querySelector(".jf-thread");
    for (const m of msgs) {
      const d = document.createElement("div");
      d.className = "jf-msg" + (isPerson(m.by) ? "" : " agent");
      d.innerHTML = '<div class="jf-by"></div><div class="jf-text"></div>';
      d.querySelector(".jf-by").textContent = who(m.by);
      d.querySelector(".jf-text").textContent = m.text;
      t.appendChild(d);
    }
    if (f.resolved) t.insertAdjacentHTML("beforeend", '<div class="jf-done">Resolved</div>');
    const ta = p.querySelector("textarea");
    const close = () => { p.remove(); pop = null; };
    const send = () => ta.value.trim() && jelly.person.reply(f.id, ta.value).then(close, (e) => alert(e.message));
    p.querySelector(".jf-close").onclick = close;
    p.querySelector(".jf-send").onclick = send;
    const r = p.querySelector(".jf-resolve");
    if (r) r.onclick = () => jelly.person.resolve(f.id).then(close);
    ta.onkeydown = (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } if (e.key === "Escape") close(); };
  }

  function drawMarkers() {
    drawAttention();
    markerEls.forEach((p) => p.remove());
    markerEls = [];
    const shown = visible();
    const anchors = attentionAnchors(shown.map((f) => ({ anchor: { selector: f.anchor.selector } })), keepFocus());
    shown.forEach((f, i) => {
      const el = anchors[i].element;
      if (!el) return;
      const r = el.getBoundingClientRect();
      const last = f.replies?.at(-1);
      const pin = document.createElement("div");
      // Amber: waiting on the agent. Teal: the agent answered. Faded: resolved.
      pin.className = "jf-marker" + (last && !isPerson(last.by) ? " agent" : "") + (f.resolved ? " done" : "");
      pin.textContent = f.resolved ? "✓" : i + 1;
      pin.title = last ? `${who(last.by)}: ${last.text}` : f.text;
      // In the margin left of the element, pointing at it.
      pin.style.left = Math.max(2, r.left - 24) + scrollX + "px";
      pin.style.top = r.top + Math.min(r.height, 40) / 2 - 10 + scrollY + "px";
      pin.onclick = (e) => { e.stopPropagation(); thread(f, r.left, r.bottom); };
      document.body.appendChild(pin);
      markerEls.push(pin);
    });
  }
  // ---- attention: questions sit on the page, right below what they are about ----
  // Each question is a card in the page flow, after its anchor row (or at the
  // top when it has none). It lives in a shadow root so the page's CSS and ours
  // never touch. Answered questions shrink to one line for a day, with Change.
  const DAY_MS = 86400000;
  const cards = new Map(); // item id -> host element, reused so drafts survive redraws
  const isOpen = (a) => !a.answer && !a.withdrawn;
  const shownItems = () => attention.filter((a) => !a.withdrawn && (!a.answer || Date.now() - a.answer.at_ms < DAY_MS));
  const editing = new Set(); // answered items the person reopened with Change
  const CARD_CSS = `
    :host { all: initial; display: block; margin: 14px 0; --bg:#fff; --ink:#2a1620; --soft:#6b5260; --line:#f0dfe5; --tint:#fdf0f4; --accent:#e8336f; --accent-ink:#a3134c; }
    :host([dark]) { --bg:#25162c; --ink:#f6e8f2; --soft:#c3a9bd; --line:#43294d; --tint:#33203b; --accent:#ff5c95; --accent-ink:#ff8fb6; }
    * { box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; letter-spacing: normal; text-transform: none; }
    .c { background: var(--bg); color: var(--ink); border: 1.5px solid var(--line); border-left: 4px solid var(--accent); border-radius: 14px; padding: 16px 18px; font-size: 14px; line-height: 1.5; text-align: left; }
    .c.flash { animation: flash 1.2s ease-out; } @keyframes flash { 0% { box-shadow: 0 0 0 0 var(--accent); } 40% { box-shadow: 0 0 0 6px color-mix(in srgb, var(--accent) 25%, transparent); } }
    .who { font: 600 12px ui-monospace, Menlo, monospace; color: var(--accent-ink); }
    h3 { margin: 4px 0 0; font-size: 18px; line-height: 1.3; font-weight: 700; }
    .ctx { margin: 6px 0 0; color: var(--soft); white-space: pre-wrap; } .ctx:empty { display: none; }
    .rec { margin-top: 10px; background: var(--tint); border-radius: 10px; padding: 8px 12px; font-size: 13.5px; }
    .rec b { color: var(--accent-ink); }
    .opts { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 8px; margin-top: 12px; }
    .o { all: unset; box-sizing: border-box; cursor: pointer; padding: 11px 13px; border: 1.5px solid var(--line); border-radius: 11px; background: var(--bg); color: var(--ink); }
    button:focus-visible, textarea:focus-visible, .c:focus-visible { outline: 3px solid var(--accent); outline-offset: 3px; }
    .exposure { color: var(--soft); font-size: 12px; margin-bottom: 8px; }
    .exposure:empty { display: none; }
    .o:hover { border-color: var(--accent); }
    .o b { display: block; font-size: 15px; font-weight: 650; }
    .o span { display: block; font-size: 13px; color: var(--soft); margin-top: 2px; } .o span:empty { display: none; }
    .o.rec-o b::after { content: "recommended"; margin-left: 8px; font-size: 11px; font-weight: 600; color: #fff; background: var(--accent); border-radius: 99px; padding: 1px 7px; vertical-align: 2px; }
    .o.on { border-color: var(--accent); background: var(--tint); }
    .row { display: flex; gap: 8px; margin-top: 10px; align-items: flex-end; }
    textarea { flex: 1; min-width: 0; min-height: 40px; font-size: 14px; color: var(--ink); background: var(--bg); border: 1.5px solid var(--line); border-radius: 10px; padding: 8px 10px; resize: vertical; }
    textarea:focus { outline: none; border-color: var(--accent); }
    .go { all: unset; cursor: pointer; font-size: 14px; font-weight: 600; padding: 9px 16px; border-radius: 10px; background: var(--accent); color: #fff; white-space: nowrap; }
    .go[disabled] { opacity: .4; cursor: default; }
    .hint { font-size: 12px; color: var(--soft); margin-top: 6px; }
    .done { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; }
    .done .q { color: var(--soft); } .done .a { font-weight: 650; } .done .n { color: var(--soft); white-space: pre-wrap; }
    .chg { all: unset; cursor: pointer; color: var(--accent-ink); font-size: 13px; font-weight: 600; margin-left: auto; }
    .c.answered { border-left-color: #3aa57a; padding: 10px 14px; }`;

  // Light or dark from the page's own background, so the card belongs to the page.
  function pageIsDark() {
    for (const el of [document.body, document.documentElement]) {
      const m = getComputedStyle(el).backgroundColor.match(/[\d.]+/g);
      if (m && (m.length < 4 || +m[3] > 0.5)) return (0.299 * m[0] + 0.587 * m[1] + 0.114 * m[2]) < 128;
    }
    // No background anywhere: the browser paints white unless the page opts into dark.
    const scheme = getComputedStyle(document.documentElement).colorScheme + " " + (document.querySelector('meta[name="color-scheme"]')?.content || "");
    return /dark/.test(scheme) && matchMedia("(prefers-color-scheme: dark)").matches;
  }
  function normalBoundary(el) {
    while (el?.parentElement && el.parentElement !== document.body) {

      const p = el.parentElement, d = getComputedStyle(p).display;
      if (/grid|flex|table|inline/.test(d) || ["UL", "OL", "TR", "TBODY", "THEAD", "TFOOT", "TABLE", "P", "BUTTON", "A", "LABEL"].includes(p.tagName)) el = p;
      else break;
    }
    return el === document.body || el === document.documentElement ? null : el;
  }
  function fallback(group) {
    return { kind: "block", after: normalBoundary(group), observe: group, status: "fallback", reason: "this layout cannot take a full-width question row" };
  }
  function rowPlacement(group, item) {
    const cs = getComputedStyle(group), grid = cs.display.includes("grid");
    if (cs.transform !== "none" || [...group.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim()) ||
        cs.writingMode !== "horizontal-tb" || cs.direction !== "ltr" ||
        (grid ? cs.gridAutoFlow !== "row" : cs.flexDirection !== "row" || cs.flexWrap !== "wrap")) return fallback(group);
    const children = [...group.children].filter((el) => !isOurs(el) && el.getClientRects().length && !["absolute", "fixed"].includes(getComputedStyle(el).position));
    const rows = [];
    for (const el of children) {
      const style = getComputedStyle(el), r = el.getBoundingClientRect();
      if (style.order !== "0" || style.transform !== "none" || style.position === "relative" && [style.top, style.right, style.bottom, style.left].some((v) => v !== "auto" && parseFloat(v) !== 0) ||
          (grid && [style.gridRowStart, style.gridRowEnd, style.gridColumnStart, style.gridColumnEnd].some((v) => v !== "auto"))) return fallback(group);
      if (!r.width || !r.height) continue;
      let row = rows.at(-1);
      if (!row || r.top >= row.bottom - 1) { row = { top: r.top, bottom: r.bottom, members: [] }; rows.push(row); }
      else if (r.bottom <= row.top + 1 || row.members.some((peer) => {
        const pr = peer.getBoundingClientRect();
        return r.left < pr.right - 1 && r.right > pr.left + 1;
      })) return fallback(group);
      row.top = Math.min(row.top, r.top); row.bottom = Math.max(row.bottom, r.bottom); row.members.push(el);
    }
    const i = rows.findIndex((row) => row.members.includes(item));
    if (i < 0) return fallback(group);
    return { kind: grid ? "grid" : "flex", group, after: rows[i].members.at(-1), peers: rows[i].members, later: rows.slice(i + 1).flatMap((row) => row.members) };
  }
  function tablePlacement(row) {
    const table = row.closest("table"), rows = [...table.rows].filter((r) => !isOurs(r));
    let occupied = [], rowGroup = null;
    let columns = 0, crossing = false;
    for (const r of rows) {
      if (r.parentElement !== rowGroup) { occupied = []; rowGroup = r.parentElement; }
      let col = 0;
      for (const cell of r.cells) {
        while (occupied[col] > 0) col++;
        const span = cell.rowSpan || Infinity;
        for (let i = 0; i < cell.colSpan; i++) occupied[col++] = span;
      }
      columns = Math.max(columns, occupied.length);
      if (r === row) crossing = occupied.some((span) => span > 1);
      for (let i = 0; i < occupied.length; i++) occupied[i] = Math.max(0, occupied[i] - 1);
    }
    return crossing ? fallback(table) : { kind: "table", group: table, after: row, columns, peers: [row], later: rows.slice(rows.indexOf(row) + 1) };
  }
  function listCanInsert(group) {
    // Without an explicit start, a reversed list counts every li, including slots.
    if (group.reversed && !group.hasAttribute("start")) return false;
    return [...group.children].filter((el) => !isOurs(el)).every((el) => {
      const increment = getComputedStyle(el).counterIncrement;
      return ["none", "list-item 1", "list-item -1"].includes(increment) &&
        ["::before", "::after"].every((pseudo) => ["none", "normal"].includes(getComputedStyle(el, pseudo).content));
    });
  }
  function resolvePlacement(a, { element: el, invalid }) {
    if (invalid) return { kind: "block", after: null, status: "invalid" };
    if (!el) return { kind: "block", after: null, status: a.anchor?.selector ? "missing" : "direct" };
    if (el === document.body || el === document.documentElement || isOurs(el)) return { kind: "block", after: null, status: "floating" };
    const hidden = !available(el);
    const svg = !!el.ownerSVGElement;
    while (el.ownerSVGElement) el = el.ownerSVGElement;
    for (let n = el; n && n !== document.body; n = n.parentElement) if (["fixed", "absolute", "sticky"].includes(getComputedStyle(n).position)) return { kind: "block", after: null, status: "floating" };
    if (hidden) return { kind: "block", after: null, status: "hidden" };
    let result;
    for (let item = el; item && item !== document.body; item = item.parentElement) {
      const group = item.parentElement;
      if (!group) break;
      const display = getComputedStyle(group).display;
      if (["TD", "TH"].includes(item.tagName)) { result = tablePlacement(item.parentElement); break; }
      if (item.tagName === "TR") { result = tablePlacement(item); break; }
      if (["UL", "OL"].includes(group.tagName) && !listCanInsert(group)) { result = fallback(group); break; }
      if (display.includes("grid") || display.includes("flex")) {
        const restricted = group.closest("button, a, label, p");
        if (restricted === group) continue;
        result = restricted ? fallback(restricted) : rowPlacement(group, item);
        break;
      }
      if (item.tagName === "LI" && ["UL", "OL"].includes(group.tagName)) {
        const siblings = [...group.children].filter((el) => !isOurs(el));
        result = { kind: "list", after: item, group, peers: [item], later: siblings.slice(siblings.indexOf(item) + 1) };
        break;
      }
    }
    result ||= { kind: "block", after: normalBoundary(el) };
    if (!result.status && result.kind === "block" && result.after !== el) { result.status = "fallback"; result.reason = "the anchor cannot contain a question card"; }
    if (svg && !result.status) { result.status = "fallback"; result.reason = "the anchor is inside a drawing"; }
    return result;
  }
  function renderCard(host, a) {
    const root = host.shadowRoot;
    const isAsk = !a.seen; // a seen ask can be answered with nothing
    const opts = a.options || [];
    const answered = a.answer && !editing.has(a.id);
    const draft = root.querySelector("textarea")?.value || "";
    root.innerHTML = `<style>${CARD_CSS}</style><div class="c${answered ? " answered" : ""}"></div>`;
    const c = root.querySelector(".c");
    c.tabIndex = -1;
    c.setAttribute("aria-label", a.title);
    if (answered) {
      const o = opts.find((x) => x.id === a.answer.option);
      c.innerHTML = `<div class="done"><span class="q"></span><span class="a"></span><span class="n"></span><button class="chg">Change</button></div>`;
      c.querySelector(".q").textContent = a.title;
      c.querySelector(".a").textContent = "✓ " + (o ? o.label || o.id : a.seen ? "Seen" : "Answered");
      c.querySelector(".n").textContent = a.answer.text || "";
      c.querySelector(".chg").onclick = () => { editing.add(a.id); drawAttention(); };
      return;
    }
    const rec = opts.find((o) => o.id === a.recommend);
    c.innerHTML = `<div class="who"></div><h3></h3><p class="ctx"></p>
      ${rec ? '<div class="rec"><b></b> <span></span></div>' : ""}
      ${opts.length ? '<div class="opts"></div>' : ""}
      <div class="row"><textarea rows="1"></textarea><button class="go"></button></div>
      <div class="hint">${opts.length ? "Click an option to answer. Add a note first if you like." : "Enter to send · Shift+Enter for a new line"}</div>`;
    c.querySelector(".who").textContent = `${a.by} ${isAsk ? "asks" : "asks you to look"}`;
    c.querySelector("h3").textContent = a.title;
    c.querySelector(".ctx").textContent = a.context || "";
    if (rec) { c.querySelector(".rec b").textContent = `Recommends ${rec.label || rec.id}.`; c.querySelector(".rec span").textContent = a.why || ""; }
    const ta = c.querySelector("textarea"), go = c.querySelector(".go");
    ta.value = draft;
    ta.placeholder = !isAsk ? "Anything to say? (optional)" : opts.length ? "A note, or your own answer if none fit" : "Your answer";
    go.textContent = !isAsk ? "Seen" : opts.length ? "Answer in words" : "Answer";
    const sync = () => (go.disabled = isAsk && !ta.value.trim());
    ta.oninput = sync; sync();
    const send = (option) => {
      if (isAsk && !option && !ta.value.trim()) return;
      go.disabled = true;
      jelly.person.answer(a.id, { option, text: ta.value }).then(() => editing.delete(a.id), (e) => { sync(); alert(e.message); });
    };
    for (const o of opts) {
      const b = document.createElement("button");
      b.className = "o" + (o.id === a.recommend ? " rec-o" : "") + (a.answer?.option === o.id ? " on" : "");
      b.innerHTML = "<b></b><span></span>";
      b.querySelector("b").textContent = o.label || o.id;
      b.querySelector("span").textContent = o.does || "";
      b.onclick = () => send(o.id);
      c.querySelector(".opts").appendChild(b);
    }
    go.onclick = () => send(undefined);
    ta.onkeydown = (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(undefined); } };
  }
  function anchorElement(a) {
    const selector = a.anchor?.selector;
    const question = inline() && selector?.startsWith("#q-") ? document.getElementById(selector.slice(1)) : null;
    if (question) return question;
    try { return selector ? document.querySelector(selector) : null; } catch { return null; }
  }
  function available(el, whole = false) {
    if (!el?.isConnected || el.closest("[inert]")) return false;
    if (getComputedStyle(el).display === "contents") return [...el.children].some((c) => available(c, whole));
    if (!el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true })) return false;
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return false;
    for (let p = el.parentElement; p; p = p.parentElement) {
      const cs = getComputedStyle(p), b = p.getBoundingClientRect();
      const left = b.left + p.clientLeft, top = b.top + p.clientTop;
      const right = left + p.clientWidth, bottom = top + p.clientHeight;
      if (["hidden", "clip"].includes(cs.overflowX) && (whole ? r.left < left - 1 || r.right > right + 1 : r.right <= left || r.left >= right)) return false;
      if (["hidden", "clip"].includes(cs.overflowY) && (whole ? r.top < top - 1 || r.bottom > bottom + 1 : r.bottom <= top || r.top >= bottom)) return false;
    }
    return true;
  }
  // Hiding a subtree can blur its control before the placement observer runs.
  let blurredCardControl = null;
  addEventListener("focusin", () => { blurredCardControl = null; });
  addEventListener("focusout", (e) => { blurredCardControl = e.target.matches?.(".jf-ask") && !available(e.target) ? e.composedPath()[0] : null; });
  let topStack = null;
  function exposureLabel(host, a, rescued) {
    let label = host.shadowRoot.querySelector(".exposure");
    if (!label) {
      label = document.createElement("div");
      label.className = "exposure";
      host.shadowRoot.querySelector(".c").prepend(label);
    }
    const text = rescued ? `About ${a.anchor?.quote || a.anchor?.selector} · The question is shown here so you can answer it.` : "";
    if (label.textContent !== text) label.textContent = text;
    host.toggleAttribute("data-exposed", rescued);
  }
  let outcomes = new Map();
  const slots = new Map();
  const groupObserver = new ResizeObserver(() => queueDraw());
  let observedGroups = new Set();
  function slotFor(id, host, place) {
    // The form's intrinsic width must not widen an auto-layout table.
    host.style.contain = place.kind === "table" ? "inline-size" : "";
    const tag = place.kind === "table" ? "tr" : place.kind === "list" || ["UL", "OL"].includes(place.group?.tagName) ? "li" : "div";
    let slot = slots.get(id);
    if (!slot || slot.localName !== tag) {
      slot?.remove();
      slot = document.createElement(tag);
      slot.className = "jf-ask jf-slot";
      slots.set(id, slot);
      if (tag === "tr") slot.appendChild(document.createElement("td"));
    }
    const container = tag === "tr" ? slot.firstElementChild : slot;
    if (host.parentElement !== container) container.appendChild(host);
    let style = "display:block;min-width:0;box-sizing:border-box;";
    if (tag === "li") style += "list-style:none;counter-increment:list-item 0;padding:0;";
    if (place.kind === "grid") style += "grid-column:1 / -1;grid-row:auto;align-self:stretch;justify-self:stretch;";
    if (place.kind === "flex") style += "flex:0 0 100%;";
    if (tag === "tr") {
      style = "";
      const cell = slot.firstElementChild;
      cell.colSpan = place.columns;
      cell.style.cssText = "padding:0;min-width:0;";
    }
    if (slot.style.cssText !== style) slot.style.cssText = style;
    return slot;
  }
  function insertSlot(slot, place, tails) {
    if (place.after) {
      const tail = tails.get(place.after) || place.after;
      if (tail.nextElementSibling !== slot) tail.after(slot);
      tails.set(place.after, slot);
    } else {
      if (!topStack?.isConnected) { topStack = document.createElement("div"); topStack.className = "jf-ask jf-ask-top"; }
      if (document.body.firstElementChild !== topStack) document.body.prepend(topStack);
      const style = "display:block;grid-column:1/-1;flex-basis:100%;box-sizing:border-box;width:100%;min-width:0;max-width:calc(100vw - 32px);margin:14px 0";
      if (topStack.style.cssText !== style) topStack.style.cssText = style;
      const tail = tails.get(topStack);
      if (tail) { if (tail.nextElementSibling !== slot) tail.after(slot); }
      else if (topStack.firstElementChild !== slot) topStack.prepend(slot);
      tails.set(topStack, slot);
    }
  }
  function rowFits(place, slot) {
    const r = slot.getBoundingClientRect(), g = place.group.getBoundingClientRect(), cs = getComputedStyle(place.group);
    if (place.kind === "table") {
      const peer = place.after.getBoundingClientRect();
      return Math.abs(r.left - peer.left) <= 1 && Math.abs(r.right - peer.right) <= 1 &&
        peer.bottom <= r.top + 1 && place.later.every((el) => el.getBoundingClientRect().top >= r.bottom - 1);
    }
    const left = g.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft);
    const right = g.right - parseFloat(cs.borderRightWidth) - parseFloat(cs.paddingRight);
    return Math.abs(r.left - left) <= 1 && Math.abs(r.right - right) <= 1 &&
      place.peers.every((el) => el.getBoundingClientRect().bottom <= r.top + 1) &&
      place.later.every((el) => el.getBoundingClientRect().top >= r.bottom - 1);
  }
  function attentionAnchors(items, restoreFocus) {
    const focusStates = ["focus", "focus-visible", "focus-within"].map((pseudo) => ({
      pseudo, elements: [...document.querySelectorAll(`:${pseudo}`)].filter((el) => !isOurs(el)),
    }));
    // Structural selectors need owned siblings absent, but detachment clears
    // focus pseudos. Match saved elements by their paths in the original tree.
    // A style rule's selectorText is the browser's own serialization: escapes
    // resolved, names lowercased, invalid forgiving arguments dropped.
    const sheet = new CSSStyleSheet();
    sheet.replaceSync("*{}");
    const rule = sheet.cssRules[0];
    const elementPath = (el) => {
      const path = [];
      for (let node = el; node.parentElement; node = node.parentElement) {
        let index = 1;
        for (let sibling = node.previousElementSibling; sibling; sibling = sibling.previousElementSibling) index++;
        path.unshift(`:nth-child(${index})`);
      }
      return [":root", ...path].join(" > ");
    };
    const owned = [...document.querySelectorAll(OURS)]
      .filter((el) => !el.parentElement || !isOurs(el.parentElement));
    const positions = owned.map((el) => [el, el.parentNode, el.nextSibling]);
    try {
      for (const el of owned) el.remove();
      const focusSelectors = focusStates.map(({ pseudo, elements }) => ({
        pseudo, selector: elements.map(elementPath).join(",") || ":not(*)",
      }));
      const selectorWithFocus = (selector) => {
        // An invalid selector leaves selectorText as it was, so reject it first.
        document.createDocumentFragment().querySelector(selector);
        rule.selectorText = selector;
        return rule.selectorText.replace(/"(?:\\[\s\S]|[^"\\])*"|\\[\s\S]|:[\w-]+/g, (token) => {
          const state = focusSelectors.find((state) => `:${state.pseudo}` === token);
          return state ? `:is(${state.selector})` : token;
        });
      };
      return items.map((a) => {
        try {
          const selector = a.anchor?.selector;
          if (inline() && selector?.startsWith("#q-")) {
            const element = document.getElementById(selector.slice(1));
            if (!element) document.createDocumentFragment().querySelector(selector);
            return { element };
          }
          return { element: typeof selector === "string" ? document.querySelector(selectorWithFocus(selector)) : null };
        } catch { return { element: null, invalid: true }; }
      });
    } finally {
      for (const [el, parent, next] of positions.reverse()) parent.insertBefore(el, next);
      restoreFocus();
    }
  }
  // Returns a function that puts focus and selection back where they are now.
  function keepFocus() {
    const focusedHost = document.hasFocus() ? [...cards.values()].find((h) => h.shadowRoot.activeElement || (h.shadowRoot.contains(blurredCardControl) && !available(h))) : null;
    const focused = focusedHost?.shadowRoot.activeElement || (focusedHost ? blurredCardControl : document.activeElement);
    const oldSlotParent = focusedHost?.parentElement?.parentElement;
    const selection = focused && "selectionStart" in focused ? [focused.selectionStart, focused.selectionEnd, focused.selectionDirection] : null;
    return () => {
      if (focused?.isConnected && focused.getRootNode().activeElement !== focused) {
        focused.focus({ preventScroll: true });
        if (selection) focused.setSelectionRange(...selection);
      }
      if (focusedHost?.isConnected && oldSlotParent !== focusedHost.parentElement?.parentElement) focused.scrollIntoView({ block: "nearest" });
    };
  }
  function drawAttention() {
    const restoreFocus = keepFocus();
    const allItems = shownItems();
    const selfRendered = inline();
    const items = selfRendered ? [] : allItems;
    const keep = new Set(items.map((a) => a.id));
    for (const [id, host] of cards) if (!keep.has(id)) { slots.get(id)?.remove(); slots.delete(id); host.remove(); cards.delete(id); editing.delete(id); }
    if (!document.body) return;
    const anchors = attentionAnchors(allItems, restoreFocus);
    const positions = [...slots.values()].filter((slot) => slot.isConnected).map((slot) => [slot, slot.style.cssText]);
    let placements;
    try {
      // Keep focus and :focus-within live while taking cards out of row flow.
      for (const [slot] of positions) slot.style.setProperty("position", "absolute", "important");
      placements = allItems.map((a, i) => resolvePlacement(a, anchors[i]));
    } finally {
      for (const [slot, style] of positions) slot.style.cssText = style;
    }
    const groups = new Set(placements.map((p) => p.group || p.observe).filter(Boolean));
    for (const g of observedGroups) if (!groups.has(g)) groupObserver.unobserve(g);
    for (const g of groups) if (!observedGroups.has(g)) groupObserver.observe(g);
    observedGroups = groups;
    const dark = pageIsDark(), tails = new Map();
    items.forEach((a, i) => {
      let host = cards.get(a.id);
      if (!host) {
        host = document.createElement("div"); host.className = "jf-ask"; host.id = `jf-q-${a.id}`;
        host.attachShadow({ mode: "open" }); cards.set(a.id, host);
      }
      host.toggleAttribute("dark", dark);
      const sig = JSON.stringify([a.title, a.context, a.options, a.answer, editing.has(a.id)]);
      if (host.dataset.sig !== sig) { host.dataset.sig = sig; renderCard(host, a); }
      exposureLabel(host, a, false);
      insertSlot(slotFor(a.id, host, placements[i]), placements[i], tails);
    });
    restoreFocus();
    const failed = items.map((a, i) => placements[i].peers && !rowFits(placements[i], slots.get(a.id)));
    if (failed.some(Boolean)) {
      failed.forEach((fail, i) => { if (fail) placements[i] = fallback(placements[i].group); });
      const settledTails = new Map();
      items.forEach((a, i) => insertSlot(slotFor(a.id, cards.get(a.id), placements[i]), placements[i], settledTails));
    }
    restoreFocus();
    const rescued = items.map((a, i) => placements[i].status === "hidden" || !available(cards.get(a.id), true));
    if (rescued.some(Boolean)) {
      const visibleTails = new Map();
      items.forEach((a, i) => {
        if (rescued[i]) placements[i] = { kind: "block", after: null, status: placements[i].status, reason: placements[i].reason };
        insertSlot(slotFor(a.id, cards.get(a.id), placements[i]), placements[i], visibleTails);
      });
    }
    items.forEach((a, i) => exposureLabel(cards.get(a.id), a, rescued[i]));
    restoreFocus();
    outcomes = new Map((refreshingAttention ? [] : allItems).map((a, i) => {
      const p = placements[i];
      const status = selfRendered && !["invalid", "missing", "floating", "hidden"].includes(p.status) ? "unchecked" : p.status || "direct";
      return [a.id, { status, destination: p.after ? { kind: "after", selector: selectorFor(p.after) } : { kind: "top" }, reason: p.reason }];
    }));
    if (topStack && !topStack.children.length) topStack.remove();
    postSnapshot();
  }
  let drawQueued = false;
  const queueDraw = () => { if (!drawQueued) { drawQueued = true; requestAnimationFrame(() => { drawQueued = false; drawMarkers(); }); } };
  new MutationObserver((ms) => {
    if (ms.some((m) => !isOurs(m.target.nodeType === 1 ? m.target : m.target.parentElement) && (m.type !== "childList" || ![...m.addedNodes, ...m.removedNodes].every((n) => n.nodeType === 1 && isOurs(n))))) queueDraw();
  }).observe(document.documentElement, { childList: true, subtree: true, attributes: true, characterData: true });
  for (const type of ["resize", "transitionend", "animationend"]) addEventListener(type, queueDraw);

  let opening = 0, openingMessage = null;
  function showOpeningMessage(text) {
    openingMessage?.remove();
    openingMessage = document.createElement("div");
    openingMessage.className = "jf-ask";
    openingMessage.setAttribute("role", "alert");
    openingMessage.tabIndex = -1;
    openingMessage.textContent = text;
    openingMessage.style.cssText = "display:block;padding:16px;background:#fff;color:#1b2221;border:2px solid #b26a12;font:16px system-ui";
    document.body.prepend(openingMessage);
    openingMessage.scrollIntoView({ block: "center" });
    openingMessage.focus();
  }
  async function openAttention(id) {
    const request = ++opening;
    let item = attention.find((a) => a.id === id);
    if (!item) {
      try {
        await refreshThreads();
      } catch (e) {
        if (request === opening) showOpeningMessage("Could not load this question. Open it from the dock to try again.");
        console.error(e);
        return;
      }
      if (request !== opening) return;
      item = attention.find((a) => a.id === id);
    }
    openingMessage?.remove();
    if (!item || item.withdrawn) {
      showOpeningMessage("This question is no longer waiting for an answer.");
      return;
    }
    for (const fn of attendSubs) { try { fn(item); } catch (e) { console.error(e); } }
    drawAttention();
    const host = inline() ? anchorElement(item) : cards.get(id);
    if (!host) return;
    const root = host.shadowRoot || host;
    let control = root.querySelector("button.o:not(:disabled), [data-pick]") || root.querySelector("textarea:not(:disabled)") || root.querySelector("button:not(:disabled)") || root.querySelector(".c") || host;
    host.scrollIntoView({ block: inline() ? "start" : "center" });
    if (inline()) {
      const r = control.getBoundingClientRect();
      if (r.top < 0 || r.bottom > innerHeight || r.left < 0 || r.right > innerWidth) {
        control = root.querySelector("h1, h2, h3") || host;
        control.tabIndex = -1;
      }
    }
    if (!control.hasAttribute("tabindex") && control === host) control.tabIndex = -1;
    control.focus({ preventScroll: true });
    if (!inline()) control.scrollIntoView({ block: "nearest" });
    const c = root.querySelector(".c");
    if (c) { c.classList.remove("flash"); void c.offsetWidth; c.classList.add("flash"); }
  }
  addEventListener("message", (e) => {
    if (e.origin === location.origin && e.data?.type === "jelly:attend") openAttention(e.data.id).catch(console.error);
  });

  // A page that shows its own questions (the ask page) opts out:
  // <meta name="jelly-attention" content="inline">.
  const inline = () => document.querySelector('meta[name="jelly-attention"]')?.content === "inline";
  let workEl = null;
  function drawWorking() {
    workEl?.remove();
    workEl = null;
    if (!working || !document.body) return;
    workEl = document.createElement("div");
    workEl.className = "jf-work";
    workEl.innerHTML = "<i></i><span></span>";
    workEl.querySelector("span").textContent = `${working.by} is working on this${working.what ? ": " + working.what : ""}`;
    document.body.appendChild(workEl);
  }

  // ---- agent view: what is on screen, as text, for `jelly look` ---------------
  // A `display: contents` element (SvelteKit's app root) has no box of its own,
  // so the walker passes through it to its children instead of skipping it.
  const LEAF = new Set(["button", "a", "input", "textarea", "select", "img", "canvas", "svg", "iframe", "video", "p", "li", "pre", "h1", "h2", "h3", "h4", "h5", "h6", "label", "td", "th", "figcaption", "blockquote", "summary"]);
  const GROUP = new Set(["section", "article", "nav", "header", "footer", "main", "aside", "form", "table", "ul", "ol", "dialog", "details", "figure"]);
  const ACTIVE = "button, a, input, textarea, select, [role=button], img, canvas, svg, video, iframe";
  const ownText = (el) => [...el.childNodes].filter((n) => n.nodeType === 3).map((n) => n.textContent).join(" ").trim().replace(/\s+/g, " ").slice(0, 120);
  function agentSnapshot() {
    const lines = [];
    const label = (el) => (el.getAttribute("aria-label") || el.innerText || el.value || el.alt || el.title || "").trim().replace(/\s+/g, " ").slice(0, 120);
    const shown = (el) => { const r = el.getBoundingClientRect(); const cs = getComputedStyle(el); return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none" && cs.opacity !== "0"; };
    const add = (depth, text, el) => lines.push(`${"  ".repeat(depth)}- ${text}${el.id ? ` #${el.id}` : ""}`);
    (function walk(el, depth) {
      for (const c of el.children) {
        if (lines.length > 400 || c.matches(OURS) || ["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "LINK", "META"].includes(c.tagName) || (getComputedStyle(c).display === "contents" ? (walk(c, depth), true) : !shown(c))) continue;
        const tag = c.tagName.toLowerCase(), role = c.getAttribute("role");
        if (/^h[1-6]$/.test(tag)) add(depth, `heading "${label(c)}" [${tag}]`, c);
        else if (tag === "button" || role === "button") add(depth, `button "${label(c)}"${c.disabled ? " (disabled)" : ""}`, c);
        else if (tag === "a") add(depth, `link "${label(c)}"`, c);
        else if (tag === "input" && (c.type === "checkbox" || c.type === "radio")) add(depth, `${c.type} "${label(c.closest("label") || c)}" ${c.checked ? "checked" : "unchecked"}`, c);
        else if (tag === "input" || tag === "textarea") add(depth, `${tag === "textarea" || !c.type || c.type === "text" ? "textbox" : c.type}${c.placeholder ? ` "${c.placeholder}"` : ""} = "${String(c.value).slice(0, 80)}"`, c);
        else if (tag === "select") add(depth, `select = "${c.options[c.selectedIndex]?.text || ""}"`, c);
        else if (tag === "img") add(depth, `img "${c.alt || ""}"`, c);
        else if (["canvas", "svg", "video", "iframe"].includes(tag)) { const r = c.getBoundingClientRect(); add(depth, `${tag} ${Math.round(r.width)}×${Math.round(r.height)}${c.title ? ` "${c.title}"` : ""}${tag === "svg" && label(c) ? ` "${label(c).slice(0, 60)}"` : ""}`, c); }
        else if (LEAF.has(tag) || (ownText(c) && !GROUP.has(tag) && !role)) {
          // One line of text stays whole, so `<span>p50 <b>6.5</b></span>` keeps its label.
          // When a control or an id sits inside (`<p>Then <button>Go</button></p>`), or the
          // text spans lines, the outline also walks inside.
          const one = LEAF.has(tag) || !c.innerText.trim().includes("\n"), inside = c.querySelector(`${ACTIVE}, [id]`);
          const t = inside && !ownText(c) ? "" : one ? label(c) : ownText(c);
          if (t) add(depth, `text "${t}"`, c);
          if (!one || inside) walk(c, depth + (t ? 1 : 0));
        }
        else if (c.id && c.children.length === 0) { const t = label(c); if (t) add(depth, `text "${t}"`, c); }
        else if ((GROUP.has(tag) || role || c.id) && c.innerText?.trim()) { add(depth, `${role || tag}${c.getAttribute("aria-label") ? ` "${c.getAttribute("aria-label")}"` : ""}:`, c); const own = ownText(c); if (own) add(depth + 1, `text "${own}"`, {}); walk(c, depth + 1); continue; }
        else if (c.children.length === 0) { const t = label(c); if (t) add(depth, `text "${t}"`, c); }
        else walk(c, depth);
      }
    })(document.body, 0);
    return lines.join("\n");
  }
  // ---- problems the browser measures, for `jelly look` ---------------------------
  const CONTROL = "button, a[href], input:not([type=hidden]), textarea, select, [role=button]";
  // The shortest selector an agent can paste into `jelly try` or `--at`: a unique
  // id, test id, href, name, label or class before falling back to the element's path.
  function shortSelector(el) {
    const tag = el.tagName.toLowerCase(), q = (v) => JSON.stringify(v), unique = (s) => { try { return document.querySelectorAll(s).length === 1; } catch { return false; } };
    const cls = [...el.classList].find((c) => !c.startsWith("jf-"));
    const tries = [el.id && "#" + CSS.escape(el.id), el.dataset?.testid && `[data-testid=${q(el.dataset.testid)}]`,
      tag === "a" && el.getAttribute("href") && `a[href=${q(el.getAttribute("href"))}]`, el.getAttribute("name") && `${tag}[name=${q(el.getAttribute("name"))}]`,
      el.getAttribute("aria-label") && `${tag}[aria-label=${q(el.getAttribute("aria-label"))}]`,
      el.type === "submit" && `${tag}[type=submit]`, cls && `${tag}.${CSS.escape(cls)}`];
    for (const s of tries) if (s && unique(s)) return s;
    // Unique within an id'd ancestor or a landmark, nearest first, when that is enough.
    const SCOPE = "[id], header, nav, main, footer, form, aside";
    for (let scope = el.parentElement?.closest(SCOPE); scope && scope !== document.body; scope = scope.parentElement?.closest(SCOPE)) {
      const prefix = scope.id ? "#" + CSS.escape(scope.id) : scope.tagName.toLowerCase();
      for (const s of tries.slice(2)) if (s && unique(`${prefix} ${s}`)) return `${prefix} ${s}`;
    }
    const path = selectorFor(el);
    return path.startsWith("#") ? path : `body > ${path}`;
  }
  // Any CSS color as [r, g, b, a] (0-255, alpha 0-1): a canvas reads them all,
  // including oklch() and color-mix().
  let paint;
  const colors = new Map();
  function rgba(value) {
    if (!colors.has(value)) {
      paint ??= new OffscreenCanvas(1, 1).getContext("2d", { willReadFrequently: true });
      paint.clearRect(0, 0, 1, 1);
      paint.fillStyle = "#0000";
      paint.fillStyle = value;
      paint.fillRect(0, 0, 1, 1);
      const [r, g, b, a] = paint.getImageData(0, 0, 1, 1).data;
      colors.set(value, [r, g, b, a / 255]);
    }
    return colors.get(value);
  }
  const over = (top, under) => [0, 1, 2].map((i) => top[i] * top[3] + under[i] * (1 - top[3])).concat(1);
  const luminance = (c) => { const [r, g, b] = c.slice(0, 3).map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }); return 0.2126 * r + 0.7152 * g + 0.0722 * b; };
  const hex = (c) => "#" + c.slice(0, 3).map((v) => Math.round(v).toString(16).padStart(2, "0")).join("");
  // The page's own background when nothing paints one: Chrome's dark canvas when
  // the page opts into dark and the browser prefers it, white otherwise.
  function canvasColor() {
    const scheme = `${getComputedStyle(document.documentElement).colorScheme} ${document.querySelector('meta[name="color-scheme"]')?.content || ""}`;
    const dark = /dark/.test(scheme) && (matchMedia("(prefers-color-scheme: dark)").matches || !/light/.test(scheme));
    return dark ? [18, 18, 18, 1] : [255, 255, 255, 1];
  }
  // What a text element's ancestors paint behind it, or null under an image or
  // gradient, which the check cannot read.
  function backdrop(el, base) {
    const layers = [];
    for (let n = el; n; n = n.parentElement) {
      const cs = getComputedStyle(n);
      if (cs.backgroundImage !== "none") return null;
      const c = rgba(cs.backgroundColor);
      if (c[3] > 0) { layers.push(c); if (c[3] >= 1) break; }
    }
    return layers.reduceRight((under, top) => over(top, under), base);
  }
  // Left out on purpose by apps, so never a problem: screen-reader-only text (a
  // 1px box), aria-hidden parts, and controls kept out of the tab order (honeypots).
  function measurable(el) {
    const r = el.getBoundingClientRect();
    return r.width > 2 && r.height > 2 && el.checkVisibility({ opacityProperty: true, visibilityProperty: true })
      && !el.closest("[aria-hidden=true], [inert]") && !(el.matches(CONTROL) && el.tabIndex < 0) && !isOurs(el);
  }
  const named = (el) => `"${(el.getAttribute("aria-label") || el.innerText || el.value || el.placeholder || el.alt || "").trim().replace(/\s+/g, " ").slice(0, 30)}"`;
  // Inside a box a person can scroll sideways, what is past the screen is reachable.
  const scrolls = (el) => { for (let n = el.parentElement; n && n !== document.body; n = n.parentElement) if (getComputedStyle(n).overflowX !== "visible") return true; return false; };
  // A control under a fixed or sticky bar is fine when scrolling moves it clear
  // of the bar while it stays on the screen.
  function scrollsClear(r, cover) {
    let bar = cover;
    while (bar && !["fixed", "sticky"].includes(getComputedStyle(bar).position)) bar = bar.parentElement;
    if (!bar) return false;
    const b = bar.getBoundingClientRect(), down = document.documentElement.scrollHeight - innerHeight - scrollY;
    return r.bottom - b.top <= Math.min(down, r.top) || b.bottom - r.top <= Math.min(scrollY, innerHeight - r.bottom);
  }
  // Text that does not reach WCAG AA contrast (4.5:1, or 3:1 for large text)
  // against what is painted behind it.
  function contrast() {
    const base = canvasColor(), found = [];
    for (const el of document.body.querySelectorAll("*")) {
      if (found.length >= 100) break;
      if (!ownText(el) || ["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "OPTION"].includes(el.tagName) || !measurable(el)) continue;
      // Disabled controls are exempt in WCAG.
      if (el.closest(":disabled, [aria-disabled=true]")) continue;
      // Symbols alone (●●●○○, ▲, →) are graphics, such as a meter's empty slots,
      // not text someone reads; the text contrast rule does not fit them.
      if (!/[\p{L}\p{N}]/u.test(ownText(el))) continue;
      const cs = getComputedStyle(el);
      const fg = rgba(cs.webkitTextFillColor || cs.color);
      if (fg[3] === 0) continue;
      const bg = backdrop(el, base);
      if (!bg) continue;
      let alpha = fg[3];
      for (let n = el; n; n = n.parentElement) alpha *= Number(getComputedStyle(n).opacity);
      const text = over([...fg.slice(0, 3), alpha], bg);
      const [hi, lo] = [luminance(text), luminance(bg)].sort((a, b) => b - a);
      const ratio = (hi + 0.05) / (lo + 0.05), size = parseFloat(cs.fontSize);
      const large = size >= 24 || (size >= 18.66 && Number(cs.fontWeight) >= 700);
      if (ratio < (large ? 3 : 4.5)) found.push({ at: shortSelector(el), text: ownText(el).slice(0, 30), fg: hex(text), bg: hex(bg), ratio: Math.floor(ratio * 10) / 10 });
    }
    return found;
  }
  // Layout, control and loading problems, by kind.
  function problems() {
    const W = innerWidth, out = [], add = (kind, items) => { if (items.length) out.push({ kind, items: items.slice(0, 100) }); };
    const all = [...document.body.querySelectorAll("*")].filter((e) => !isOurs(e));
    const sw = document.documentElement.scrollWidth;
    if (sw > W + 1) {
      const wide = all.filter((e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.right > W + 1 && getComputedStyle(e).position !== "fixed"; });
      add(`wider than the screen (${sw}px at ${W}px)`, wide.filter((e) => !wide.includes(e.parentElement)).map((e) => `${shortSelector(e)} reaches ${Math.round(e.getBoundingClientRect().right)}px`));
    }
    const controls = [...document.querySelectorAll(CONTROL)].filter(measurable);
    const rects = new Map(controls.map((c) => [c, c.getBoundingClientRect()]));
    const off = [], covered = [], small = [], unlabeled = [];
    // Under 24px is fine when nothing else a person could tap is within 12px of its
    // middle: WCAG 2.5.8's spacing exception.
    const undersized = (r) => r.width < 24 || r.height < 24;
    const spaced = (c, r) => controls.every((o) => {
      if (o === c || o.contains(c) || c.contains(o)) return true;
      const p = rects.get(o), x = r.left + r.width / 2, y = r.top + r.height / 2;
      if (undersized(p)) return Math.hypot(p.left + p.width / 2 - x, p.top + p.height / 2 - y) >= 24;
      return Math.hypot(Math.max(p.left - x, 0, x - p.right), Math.max(p.top - y, 0, y - p.bottom)) >= 12;
    });
    // Scrolled out of view inside a box that scrolls (a sideways tab bar): reachable
    // by scrolling that box, so not covered by whatever sits at that spot.
    const clipped = (c, r) => {
      const x = r.left + r.width / 2, y = r.top + r.height / 2;
      for (let a = c.parentElement; a && a !== document.body; a = a.parentElement) {
        const cs = getComputedStyle(a);
        if (!/auto|scroll|hidden/.test(cs.overflowX + cs.overflowY)) continue;
        const b = a.getBoundingClientRect();
        if (x < b.left || x > b.right || y < b.top || y > b.bottom) return true;
      }
      return false;
    };
    for (const c of controls) {
      const r = rects.get(c), name = `${c.tagName.toLowerCase()} ${named(c)} ${shortSelector(c)}`;
      if ((r.right > W + 1 || r.left < -1) && !scrolls(c)) { off.push(name); continue; }
      const p = r.top >= 0 && r.bottom <= innerHeight ? document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) : null;
      if (p && !c.contains(p) && !p.contains(c) && !(c.labels && [...c.labels].some((l) => l.contains(p))) && !scrollsClear(r, p) && !clipped(c, r)) {
        covered.push(`${name} under ${p.closest(".jf-btn") ? "jelly's Comment button (move it with --jelly-comment-inset)" : shortSelector(p)}`);
      }
      // A labelled checkbox or radio is tapped through its label; a link inside text is exempt.
      const viaLabel = c.matches("input[type=checkbox], input[type=radio]") && c.labels?.length;
      if (W < 600 && !viaLabel && getComputedStyle(c).display !== "inline" && undersized(r) && !spaced(c, r)) small.push(`${name} ${Math.round(r.width)}×${Math.round(r.height)}`);
      if (c.matches("input:not([type=submit]):not([type=button]):not([type=reset]):not([type=image]), select, textarea")
        && !c.labels?.length && !c.getAttribute("aria-label") && !c.getAttribute("aria-labelledby") && !c.title && !c.placeholder) unlabeled.push(name);
    }
    add("off the screen", off);
    add("covered by another element", covered);
    add("too small to tap (under 24px)", small);
    add("field without a label", unlabeled);
    add("image without alt", [...document.images].filter(measurable).filter((i) => !i.hasAttribute("alt") && !["presentation", "none"].includes(i.getAttribute("role"))).map((i) => `${shortSelector(i)} ${(i.currentSrc || i.src).split("/").pop().slice(0, 40)}`));
    add("text cut off", all.filter((e) => !e.children.length && e.textContent.trim() && measurable(e) && e.scrollWidth > e.clientWidth + 1
      && ["hidden", "clip"].includes(getComputedStyle(e).overflowX) && getComputedStyle(e).textOverflow !== "ellipsis").map((e) => `${shortSelector(e)} "${e.textContent.trim().slice(0, 30)}"`));
    // A load that failed already shows as an error line.
    add("failed request", performance.getEntriesByType("resource")
      .filter((r) => r.responseStatus >= 400 && ![`${location.origin}${screenUrl}`, `${location.origin}/favicon.ico`].includes(r.name) && !errors.includes(`could not load ${r.name}`))
      .map((r) => `${r.responseStatus} ${r.name.replace(location.origin, "")}`));
    return out;
  }
  // At most one post per interval, so a page that changes all the time still reports.
  function postSnapshot() {
    // A trial's page is not what `jelly look` should show; it reports in `runTrial`.
    if (snapT || trial || share) return;
    snapT = setTimeout(() => {
      snapT = null;
      fetch(screenUrl, { method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ owner: boot.snapshotOwner, text: agentSnapshot(), width: innerWidth, height: innerHeight, version, errors,
          // Only `jelly look`'s own renders measure problems; a person's browser does not pay for them.
          ...(agentMode && { dark: darkRender, contrast: contrast(), problems: darkRender ? [] : problems() }),
          inline: inline(), page_height: document.documentElement.scrollHeight, below: anchorsThat("direct"), invalid: anchorsThat("invalid"), missing: anchorsThat("missing"), floating: anchorsThat("floating"), hidden: anchorsThat("hidden"),
          exposed: [...cards].filter(([, h]) => h.hasAttribute("data-exposed") && available(h, true)).map(([id]) => id),
          fallback: attention.filter(isOpen).filter((a) => outcomes.get(a.id)?.status === "fallback").map((a) => ({ id: a.id, destination: outcomes.get(a.id).destination, reason: outcomes.get(a.id).reason })) }) }).then((r) => { if (r.status === 409) reload(); }).catch(() => {});
    }, agentMode ? 400 : 1200);
  }
  function anchorsThat(kind) {
    return attention.filter((a) => isOpen(a) && typeof a.anchor?.selector === "string" && outcomes.get(a.id)?.status === kind).map((a) => `${a.id} ${a.anchor.selector}`);
  }
  addEventListener("load", postSnapshot);
  subs.push(() => postSnapshot());

  // ---- jelly try: do the steps, then report what the page shows ----
  let trialCalls = 0;
  async function trialCall(verb, args) {
    trialCalls++;
    try {
      const r = await fetch(`/api/tries/${trial.token}/call`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ verb, args, by: me() }) });
      const j = await r.json();
      if (!r.ok) throw new Error(j.error || r.statusText);
      return j;
    } finally { trialCalls--; }
  }
  const pause = (ms) => new Promise((r) => setTimeout(r, ms));
  // Does one step the way a person would; returns why it could not be done.
  function doStep(s) {
    let el;
    try { el = document.querySelector(s.at); } catch { return `${s.at} is not a valid CSS selector`; }
    if (!el) return `${s.at} matches nothing`;
    if (el.checkVisibility && !el.checkVisibility()) return `${s.at} is not visible`;
    if (el.disabled) return `${s.at} is disabled`;
    el.scrollIntoView?.({ block: "center" });
    el.focus?.();
    if (s.do === "click") {
      const o = { bubbles: true, cancelable: true, composed: true, view: window };
      el.dispatchEvent(new PointerEvent("pointerdown", o)); el.dispatchEvent(new MouseEvent("mousedown", o));
      el.dispatchEvent(new PointerEvent("pointerup", o)); el.dispatchEvent(new MouseEvent("mouseup", o));
      // `click()` also does what the browser does: toggles a checkbox, submits a form.
      if (typeof el.click === "function") el.click(); else el.dispatchEvent(new MouseEvent("click", o));
    } else if (s.do === "type") {
      if (el.isContentEditable) el.textContent = s.text;
      else if ("value" in el) {
        // The prototype's setter, so frameworks that track the value see the change.
        const set = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(el), "value")?.set;
        if (set) set.call(el, s.text); else el.value = s.text;
      } else return `${s.at} is not a field`;
      el.dispatchEvent(new InputEvent("input", { bubbles: true, composed: true, data: s.text, inputType: "insertText" }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    } else if (s.do === "press") {
      const o = { key: s.key, bubbles: true, cancelable: true, composed: true };
      const go = el.dispatchEvent(new KeyboardEvent("keydown", o));
      el.dispatchEvent(new KeyboardEvent("keyup", o));
      // Enter in a form field submits its form, as it does for a person.
      if (go && s.key === "Enter" && el.form && el.matches("input")) el.form.requestSubmit();
    }
  }
  async function runTrial() {
    let failed = null;
    for (const [i, s] of trial.steps.entries()) {
      let error;
      try { error = doStep(s); } catch (e) { error = String(e?.message || e); }
      if (error) { failed = { step: i, error }; break; }
      // Let handlers start their calls, wait for the calls, then let the view redraw.
      do { await pause(30); } while (trialCalls);
    }
    await pause(30);
    await fetch(`/api/tries/${trial.token}`, { method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ text: agentSnapshot(), width: innerWidth, height: innerHeight, errors, failed }) }).catch(() => {});
  }
  if (trial) addEventListener("load", () => setTimeout(runTrial, 300));
  // Text a page fills in later (fetches, timers) counts too.
  new MutationObserver(() => postSnapshot()).observe(document.documentElement, { childList: true, subtree: true, characterData: true });

  let refreshingAttention = false;
  let latestRefresh;
  function refreshThreads() {
    refreshingAttention = true;
    outcomes.clear();
    const refresh = (async () => {
      let j;
      try {
        if (share) j = await viaShare("show");
        else {
          const r = await fetch(api);
          if (!r.ok) throw new Error(`Could not refresh questions (${r.status})`);
          j = await r.json();
        }
      } catch (e) {
        if (refresh !== latestRefresh) return latestRefresh;
        throw e;
      }
      if (refresh !== latestRefresh) return latestRefresh;
      refreshingAttention = false;
      comments = j.comments || []; attention = j.attention || []; working = j.working || null;
      drawMarkers(); drawWorking();
      for (const fn of attentionSubs) { try { fn(); } catch (e) { console.error(e); } }
    })();
    latestRefresh = refresh;
    return refresh;
  }

  addEventListener("DOMContentLoaded", () => {
    const style = document.createElement("style");
    style.textContent = css;
    document.head.appendChild(style);
    // A view that draws its own comment control opts out with <meta name="jelly-comment" content="off">.
    const optOut = document.querySelector('meta[name="jelly-comment"]')?.content === "off";
    if (!embedded && !optOut && share?.can_act !== false) document.body.appendChild(btn);
    drawMarkers();
    drawWorking();
    new ResizeObserver(queueDraw).observe(document.body);
    addEventListener("resize", queueDraw);
  });
  subs.push(() => setTimeout(drawMarkers, 0));
  btn.onclick = (e) => { e.stopPropagation(); setMode(!commenting); };
  // Esc that ends comment mode is spent: the view never sees it, so it does not also
  // close what the person was commenting on. Window capture runs before any view listener.
  addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || !commenting || e.target.closest?.(".jf-pop")) return;
    e.stopPropagation();
    setMode(false);
  }, true);
  addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    // Esc inside a jelly closes the comment mode first, then reaches the jelly page.
    const inPop = e.target.closest?.(".jf-pop");
    if (embedded && !commenting && !inPop && !pop?.isConnected) parent.postMessage({ type: "jelly:escape", name: boot.name }, location.origin);
    setMode(false);
  });
  addEventListener("mouseover", (e) => {
    if (!(commenting || e.altKey) || isOurs(e.target)) return;
    hovered?.classList.remove("jf-hover");
    hovered = e.target;
    hovered.classList.add("jf-hover");
  });
  addEventListener("click", (e) => {
    if (!(commenting || e.altKey) || isOurs(e.target)) return;
    e.preventDefault();
    e.stopPropagation();
    e.target.classList.remove("jf-hover");
    setMode(false);
    startComment(e.target, e.clientX, e.clientY);
  }, true);
})();
