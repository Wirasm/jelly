// Injected into every jelly view as `window.jelly`, grouped by what it is about.
//
// This jelly:
//   jelly.state                 current state (read only; change it with verbs)
//   jelly.onState(fn)           fn(state, event) now and after every change, from anyone
//   jelly.do(verb, args)      run a verb; resolves to the event, rejects with the reason
//   jelly.asset(file)           URL of a file added with `jelly asset` (or just use "assets/<file>")
//   jelly.events(fn)            fn(event) for every event of every jelly; returns unsubscribe
//
// Other jellies ("plan" means plan in this project):
//   jelly.other(name).state     a promise of its state
//   jelly.other(name).onState(fn) fn(state, event) now and after its changes; returns unsubscribe
//   jelly.other(name).open()    put it on screen: full size on the jelly page, else navigate
//
// The person:
//   jelly.person.name           this browser's name, "" if unset; assign to rename (kept per browser)
//   jelly.person.by             what this browser writes as: "person", or "person:<name>" once named
//   jelly.person.asks           every ask on this jelly, answered and unasked ones too;
//                               open ones have no .answer and no .unasked; .answer.option is the pick
//   jelly.person.onAsk(fn)      fn() now and whenever asks or comments change
//   jelly.person.onOpen(fn)     fn(ask) when the person opens an ask from the dock; returns unsubscribe
//   jelly.person.comments       comment threads on this jelly
//   jelly.person.answer(id, { option, text }), .comment(anchor, text), .reply(id, text), .resolve(id)
//
//   jelly.who(by)               "you" for this browser, a person's name, or the agent's name
//   jelly.isPerson(by)          true for "person" and "person:<name>"
//
// Asks: an ask you do not draw shows as a card in jelly's stack of questions,
// fixed over the bottom of the page, with a pin on the element it is about.
// To draw one yourself, put `data-ask="<ask id>"` on its element and answer
// with jelly.person.answer; jelly then leaves that ask to you.
//
// Comments anywhere: hold Option (Alt) and click any element, or press the
// Comment button, to pin a comment to it. The agent gets a `comment` event that
// names the element. Give elements an `id` so comments survive a shape.
// Agents answer on the pin (`jelly reply`); the thread shows when you click it.
//
// On a share link (`jelly share --to`) the same runtime runs, with a few
// differences: jelly.other() is not available, jelly.person.name is whom the
// link is for and cannot be changed, and a view-only link refuses changes.
//
// A shape or an undo that changes the view reloads the page. State lives in
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
  // Nobody is at an agent render to close a dialog, and an open one stops the
  // page until the render times out. Each is reported instead, and answered the
  // way that lets the page go on: OK to a confirm, Cancel to a prompt.
  const dialogs = [];
  if (agentMode) {
    const dialog = (kind, answer, said) => (text = "") => {
      if (dialogs.length < 10) dialogs.push(`${kind} ${JSON.stringify(String(text).slice(0, 200))}${said}`);
      postSnapshot();
      return answer;
    };
    window.alert = dialog("alert", undefined, "");
    window.confirm = dialog("confirm", true, ", answered OK");
    window.prompt = dialog("prompt", null, ", cancelled");
  }
  const noteError = (msg) => { if (errors.length < 10 && msg && !errors.includes(msg)) { errors.push(String(msg).slice(0, 300)); postSnapshot(); } };
  // Someone at the page sees what failed too: a button that throws otherwise
  // looks like a button that does nothing. Not for a file that did not load.
  let failEl = null, failT = null;
  function showFailure(msg) {
    if (agentMode || !msg) return;
    failEl ??= Object.assign(document.createElement("button"), { className: "jf-fail", title: "Close", onclick: () => failEl.remove() });
    failEl.textContent = `This did not work: ${String(msg).slice(0, 200)}`;
    ui().appendChild(failEl);
    clearTimeout(failT);
    failT = setTimeout(() => failEl.remove(), 12000);
  }
  addEventListener("error", (e) => { if (e.error || e.message) showFailure(e.error?.message || e.message); });
  addEventListener("unhandledrejection", (e) => showFailure(e.reason?.message || e.reason));
  addEventListener("error", (e) => noteError(e.error?.stack?.split("\n").slice(0, 2).join(" ") || e.message || (e.target?.src || e.target?.href ? `could not load ${e.target.src || e.target.href}` : "")), true);
  addEventListener("unhandledrejection", (e) => noteError(`unhandled rejection: ${e.reason?.message || e.reason}`));
  const consoleError = console.error.bind(console);
  console.error = (...a) => { noteError(a.map((x) => x?.stack || x?.message || String(x)).join(" ")); consoleError(...a); };
  let state = boot.state;
  let version = boot.version;
  let comments = boot.comments || [];
  let asks = boot.asks || [];
  const askSubs = [];
  const openSubs = new Set();
  let working = boot.working || null;
  // What people did here since someone last pressed Send.
  let unsent = boot.unsent || 0;
  const unsentSubs = [];
  const FOR_AGENT = ["do", "comment", "reply", "resolve", "undo", "answer"];
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
      if (el?.nodeType === 1 && !isOurs(el)) lastUsed = { el, at: Date.now() };
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
    const r = await fetch(`${api}/${op === "answer" ? `asks/${enc(body.id)}/answer` : op}`, {
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
    // The daemon tells an agent whether anyone has this jelly on screen, and
    // whether this browser may show a notification when nobody does.
    const tell = () => { if (ws.readyState === 1 && !agentMode && !trial && !boot.past) ws.send(JSON.stringify({ open: boot.name, visible: document.visibilityState === "visible", notifies: typeof Notification !== "undefined" && Notification.permission === "granted" })); };
    addEventListener("visibilitychange", tell);
    ws.onopen = () => { if (dropped) reload(); tell(); };
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
    if (ev.kind === "make" || ev.view_rev !== boot.viewRev) { reload(); return; }
    const was = unsent;
    if (ev.kind === "send") unsent = 0;
    else if (isPerson(ev.by) && FOR_AGENT.includes(ev.kind)) unsent++;
    if (unsent !== was) { drawSend(); unsentSubs.forEach((fn) => fn(unsent)); }
    if (ev.kind === "comment" || ev.kind === "resolve" || ev.kind === "reply") refreshThreads().catch(console.error);
    if (ev.kind === "ask" || ev.kind === "answer" || ev.kind === "unask") refreshThreads().catch(console.error);

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
    /** jelly's own stylesheet, for a view's shadow roots: `<style>${jelly.css}</style>`. */
    get css() { return JELLY_CSS; },
    get state() { return state; },
    onState(fn) { subs.push(fn); fn(state, null); },
    async do(verb, args = {}) {
      const ev = trial ? await trialCall(verb, args) : await post("do", { verb, args, source: source() });
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
        onState(fn) {
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
      get asks() { return asks; },
      onAsk(fn) { askSubs.push(fn); fn(); },
      onOpen(fn) { openSubs.add(fn); return () => openSubs.delete(fn); },
      get comments() { return comments; },
      /** An option id, text, or both. A seen ask takes nothing ("seen"). */
      answer: (id, { option, text } = {}) => post("answer", { id, option, text: text || "" }),
      comment: (anchor, text) => post("comment", { anchor, text }),
      reply: (id, text) => post("reply", { id, text }),
      resolve: (id) => post("resolve", { id }),
      /** How many things people did here since the last send. */
      get unsent() { return unsent; },
      onUnsent(fn) { unsentSubs.push(fn); fn(unsent); },
      /** "I am done here for now": wakes an agent that waits on this jelly. */
      send: () => post("send", {}),
    },
    who,
    isPerson,
  };

  // ---- comments: markers on the elements they are about ------------------------

  // Views place Comment with --jelly-comment-inset on :root.
  // jelly's own look (assets/jelly.css), put here by the daemon. Every control
  // below is styled from its tokens, so jelly's parts look the same everywhere.
  const JELLY_CSS = "__JELLY_CSS__";
  const css = `
    jelly-ui { font-family: var(--jy-font); }
    .jf-btn { position: fixed; inset: var(--jelly-comment-inset, auto 10px 10px auto); z-index: 2147483000; font: 12px var(--jy-font);
      background: var(--jy-card); color: var(--jy-ink); border: 1px solid var(--jy-line-2); border-radius: 999px; padding: 5px 11px 5px 8px; cursor: pointer;
      display: inline-flex; align-items: center; gap: 6px; box-shadow: 0 6px 18px -8px var(--jy-shadow); }
    .jf-btn::before { content: ""; width: 15px; height: 15px; background: var(--jy-blob) center / contain no-repeat; }
    .jf-btn:hover { background: var(--jy-brand-soft); }
    .jf-btn.on { background: var(--jy-sun); color: var(--jy-sun-ink); border-color: var(--jy-sun); }
    .jf-btn.jf-send { inset: var(--jelly-send-inset, auto auto 12px 10px); background: var(--jy-action); color: var(--jy-action-ink); border-color: var(--jy-action); font-weight: 600;
      padding: 6px 13px 6px 9px; box-shadow: 0 3px 0 var(--jy-action-edge); }
    .jf-btn.jf-send:active { transform: translateY(2px); box-shadow: 0 1px 0 var(--jy-action-edge); }
    .jf-hover { outline: 2px dashed #e8336f !important; outline-offset: 2px; cursor: crosshair !important; }
    .jf-marker { position: absolute; z-index: 2147482999; width: 20px; height: 20px; border-radius: 50% 50% 0 50%; background: var(--jy-sun); color: var(--jy-sun-ink);
      font: 600 11px var(--jy-font); display: grid; place-items: center; cursor: pointer; box-shadow: 0 2px 6px rgba(0,0,0,.3); }
    .jf-marker.agent { background: var(--jy-on); color: var(--jy-on-ink); }
    .jf-marker.ask { background: var(--jy-action); color: var(--jy-action-ink); border-radius: 50%; }
    .jf-marker.done { opacity: .55; }
    .jf-pop { position: absolute; z-index: 2147483001; width: 260px; background: var(--jy-card); color: var(--jy-ink); border: 1px solid var(--jy-line-2); border-radius: 12px;
      box-shadow: 0 18px 40px -18px var(--jy-shadow); padding: 8px; font: 12px var(--jy-font); }
    .jf-pop .jf-on { font: 10px var(--jy-mono); color: var(--jy-muted); margin-bottom: 5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .jf-pop textarea { width: 100%; box-sizing: border-box; min-height: 54px; font: 12px var(--jy-font); color: var(--jy-ink); background: var(--jy-page); border: 1px solid var(--jy-line-2); border-radius: 8px; padding: 5px; resize: vertical; }
    .jf-pop .jf-row { display: flex; gap: 6px; justify-content: flex-end; margin-top: 6px; align-items: center; }
    .jf-pop button { font: 12px var(--jy-font); color: var(--jy-ink); border: 1px solid var(--jy-line); background: var(--jy-card); border-radius: 7px; padding: 3px 9px; cursor: pointer; }
    .jf-pop button.jf-send { background: var(--jy-action); color: var(--jy-action-ink); border-color: var(--jy-action); font-weight: 600; }
    .jf-pop .jf-by { font: 10px var(--jy-mono); color: var(--jy-muted); }
    .jf-pop .jf-thread { display: grid; gap: 6px; margin-bottom: 6px; max-height: 260px; overflow-y: auto; }
    .jf-pop .jf-msg { border-left: 2px solid var(--jy-sun); padding: 1px 0 1px 7px; }
    .jf-pop .jf-msg.agent { border-color: var(--jy-brand); }
    .jf-pop .jf-msg.agent .jf-by { color: var(--jy-brand-text); }
    .jf-pop .jf-text { white-space: pre-wrap; margin-top: 1px; }
    .jf-pop .jf-done { font-size: 11px; color: var(--jy-muted); }
    .jf-work { position: fixed; left: 50%; top: 10px; transform: translateX(-50%); z-index: 2147483000; background: var(--jy-on); color: var(--jy-on-ink); border-radius: 999px; padding: 6px 14px 6px 12px; font: 12.5px var(--jy-font); display: flex; gap: 8px; align-items: center; box-shadow: 0 8px 24px -8px var(--jy-shadow); }
    .jf-fail { position: fixed; left: 50%; bottom: 52px; transform: translateX(-50%); z-index: 2147483002; max-width: min(520px, calc(100vw - 24px)); text-align: left;
      background: var(--jy-card); color: var(--jy-ink); border: 1px solid var(--jy-bad); border-left-width: 4px; border-radius: 10px; padding: 8px 12px; font: 12.5px var(--jy-font);
      cursor: pointer; box-shadow: 0 8px 24px -8px var(--jy-shadow); overflow-wrap: anywhere; }
    .jf-work i { width: 8px; height: 8px; border-radius: 50%; background: var(--jy-brand); animation: jf-blink 1s infinite; } @keyframes jf-blink { 50% { opacity: .3; } }
    `;
  // In the page before it first paints, so a view that takes jelly's look
  // (<html class="jy">) never shows without it.
  const style = document.createElement("style");
  style.textContent = JELLY_CSS + css;
  document.head?.appendChild(style);
  let commenting = false, hovered = null, pop = null;
  const btn = document.createElement("button");
  btn.className = "jf-btn";
  btn.textContent = "Comment";
  btn.title = "Pin a comment to any part of this jelly (or hold Option and click)";

  // Everything the runtime adds to a view. Selectors name view elements only.
  // Everything jelly adds to a page lives in one element beside <body>, never
  // among the view's own elements: the view's selectors and layout see only the view.
  let layer = null;
  function ui() {
    if (!layer?.isConnected) {
      layer = document.createElement("jelly-ui");
      layer.className = "jy";
      scheme(layer);
      layer.style.cssText = "position:absolute;top:0;left:0;width:0;height:0";
      document.documentElement.appendChild(layer);
    }
    return layer;
  }
  const isOurs = (el) => el.closest?.("jelly-ui");
  function selectorFor(el) {
    const parts = [];
    for (let n = el; n && n.nodeType === 1 && n !== document.body; n = n.parentElement) {
      if (n.id) { parts.unshift("#" + CSS.escape(n.id)); break; }
      let i = 1;
      for (let s = n.previousElementSibling; s; s = s.previousElementSibling) if (s.tagName === n.tagName) i++;
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
    ui().appendChild(pop);
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
    if (layer) scheme(layer);
    drawAsks();
    markerEls.forEach((p) => p.remove());
    markerEls = [];
    const shown = visible();
    shown.forEach((f, i) => {
      const el = anchorOf(f).element;
      // A part that is not on screen now (another tab, a closed section) shows no pin.
      if (!el || !el.getClientRects().length) return;
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
      ui().appendChild(pin);
      markerEls.push(pin);
    });
  }
  // ---- asks: the view draws them, or they show in jelly's own stack ------------
  // A view that draws an ask itself marks an element `data-ask="<id>"` and
  // answers with jelly.person.answer. Every other ask gets a card in one stack
  // that jelly owns, fixed over the bottom of the page and never part of the
  // view's layout, and a pin on the element it is about. An answered ask stays
  // in the stack as one line for a day, with Change.
  const DAY_MS = 86400000;
  const cards = new Map(); // ask id -> its card in the stack, reused so drafts survive redraws
  const isOpen = (a) => !a.answer && !a.unasked;
  const shownItems = () => asks.filter((a) => !a.unasked && (!a.answer || Date.now() - a.answer.at_ms < DAY_MS));
  const editing = new Set(); // answered asks the person reopened with Change
  const drawnByView = (id) => document.querySelector(`[data-ask="${CSS.escape(id)}"]`);
  // The element an ask or a comment is about: `element` is null when the
  // selector matches nothing, `invalid` is set when it is not a selector.
  function anchorOf(item) {
    const selector = item.anchor?.selector;
    if (typeof selector !== "string") return {};
    try { return { element: document.querySelector(selector) }; } catch { return { invalid: true }; }
  }
  // Brings an element into view and outlines it for a moment.
  function point(el) {
    el.scrollIntoView({ block: "center" });
    el.classList.add("jf-hover");
    setTimeout(() => el.classList.remove("jf-hover"), 1600);
  }
  const CARD_CSS = `
    :host { all: initial; position: fixed; left: 0; right: 0; bottom: 0; z-index: 2147482998; display: flex; justify-content: center; pointer-events: none; }
    * { box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; letter-spacing: normal; text-transform: none; }
    .sheet { pointer-events: auto; width: min(640px, calc(100vw - 16px)); margin-bottom: 8px; display: grid; justify-items: center; gap: 6px;
      --bg: var(--jy-card); --ink: var(--jy-ink); --soft: var(--jy-muted); --line: var(--jy-line-2); --tint: var(--jy-soft); --accent: var(--jy-brand); --accent-ink: var(--jy-brand-text); }
    .tab { all: unset; cursor: pointer; font-size: 13px; font-weight: 600; padding: 6px 14px; border-radius: 999px; background: var(--ink); color: var(--bg); box-shadow: 0 6px 18px -6px rgba(0,0,0,.5); }
    .list { width: 100%; max-height: min(70vh, 560px); overflow-y: auto; display: grid; gap: 8px; padding: 2px; }
    .shut .list { display: none; }
    .c { background: var(--bg); color: var(--ink); border: 1.5px solid var(--line); border-left: 4px solid var(--accent); border-radius: 14px; padding: 16px 18px; font-size: 14px; line-height: 1.5; text-align: left; box-shadow: 0 14px 34px -14px rgba(0,0,0,.5); }
    .c.flash { animation: flash 1.2s ease-out; } @keyframes flash { 0% { box-shadow: 0 0 0 0 var(--accent); } 40% { box-shadow: 0 0 0 6px color-mix(in srgb, var(--accent) 25%, transparent); } }
    .about { all: unset; cursor: pointer; display: block; color: var(--soft); font-size: 12px; margin-bottom: 6px; text-decoration: underline; }
    .who { font: 600 12px ui-monospace, Menlo, monospace; color: var(--accent-ink); }
    h3 { margin: 4px 0 0; font-size: 18px; line-height: 1.3; font-weight: 700; }
    .ctx { margin: 6px 0 0; color: var(--soft); white-space: pre-wrap; } .ctx:empty { display: none; }
    .rec { margin-top: 10px; background: var(--tint); border-radius: 10px; padding: 8px 12px; font-size: 13.5px; }
    .rec b { color: var(--accent-ink); }
    .opts { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 8px; margin-top: 12px; }
    .o { all: unset; box-sizing: border-box; cursor: pointer; padding: 11px 13px; border: 1.5px solid var(--line); border-radius: 11px; background: var(--bg); color: var(--ink); }
    button:focus-visible, textarea:focus-visible, .c:focus-visible { outline: 3px solid var(--accent); outline-offset: 3px; }
    .o:hover { border-color: var(--accent); }
    .o b { display: block; font-size: 15px; font-weight: 650; }
    .o span { display: block; font-size: 13px; color: var(--soft); margin-top: 2px; } .o span:empty { display: none; }
    .o.rec-o b::after { content: "recommended"; margin-left: 8px; font-size: 11px; font-weight: 600; color: var(--bg); background: var(--accent-ink); border-radius: 99px; padding: 1px 7px; vertical-align: 2px; }
    .o.on { border-color: var(--accent); background: var(--tint); }
    .row { display: flex; gap: 8px; margin-top: 10px; align-items: flex-end; }
    textarea { flex: 1; min-width: 0; min-height: 40px; font-size: 14px; color: var(--ink); background: var(--bg); border: 1.5px solid var(--line); border-radius: 10px; padding: 8px 10px; resize: vertical; }
    textarea:focus { outline: none; border-color: var(--accent); }
    .go { all: unset; cursor: pointer; font-size: 14px; font-weight: 600; padding: 9px 16px; border-radius: 999px; background: var(--jy-action); color: var(--jy-action-ink); white-space: nowrap; }
    .go[disabled] { opacity: .4; cursor: default; }
    .hint { font-size: 12px; color: var(--soft); margin-top: 6px; }
    .done { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; }
    .done .q { color: var(--soft); } .done .a { font-weight: 650; } .done .n { color: var(--soft); white-space: pre-wrap; }
    .chg { all: unset; cursor: pointer; color: var(--accent-ink); font-size: 13px; font-weight: 600; margin-left: auto; }
    .c.answered { border-left-color: #3aa57a; padding: 10px 14px; }`;

  // Light or dark from the page's own background, so the card belongs to the page.
  // jelly's parts match the view they sit on, light or dark, whatever the reader's own setting.
  function scheme(el) {
    const dark = pageIsDark();
    el.classList.toggle("jy-dark", dark);
    el.classList.toggle("jy-light", !dark);
  }
  function pageIsDark() {
    for (const el of [document.body, document.documentElement]) {
      const m = getComputedStyle(el).backgroundColor.match(/[\d.]+/g);
      if (m && (m.length < 4 || +m[3] > 0.5)) return (0.299 * m[0] + 0.587 * m[1] + 0.114 * m[2]) < 128;
    }
    // No background anywhere: the browser paints white unless the page opts into dark.
    const scheme = getComputedStyle(document.documentElement).colorScheme + " " + (document.querySelector('meta[name="color-scheme"]')?.content || "");
    return /dark/.test(scheme) && matchMedia("(prefers-color-scheme: dark)").matches;
  }
  function renderCard(host, a) {
    const isAsk = !a.seen; // a seen ask can be answered with nothing
    const opts = a.options || [];
    const answered = a.answer && !editing.has(a.id);
    const draft = host.querySelector("textarea")?.value || "";
    host.innerHTML = `<div class="c${answered ? " answered" : ""}"></div>`;
    const c = host.querySelector(".c");
    c.tabIndex = -1;
    c.setAttribute("aria-label", a.title);
    if (answered) {
      const o = opts.find((x) => x.id === a.answer.option);
      c.innerHTML = `<div class="done"><span class="q"></span><span class="a"></span><span class="n"></span><button class="chg">Change</button></div>`;
      c.querySelector(".q").textContent = a.title;
      c.querySelector(".a").textContent = "✓ " + (o ? o.label || o.id : a.seen ? "Seen" : "Answered");
      c.querySelector(".n").textContent = a.answer.text || "";
      c.querySelector(".chg").onclick = () => { editing.add(a.id); drawAsks(); };
      return;
    }
    const rec = opts.find((o) => o.id === a.recommend);
    const about = anchorOf(a).element;
    c.innerHTML = `${about ? '<button class="about"></button>' : ""}<div class="who"></div><h3></h3><p class="ctx"></p>
      ${rec ? '<div class="rec"><b></b> <span></span></div>' : ""}
      ${opts.length ? '<div class="opts"></div>' : ""}
      <div class="row"><textarea rows="1"></textarea><button class="go"></button></div>
      <div class="hint">${opts.length ? "Click an option to answer. Add a note first if you like." : "Enter to send · Shift+Enter for a new line"}</div>`;
    if (about) {
      c.querySelector(".about").textContent = `About ${a.anchor.quote ? `"${a.anchor.quote}"` : a.anchor.selector} · show it`;
      c.querySelector(".about").onclick = () => point(about);
    }
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
  let stack = null, stackOpen = false, askPins = [];
  function drawAsks() {
    if (!document.body) return;
    const items = shownItems().filter((a) => !drawnByView(a.id));
    for (const [id, card] of cards) if (!items.some((a) => a.id === id)) { card.remove(); cards.delete(id); editing.delete(id); }
    askPins.forEach((p) => p.remove());
    askPins = [];
    if (!items.length) { stack?.remove(); stack = null; return postSnapshot(); }
    if (!stack?.isConnected) {
      stack = document.createElement("jelly-asks");
      stack.className = "jf-ask";
      (stack.shadowRoot || stack.attachShadow({ mode: "open" })).innerHTML = `<style>${CARD_CSS}${JELLY_CSS}</style><div class="sheet"><button class="tab"></button><div class="list"></div></div>`;
      stack.shadowRoot.querySelector(".tab").onclick = () => { stackOpen = !stackOpen; drawAsks(); };
      cards.clear();
      ui().appendChild(stack);
    }
    const sheet = stack.shadowRoot.querySelector(".sheet"), list = sheet.querySelector(".list");
    items.forEach((a, i) => {
      let card = cards.get(a.id);
      if (!card) {
        card = document.createElement("div");
        card.id = `jf-q-${a.id}`;
        cards.set(a.id, card);
        // A new question opens the stack, unless the page is too short to share with it.
        if (isOpen(a) && innerHeight >= 480) stackOpen = true;
      }
      const sig = JSON.stringify([a.title, a.context, a.options, a.answer, a.anchor, editing.has(a.id)]);
      if (card.dataset.sig !== sig) { card.dataset.sig = sig; renderCard(card, a); }
      if (list.children[i] !== card) list.insertBefore(card, list.children[i] || null);
      const el = isOpen(a) && anchorOf(a).element;
      if (!el || isOurs(el) || !el.getClientRects().length) return;
      const r = el.getBoundingClientRect(), pin = document.createElement("div");
      pin.className = "jf-marker ask";
      pin.textContent = "?";
      pin.title = a.title;
      // On the element's top right corner; comment pins sit in the left margin.
      pin.style.left = Math.min(r.right - 10, document.documentElement.clientWidth - 22) + scrollX + "px";
      pin.style.top = r.top - 10 + scrollY + "px";
      pin.onclick = (e) => { e.stopPropagation(); openAsk(a.id).catch(console.error); };
      ui().appendChild(pin);
      askPins.push(pin);
    });
    const open = items.filter(isOpen).length;
    scheme(stack);
    sheet.classList.toggle("shut", !stackOpen);
    sheet.querySelector(".tab").textContent = (open ? `${open} question${open > 1 ? "s" : ""}` : "Answered") + (stackOpen ? " · hide" : " · show");
    postSnapshot();
  }
  let drawQueued = false;
  const queueDraw = () => { if (!drawQueued) { drawQueued = true; requestAnimationFrame(() => { drawQueued = false; drawMarkers(); placeSend(); }); } };
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
    openingMessage.style.cssText = "position:fixed;top:12px;left:50%;transform:translateX(-50%);z-index:2147483001;padding:16px;background:#fff;color:#1b2221;border:2px solid #b26a12;font:16px system-ui";
    ui().appendChild(openingMessage);
    openingMessage.focus();
  }
  async function openAsk(id) {
    const request = ++opening;
    let item = asks.find((a) => a.id === id);
    if (!item) {
      try {
        await refreshThreads();
      } catch (e) {
        if (request === opening) showOpeningMessage("Could not load this question. Open it from the dock to try again.");
        console.error(e);
        return;
      }
      if (request !== opening) return;
      item = asks.find((a) => a.id === id);
    }
    openingMessage?.remove();
    if (!item || item.unasked) {
      showOpeningMessage("This question is no longer waiting for an answer.");
      return;
    }
    for (const fn of openSubs) { try { fn(item); } catch (e) { console.error(e); } }
    const own = drawnByView(id);
    if (own) {
      own.scrollIntoView({ block: "start" });
      let control = own.querySelector("button:not(:disabled), textarea:not(:disabled), input:not(:disabled)") || own;
      // A control out of sight must not take Enter: focus the question's heading instead.
      const r = control.getBoundingClientRect();
      if (r.top < 0 || r.bottom > innerHeight || r.left < 0 || r.right > innerWidth) control = own.querySelector("h1, h2, h3") || own;
      if (!control.matches("button, textarea, input")) control.tabIndex = -1;
      return control.focus({ preventScroll: true });
    }
    stackOpen = true;
    drawAsks();
    const card = cards.get(id);
    if (!card) return;
    card.scrollIntoView({ block: "nearest" });
    const about = anchorOf(item).element;
    if (about && !isOurs(about)) point(about);
    (card.querySelector("button.o:not(:disabled)") || card.querySelector("textarea:not(:disabled)") || card.querySelector(".c")).focus({ preventScroll: true });
    const c = card.querySelector(".c");
    c.classList.remove("flash"); void c.offsetWidth; c.classList.add("flash");
  }
  addEventListener("message", (e) => {
    if (e.origin === location.origin && e.data?.type === "jelly:ask") openAsk(e.data.id).catch(console.error);
  });

  let workEl = null;
  // The person works through a jelly, then hands the turn back with one press.
  // A view that draws its own marks it with data-send and calls jelly.person.send().
  let sendEl = null;
  function drawSend() {
    // The jelly page is not a jelly an agent waits on: it shows no Send.
    const show = unsent > 0 && boot.name !== "page" && document.body && !agentMode && share?.can_act !== false && !document.querySelector("[data-send]");
    if (!show) { sendEl?.remove(); sendEl = null; return; }
    if (!sendEl) {
      sendEl = document.createElement("button");
      sendEl.className = "jf-btn jf-send";
      sendEl.title = "Tell the agent you are done here for now";
      sendEl.onclick = (e) => { e.stopPropagation(); jelly.person.send().catch(console.error); };
    }
    sendEl.textContent = `${unsent} change${unsent > 1 ? "s" : ""} · Send`;
    if (!sendEl.isConnected) ui().appendChild(sendEl);
    placeSend();
  }
  // Send sits in a corner, where a view often has its own buttons (a chat's
  // reply row). When it would lie on one, it moves up until it is clear of
  // them, and back down when they are gone.
  function placeSend() {
    if (!sendEl?.isConnected) return;
    sendEl.style.bottom = "";
    for (let i = 0; i < 4; i++) {
      const r = sendEl.getBoundingClientRect();
      let top = Infinity;
      for (const x of [r.left + 2, (r.left + r.right) / 2, r.right - 2]) for (const y of [r.top + 2, (r.top + r.bottom) / 2, r.bottom - 2]) {
        for (const el of document.elementsFromPoint(x, y)) {
          const c = isOurs(el) ? null : el.closest(CONTROL), box = c?.getBoundingClientRect();
          // A whole card that is clickable stays usable around the button.
          if (c && box.height <= 3 * r.height) top = Math.min(top, box.top);
        }
      }
      if (top === Infinity) return;
      // No room above: where it was is no worse.
      if (top - 8 - r.height < 0) { sendEl.style.bottom = ""; return; }
      sendEl.style.bottom = `${innerHeight - top + 8}px`;
    }
  }
  function drawWorking() {
    workEl?.remove();
    workEl = null;
    if (!working || !document.body) return;
    workEl = document.createElement("div");
    workEl.className = "jf-work";
    workEl.innerHTML = "<i></i><span></span>";
    workEl.querySelector("span").textContent = `${working.by} is working on this${working.what ? ": " + working.what : ""}`;
    ui().appendChild(workEl);
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
    // A view's own open shadow roots are part of what the person sees.
    const kids = (el) => (el.shadowRoot ? [...el.shadowRoot.children, ...el.children] : el.children);
    (function walk(el, depth) {
      for (const c of kids(el)) {
        if (lines.length > 400 || ["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "LINK", "META"].includes(c.tagName) || (getComputedStyle(c).display === "contents" ? (walk(c, depth), true) : !shown(c))) continue;
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
        else if (c.id && c.children.length === 0 && !c.shadowRoot) { const t = label(c); if (t) add(depth, `text "${t}"`, c); }
        else if ((GROUP.has(tag) || role || c.id) && c.innerText?.trim()) { add(depth, `${role || tag}${c.getAttribute("aria-label") ? ` "${c.getAttribute("aria-label")}"` : ""}:`, c); const own = ownText(c); if (own) add(depth + 1, `text "${own}"`, {}); walk(c, depth + 1); continue; }
        else if (c.children.length === 0 && !c.shadowRoot) { const t = label(c); if (t) add(depth, `text "${t}"`, c); }
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
  // What the view's own scripts and on* attributes use of `jelly` that the runtime
  // does not have. A view written from an older copy of the skill does this, and
  // it only throws when someone clicks, so a render alone never shows it.
  function unknownJelly() {
    const sources = [...document.scripts].filter((s) => !s.src && !isOurs(s) && !s.textContent.startsWith("window.__JELLY__=")).map((s) => s.textContent)
      .concat([...document.body.querySelectorAll("*")].filter((e) => !isOurs(e)).flatMap((e) => [...e.attributes].filter((a) => a.name.startsWith("on")).map((a) => a.value)));
    const found = new Set(), has = `it has ${Object.keys(jelly).join(", ")}`;
    // `jelly.x(` or `jelly.x.`, so a file name such as "jelly.png" is not read as a use.
    for (const src of sources) for (const [, first, second, call] of src.matchAll(/(?<![\w$.\/-])jelly\.([A-Za-z_$][\w$]*)(?:\.([A-Za-z_$][\w$]*))?(?=\s*([(.]))/g)) {
      const group = jelly[first];
      if (!(first in jelly)) found.add(`jelly.${first} (${has})`);
      else if (!second && call === "(" && typeof group !== "function") found.add(`jelly.${first}() (it is not a function)`);
      // Only jelly's own groups, such as person: state is the view's data.
      else if (second && first !== "state" && group && typeof group === "object" && !(second in group)) found.add(`jelly.${first}.${second} (jelly.${first} has ${Object.keys(group).join(", ")})`);
    }
    return [...found];
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
      if (p && (!isOurs(p) || p.closest(".jf-btn")) && !c.contains(p) && !p.contains(c) && !(c.labels && [...c.labels].some((l) => l.contains(p))) && !scrollsClear(r, p) && !clipped(c, r)) {
        covered.push(`${name} under ${p.closest(".jf-send") ? "jelly's Send button (move it with --jelly-send-inset)" : p.closest(".jf-btn") ? "jelly's Comment button (move it with --jelly-comment-inset)" : shortSelector(p)}`);
      }
      // A labelled checkbox or radio is tapped through its label; a link inside text is exempt.
      const viaLabel = c.matches("input[type=checkbox], input[type=radio]") && c.labels?.length;
      if (W < 600 && !viaLabel && getComputedStyle(c).display !== "inline" && undersized(r) && !spaced(c, r)) small.push(`${name} ${Math.round(r.width)}×${Math.round(r.height)}`);
      if (c.matches("input:not([type=submit]):not([type=button]):not([type=reset]):not([type=image]), select, textarea")
        && !c.labels?.length && !c.getAttribute("aria-label") && !c.getAttribute("aria-labelledby") && !c.title && !c.placeholder) unlabeled.push(name);
    }
    add("the view uses a part of jelly that does not exist, so it fails on a click", unknownJelly());
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
        body: JSON.stringify({ owner: boot.snapshotOwner, text: agentSnapshot(), width: innerWidth, height: innerHeight, version, errors, dialogs,
          // Only `jelly look`'s own renders measure problems; a person's browser does not pay for them.
          ...(agentMode && { dark: darkRender, contrast: contrast(), problems: darkRender ? [] : problems() }),
          page_height: document.documentElement.scrollHeight, asks: askReport() }) }).then((r) => { if (r.status === 409) reload(); }).catch(() => {});
    }, agentMode ? 400 : 1200);
  }
  // How each open ask shows: drawn by the view or in jelly's stack, and whether
  // the element it is about is on the page.
  function askReport() {
    return asks.filter(isOpen).map((a) => {
      const at = anchorOf(a);
      return { id: a.id, drawn: !!drawnByView(a.id), at: typeof a.anchor?.selector === "string" ? a.anchor.selector : "",
        anchor: at.invalid ? "invalid" : at.element ? "matched" : at.element === null ? "missing" : "none" };
    });
  }
  addEventListener("load", postSnapshot);
  subs.push(() => postSnapshot());

  // ---- jelly try: do the steps, then report what the page shows ----
  let trialCalls = 0;
  async function trialCall(verb, args) {
    trialCalls++;
    try {
      const r = await fetch(`/api/tries/${trial.token}/do`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ verb, args, by: me() }) });
      const j = await r.json();
      if (!r.ok) throw new Error(j.error || r.statusText);
      return j;
    } finally { trialCalls--; }
  }
  const pause = (ms) => new Promise((r) => setTimeout(r, ms));
  // Does one step the way a person would; returns why it could not be done.
  function doStep(s) {
    let el;
    // The page first, then inside the view's own open shadow roots.
    const find = (root) => root.querySelector(s.at) || [...root.querySelectorAll("*")].reduce((hit, e) => hit || (e.shadowRoot && find(e.shadowRoot)), null);
    try { el = find(document.body); } catch { return `${s.at} is not a valid CSS selector`; }
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
      body: JSON.stringify({ text: agentSnapshot(), width: innerWidth, height: innerHeight, errors, dialogs, failed }) }).catch(() => {});
  }
  if (trial) addEventListener("load", () => setTimeout(runTrial, 300));
  // Text a page fills in later (fetches, timers) counts too.
  new MutationObserver(() => postSnapshot()).observe(document.documentElement, { childList: true, subtree: true, characterData: true });

  let latestRefresh;
  function refreshThreads() {
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
      comments = j.comments || []; asks = j.asks || []; working = j.working || null; unsent = j.unsent || 0;
      // The view first: an ask it draws must not flash in the stack.
      for (const fn of askSubs) { try { fn(); } catch (e) { console.error(e); } }
      drawMarkers(); drawWorking(); drawSend();
    })();
    latestRefresh = refresh;
    return refresh;
  }

  addEventListener("DOMContentLoaded", () => {
    if (!style.isConnected) document.head.appendChild(style);
    // A view that draws its own comment control opts out with <meta name="jelly-comment" content="off">.
    const optOut = document.querySelector('meta[name="jelly-comment"]')?.content === "off";
    if (!embedded && !optOut && share?.can_act !== false) ui().appendChild(btn);
    drawMarkers();
    drawWorking();
    drawSend();
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
  // On the jelly page, a key the jelly does not use goes up to the page, which
  // decides whether it is one of its shortcuts. The jelly keeps a key typed into a
  // field or a comment, Enter on a button or link, a key with Ctrl, Alt or Cmd, and
  // a key the view called preventDefault() on (checked a tick later, since a view's
  // own listener on window runs after this one). Esc first closes an open comment.
  addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      setMode(false);
      if (pop?.isConnected) { pop.remove(); pop = null; return; }
    }
    if (!embedded || e.metaKey || e.ctrlKey || e.altKey || e.isComposing || pop?.isConnected) return;
    if (e.target.closest?.(".jf-pop, input, textarea, select") || e.target.isContentEditable) return;
    if (e.key === "Enter" && e.target.closest?.("button, a[href], summary")) return;
    setTimeout(() => { if (!e.defaultPrevented) parent.postMessage({ type: "jelly:key", name: boot.name, key: e.key }, location.origin); });
  });
  // A click or Tab into a jelly never reaches the page, so the jelly tells it, and
  // the page selects it. A click into a jelly that already has focus fires no focus.
  const focused = () => { if (embedded) parent.postMessage({ type: "jelly:focus", name: boot.name }, location.origin); };
  addEventListener("focus", focused);
  addEventListener("pointerdown", focused);
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
