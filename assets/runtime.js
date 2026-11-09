// Injected into every jelly view as `window.jelly`.
//
//   jelly.state                 current state (read only; change it with verbs)
//   jelly.onState(fn)           fn(state, event) now and after every change, from anyone
//   jelly.call(verb, args)      run a verb; resolves to the event, rejects with the reason
//   jelly.read(name)            another jelly's state ("plan" means plan in this project)
//   jelly.asset(file)           URL of a file added with `jelly asset` (or just use "assets/<file>")
//   jelly.watch(name, fn)       fn(state, event) for another jelly's changes (a feed)
//
// Feedback anywhere: hold Option (Alt) and click any element, or press the
// Comment button, to pin a comment to it. The agent gets a `comment` event that
// names the element. Give elements an `id` so comments survive a reshape.
// Agents answer on the pin (`jelly reply`); the thread shows when you click it.
//
// A reshape or an undo that changes the view reloads the page. State lives in
// the daemon, so nothing is lost.
(() => {
  const boot = window.__JELLY__;
  let state = boot.state;
  let version = boot.version;
  let feedback = boot.feedback || [];
  let attention = boot.attention || [];
  let working = boot.working || null;
  const subs = [];
  const enc = encodeURIComponent;
  const api = `/api/jellies/${enc(boot.name)}`;
  // Names are unique per project: a short name means a jelly in this one.
  const project = boot.name.includes(":") ? boot.name.split(":")[0] : "";
  const full = (name) => (name.includes(":") || name === "surface" || !project ? name : `${project}:${name}`);

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
      if (el?.nodeType === 1 && !el.closest(".jf-pop, .jf-pin, .jf-att")) lastUsed = { el, at: Date.now() };
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

  async function post(path, body) {
    const r = await fetch(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ ...body, by: "person" }),
    });
    const j = await r.json();
    if (!r.ok) throw new Error(j.error || r.statusText);
    return j;
  }

  // One WebSocket per page carries every event; handlers filter by jelly.
  const listeners = new Set();
  let dropped = false;
  (function connect() {
    const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/ws`);
    ws.onopen = () => { if (dropped) location.reload(); };
    ws.onmessage = (m) => {
      const ev = JSON.parse(m.data);
      if (ev.kind === "resync") return location.reload();
      for (const fn of listeners) {
        try { fn(ev); } catch (e) { console.error(e); }
      }
    };
    ws.onclose = () => { dropped = true; setTimeout(connect, 1000); };
  })();

  listeners.add((ev) => {
    if (ev.jelly !== boot.name) return;
    if (ev.kind === "remove") {
      document.body.innerHTML = '<p style="font:14px system-ui;padding:16px;opacity:.7">This jelly was removed.</p>';
      return;
    }
    if (ev.view_rev !== boot.viewRev) { location.reload(); return; }
    if (ev.kind === "comment" || ev.kind === "resolve" || ev.kind === "reply") refreshFeedback();
    if (ev.kind === "attention" || ev.kind === "answer" || ev.kind === "withdraw") refreshFeedback();
    if (ev.kind === "working") { working = ev.args ? { what: ev.args, by: ev.by } : null; drawWorking(); }
    apply(ev);
  });

  window.jelly = {
    name: boot.name,
    about: boot.about,
    get state() { return state; },
    /** Every event from every jelly. Returns a function that stops listening. */
    events(fn) { listeners.add(fn); return () => listeners.delete(fn); },
    get feedback() { return feedback; },
    onState(fn) { subs.push(fn); fn(state, null); },
    async call(verb, args = {}) {
      const ev = await post(`${api}/call`, { verb, args, source: source() });
      apply(ev);
      return ev;
    },
    async read(name) {
      const r = await fetch(`/api/jellies/${enc(full(name))}/state`);
      if (!r.ok) throw new Error((await r.json()).error);
      return r.json();
    },
    watch(name, fn) {
      name = full(name);
      let seen = 0;
      this.read(name).then((s) => fn(s, null), (e) => console.warn(e.message));
      return this.events((ev) => {
        if (ev.jelly === name && ev.state !== undefined && ev.version > seen) { seen = ev.version; fn(ev.state, ev); }
      });
    },
    comment: (anchor, text) => post(`${api}/comment`, { anchor, text }),
    /** Put another jelly on screen: inside the surface it is focused, alone it opens. */
    show(name) {
      name = full(name);
      if (embedded) parent.postMessage({ type: "jelly:show", name }, location.origin);
      else location.href = `/j/${enc(name)}/`;
    },
    resolve: (id) => post(`${api}/resolve`, { id }),
    /** Open asks and looks the agent put on this jelly. */
    get attention() { return attention.filter((a) => !a.answer); },
    /** Answer one: an option id, text, or both. A look takes nothing ("seen"). */
    answer: (id, { option, text } = {}) => post(`${api}/attention/${enc(id)}/answer`, { option, text: text || "" }),
    asset: (file) => `/j/${enc(boot.name)}/assets/${enc(file)}`,
    reply: (id, text) => post(`${api}/reply`, { id, text }),
  };

  // ---- feedback layer -------------------------------------------------------

  const css = `
    .jf-btn { position: fixed; right: 10px; bottom: 10px; z-index: 2147483000; font: 12px -apple-system, sans-serif;
      background: #1a2021; color: #d6dedc; border: 1px solid #2f383a; border-radius: 999px; padding: 5px 11px; cursor: pointer; opacity: .85; }
    .jf-btn.on { background: #e0a24f; color: #1b2221; border-color: #e0a24f; opacity: 1; }
    .jf-hover { outline: 2px dashed #e0a24f !important; outline-offset: 2px; cursor: crosshair !important; }
    .jf-pin { position: absolute; z-index: 2147482999; width: 20px; height: 20px; border-radius: 50% 50% 0 50%; background: #e0a24f; color: #1b2221;
      font: 600 11px -apple-system, sans-serif; display: grid; place-items: center; cursor: pointer; box-shadow: 0 2px 6px rgba(0,0,0,.3); }
    .jf-pin.agent { background: #46c4b8; }
    .jf-pin.done { opacity: .55; }
    .jf-pop { position: absolute; z-index: 2147483001; width: 260px; background: #fff; color: #1b2221; border: 1px solid #e2e7e5; border-radius: 10px;
      box-shadow: 0 14px 34px -10px rgba(0,0,0,.45); padding: 8px; font: 12px -apple-system, sans-serif; }
    .jf-pop .jf-on { font: 10px ui-monospace, Menlo, monospace; color: #66726f; margin-bottom: 5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .jf-pop textarea { width: 100%; box-sizing: border-box; min-height: 54px; font: 12px -apple-system, sans-serif; border: 1px solid #e2e7e5; border-radius: 6px; padding: 5px; resize: vertical; }
    .jf-pop .jf-row { display: flex; gap: 6px; justify-content: flex-end; margin-top: 6px; align-items: center; }
    .jf-pop button { font: 12px -apple-system, sans-serif; border: 1px solid #e2e7e5; background: #fff; border-radius: 6px; padding: 3px 9px; cursor: pointer; }
    .jf-pop button.jf-send { background: #0f7b74; color: #fff; border-color: #0f7b74; }
    .jf-pop .jf-by { font: 10px ui-monospace, monospace; color: #b26a12; }
    .jf-pop .jf-thread { display: grid; gap: 6px; margin-bottom: 6px; max-height: 260px; overflow-y: auto; }
    .jf-pop .jf-msg { border-left: 2px solid #e0a24f; padding: 1px 0 1px 7px; }
    .jf-pop .jf-msg.agent { border-color: #46c4b8; }
    .jf-pop .jf-msg.agent .jf-by { color: #0f7b74; }
    .jf-pop .jf-text { white-space: pre-wrap; margin-top: 1px; }
    .jf-pop .jf-done { font-size: 11px; color: #66726f; }
    .jf-att { position: absolute; z-index: 2147482999; height: 22px; min-width: 22px; padding: 0 7px; border-radius: 11px; background: #6d5ce8; color: #fff;
      font: 700 12px -apple-system, sans-serif; display: grid; place-items: center; cursor: pointer; box-shadow: 0 2px 8px rgba(80,60,200,.45); animation: jf-pulse 2s infinite; }
    .jf-att.look { background: #0f7b74; box-shadow: 0 2px 8px rgba(15,123,116,.45); }
    @keyframes jf-pulse { 50% { box-shadow: 0 0 0 6px rgba(109,92,232,.15); } }
    .jf-chip { position: fixed; right: 12px; top: 12px; z-index: 2147483000; background: #6d5ce8; color: #fff; border: 0; border-radius: 999px; padding: 6px 12px; font: 600 12.5px -apple-system, sans-serif; cursor: pointer; box-shadow: 0 6px 18px -6px rgba(80,60,200,.6); }
    .jf-pop .jf-q { font-weight: 600; font-size: 13px; margin: 2px 0 4px; }
    .jf-pop .jf-why { font-size: 11.5px; color: #4b3cc4; background: #f0eefe; border-radius: 6px; padding: 5px 7px; margin: 4px 0 6px; }
    .jf-pop .jf-opts { display: grid; gap: 4px; margin: 6px 0; }
    .jf-pop .jf-opt { text-align: left; border: 1px solid #e2e7e5; background: #fff; border-radius: 8px; padding: 6px 8px; cursor: pointer; }
    .jf-pop .jf-opt b { display: block; font-size: 12.5px; } .jf-pop .jf-opt span { font-size: 11.5px; color: #66726f; }
    .jf-pop .jf-opt.rec { border-color: #6d5ce8; background: #f6f5ff; } .jf-pop .jf-opt.rec b::after { content: " · recommended"; color: #6d5ce8; font-weight: 600; font-size: 11px; }
    .jf-pop .jf-opt.on { border: 2px solid #6d5ce8; padding: 5px 7px; }
    .jf-pop .jf-list { display: grid; gap: 6px; } .jf-pop .jf-li { border: 1px solid #e2e7e5; border-radius: 8px; padding: 6px 8px; cursor: pointer; font-size: 12.5px; }
    .jf-work { position: fixed; left: 50%; top: 10px; transform: translateX(-50%); z-index: 2147483000; background: #141a19; color: #e8eeec; border-radius: 999px; padding: 6px 14px 6px 12px; font: 12.5px -apple-system, sans-serif; display: flex; gap: 8px; align-items: center; box-shadow: 0 8px 24px -8px rgba(0,0,0,.5); }
    .jf-work i { width: 8px; height: 8px; border-radius: 50%; background: #46c4b8; animation: jf-blink 1s infinite; } @keyframes jf-blink { 50% { opacity: .3; } }`;
  let commenting = false, hovered = null, pop = null;
  const btn = document.createElement("button");
  btn.className = "jf-btn";
  btn.textContent = "Comment";
  btn.title = "Pin a comment to any part of this jelly (or hold Option and click)";

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
  const isOurs = (el) => el.closest(".jf-btn, .jf-pop, .jf-pin, .jf-att, .jf-chip, .jf-work");

  // Inside the surface, the surface's header owns the Comment button.
  const embedded = window.parent !== window;
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
    const send = () => jelly.comment(anchor, ta.value).then(() => { p.remove(); pop = null; }, (e) => alert(e.message));
    p.querySelector(".jf-send").onclick = send;
    p.querySelector(".jf-cancel").onclick = () => { p.remove(); pop = null; };
    ta.onkeydown = (e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) send(); if (e.key === "Escape") { p.remove(); pop = null; } };
  }

  let pinEls = [];
  const DAY = 86400000;
  const lastAt = (f) => Math.max(f.at_ms, ...(f.replies || []).map((r) => r.at_ms));
  // Open threads, plus resolved ones from the last day so the person sees the answer.
  const visible = () => feedback.filter((f) => !f.resolved || Date.now() - lastAt(f) < DAY);
  const who = (by) => (by === "person" ? "you" : by);

  function thread(f, x, y) {
    const msgs = [{ by: f.by, text: f.text, at_ms: f.at_ms }, ...(f.replies || [])];
    const p = openPop(x, y, `<div class="jf-on"></div><div class="jf-thread"></div>
      <textarea placeholder="${f.resolved ? "Not done? Say what is still off" : "Reply"}"></textarea>
      <div class="jf-row"><button class="jf-close">Close</button>${f.resolved ? "" : '<button class="jf-resolve">Resolve</button>'}<button class="jf-send">Reply</button></div>`);
    p.querySelector(".jf-on").textContent = f.anchor?.quote ? `on "${f.anchor.quote}"` : `on ${f.anchor?.selector || "the jelly"}`;
    const t = p.querySelector(".jf-thread");
    for (const m of msgs) {
      const d = document.createElement("div");
      d.className = "jf-msg" + (m.by === "person" ? "" : " agent");
      d.innerHTML = '<div class="jf-by"></div><div class="jf-text"></div>';
      d.querySelector(".jf-by").textContent = who(m.by);
      d.querySelector(".jf-text").textContent = m.text;
      t.appendChild(d);
    }
    if (f.resolved) t.insertAdjacentHTML("beforeend", '<div class="jf-done">Resolved</div>');
    const ta = p.querySelector("textarea");
    const close = () => { p.remove(); pop = null; };
    const send = () => ta.value.trim() && jelly.reply(f.id, ta.value).then(close, (e) => alert(e.message));
    p.querySelector(".jf-close").onclick = close;
    p.querySelector(".jf-send").onclick = send;
    const r = p.querySelector(".jf-resolve");
    if (r) r.onclick = () => jelly.resolve(f.id).then(close);
    ta.onkeydown = (e) => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) send(); if (e.key === "Escape") close(); };
  }

  function drawPins() {
    drawAttention();
    pinEls.forEach((p) => p.remove());
    pinEls = [];
    visible().forEach((f, i) => {
      let el = null;
      try { el = document.querySelector(f.anchor?.selector || "body"); } catch {}
      if (!el) return;
      const r = el.getBoundingClientRect();
      const last = f.replies?.at(-1);
      const pin = document.createElement("div");
      // Amber: waiting on the agent. Teal: the agent answered. Faded: resolved.
      pin.className = "jf-pin" + (last && last.by !== "person" ? " agent" : "") + (f.resolved ? " done" : "");
      pin.textContent = f.resolved ? "✓" : i + 1;
      pin.title = last ? `${who(last.by)}: ${last.text}` : f.text;
      // In the margin left of the element, pointing at it.
      pin.style.left = Math.max(2, r.left - 24) + scrollX + "px";
      pin.style.top = r.top + Math.min(r.height, 40) / 2 - 10 + scrollY + "px";
      pin.onclick = (e) => { e.stopPropagation(); thread(f, r.left, r.bottom); };
      document.body.appendChild(pin);
      pinEls.push(pin);
    });
  }
  // ---- attention: asks and looks the agent put here ------------------------
  let attEls = [];
  function askPop(a, x, y) {
    const isAsk = a.kind === "ask";
    let picked = null;
    const p = openPop(x, y, `<div class="jf-by"></div><div class="jf-q"></div><div class="jf-text" style="color:#4b5553"></div>
      ${a.why ? '<div class="jf-why"></div>' : ""}<div class="jf-opts"></div>
      <textarea placeholder="${isAsk ? (a.options?.length ? "Add context or a condition (optional)" : "Your answer") : "Anything to say? (optional)"}"></textarea>
      <div class="jf-row"><button class="jf-close">Later</button><button class="jf-send">${isAsk ? "Answer" : "Seen"}</button></div>`);
    p.querySelector(".jf-by").textContent = `${a.by} ${isAsk ? "asks" : "asks you to look"}`;
    p.querySelector(".jf-q").textContent = a.title;
    p.querySelector(".jf-text").textContent = a.detail || "";
    if (a.why) p.querySelector(".jf-why").textContent = `Recommends ${a.options.find((o) => o.id === a.recommend)?.label || a.recommend}: ${a.why}`;
    const box = p.querySelector(".jf-opts");
    for (const o of a.options || []) {
      const b = document.createElement("button");
      b.className = "jf-opt" + (o.id === a.recommend ? " rec" : "");
      b.innerHTML = "<b></b><span></span>";
      b.querySelector("b").textContent = o.label || o.id;
      b.querySelector("span").textContent = o.does || "";
      b.onclick = () => { picked = o.id; box.querySelectorAll(".jf-opt").forEach((x) => x.classList.toggle("on", x === b)); };
      box.appendChild(b);
    }
    const ta = p.querySelector("textarea");
    const close = () => { p.remove(); pop = null; };
    p.querySelector(".jf-close").onclick = close;
    p.querySelector(".jf-send").onclick = () => jelly.answer(a.id, { option: picked || undefined, text: ta.value }).then(close, (e) => alert(e.message));
  }
  function drawAttention() {
    attEls.forEach((e) => e.remove());
    attEls = [];
    const open = attention.filter((a) => !a.answer);
    const loose = [];
    for (const a of open) {
      let el = null;
      try { el = a.anchor?.selector ? document.querySelector(a.anchor.selector) : null; } catch {}
      if (!el) { loose.push(a); continue; }
      const r = el.getBoundingClientRect();
      const m = document.createElement("div");
      m.className = "jf-att" + (a.kind === "look" ? " look" : "");
      m.textContent = a.kind === "look" ? "look" : "?";
      m.title = a.title;
      m.style.left = Math.max(2, r.right - 18) + scrollX + "px";
      m.style.top = Math.max(2, r.top - 10) + scrollY + "px";
      m.onclick = (e) => { e.stopPropagation(); askPop(a, r.left, r.bottom); };
      document.body.appendChild(m);
      attEls.push(m);
    }
    if (loose.length) {
      const c = document.createElement("button");
      c.className = "jf-chip";
      c.textContent = loose.length === 1 ? (loose[0].kind === "look" ? "1 thing to look at" : "1 question for you") : `${loose.length} things for you`;
      c.onclick = (e) => {
        e.stopPropagation();
        if (loose.length === 1) return askPop(loose[0], innerWidth - 290, 50);
        const p = openPop(innerWidth - 290, 50, '<div class="jf-list"></div>');
        for (const a of loose) {
          const li = document.createElement("div");
          li.className = "jf-li";
          li.textContent = a.title;
          li.onclick = () => askPop(a, innerWidth - 290, 50);
          p.querySelector(".jf-list").appendChild(li);
        }
      };
      document.body.appendChild(c);
      attEls.push(c);
    }
  }
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

  // ---- agent view: what is on screen, as text, for `jelly see` ----------------
  const SKIP = ".jf-btn, .jf-pop, .jf-pin, .jf-att, .jf-chip, .jf-work";
  const LEAF = new Set(["button", "a", "input", "textarea", "select", "img", "canvas", "svg", "iframe", "video", "p", "li", "pre", "h1", "h2", "h3", "h4", "h5", "h6", "label", "td", "th", "figcaption", "blockquote", "summary"]);
  const GROUP = new Set(["section", "article", "nav", "header", "footer", "main", "aside", "form", "table", "ul", "ol", "dialog", "details", "figure"]);
  function agentSnapshot() {
    const lines = [];
    const label = (el) => (el.getAttribute("aria-label") || el.innerText || el.value || el.alt || el.title || "").trim().replace(/\s+/g, " ").slice(0, 120);
    const shown = (el) => { const r = el.getBoundingClientRect(); const cs = getComputedStyle(el); return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none" && cs.opacity !== "0"; };
    const add = (depth, text, el) => lines.push(`${"  ".repeat(depth)}- ${text}${el.id ? ` #${el.id}` : ""}`);
    (function walk(el, depth) {
      for (const c of el.children) {
        if (lines.length > 400 || c.matches(SKIP) || ["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "LINK", "META"].includes(c.tagName) || !shown(c)) continue;
        const tag = c.tagName.toLowerCase(), role = c.getAttribute("role");
        if (/^h[1-6]$/.test(tag)) add(depth, `heading "${label(c)}" [${tag}]`, c);
        else if (tag === "button" || role === "button") add(depth, `button "${label(c)}"${c.disabled ? " (disabled)" : ""}`, c);
        else if (tag === "a") add(depth, `link "${label(c)}"`, c);
        else if (tag === "input" && (c.type === "checkbox" || c.type === "radio")) add(depth, `${c.type} "${label(c.closest("label") || c)}" ${c.checked ? "checked" : "unchecked"}`, c);
        else if (tag === "input" || tag === "textarea") add(depth, `${tag === "textarea" || !c.type || c.type === "text" ? "textbox" : c.type}${c.placeholder ? ` "${c.placeholder}"` : ""} = "${String(c.value).slice(0, 80)}"`, c);
        else if (tag === "select") add(depth, `select = "${c.options[c.selectedIndex]?.text || ""}"`, c);
        else if (tag === "img") add(depth, `img "${c.alt || ""}"`, c);
        else if (["canvas", "svg", "video", "iframe"].includes(tag)) { const r = c.getBoundingClientRect(); add(depth, `${tag} ${Math.round(r.width)}×${Math.round(r.height)}${c.title ? ` "${c.title}"` : ""}${tag === "svg" && label(c) ? ` "${label(c).slice(0, 60)}"` : ""}`, c); }
        else if (LEAF.has(tag)) { const t = label(c); if (t) add(depth, `text "${t}"`, c); }
        else if ((GROUP.has(tag) || role || c.id) && c.innerText?.trim()) { add(depth, `${role || tag}${c.getAttribute("aria-label") ? ` "${c.getAttribute("aria-label")}"` : ""}:`, c); walk(c, depth + 1); continue; }
        else if (c.children.length === 0) { const t = label(c); if (t) add(depth, `text "${t}"`, c); }
        else walk(c, depth);
      }
    })(document.body, 0);
    return lines.join("\n");
  }
  let snapT = null;
  const agentMode = new URLSearchParams(location.search).has("agent");
  function postSnapshot() {
    clearTimeout(snapT);
    snapT = setTimeout(() => {
      fetch(`${api}/snapshot`, { method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ text: agentSnapshot(), width: innerWidth, height: innerHeight, version }) }).catch(() => {});
    }, agentMode ? 400 : 1200);
  }
  addEventListener("load", postSnapshot);
  subs.push(() => postSnapshot());

  async function refreshFeedback() {
    const r = await fetch(api);
    if (r.ok) { const j = await r.json(); feedback = j.feedback || []; attention = j.attention || []; working = j.working || null; drawPins(); drawWorking(); }
  }

  addEventListener("DOMContentLoaded", () => {
    const style = document.createElement("style");
    style.textContent = css;
    document.head.appendChild(style);
    // A view that draws its own comment control opts out with <meta name="jelly-comment" content="off">.
    const optOut = document.querySelector('meta[name="jelly-comment"]')?.content === "off";
    if (!embedded && !optOut) document.body.appendChild(btn);
    drawPins();
    drawWorking();
    new ResizeObserver(drawPins).observe(document.body);
  });
  subs.push(() => setTimeout(drawPins, 0));
  btn.onclick = (e) => { e.stopPropagation(); setMode(!commenting); };
  addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    // Esc inside a jelly closes the comment mode first, then reaches the surface.
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
