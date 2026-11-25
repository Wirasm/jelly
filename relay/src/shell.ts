// The page a link opens. It holds the link's WebSocket to the shared jelly and
// shows the jelly's view in a sandboxed iframe with an opaque origin: the view
// cannot read this page, its URL (the link) or the socket, and reaches the
// jelly only through postMessage, which this page turns into the few things a
// link may do. The view runs the daemon's own runtime.js, sent by the daemon.
// In a PIN's room, the strip under the view lists the room's jellies: a tap
// gets this person their own link on that one (POST /room) and opens it.
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
export const FAVICON = "data:image/svg+xml," + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path d="M8 30 C4 22 6 10 20 9 C34 10 36 22 32 30 C29 35 11 35 8 30Z" fill="#ff4f8b"/><ellipse cx="15" cy="16" rx="4" ry="2.4" fill="#fff" opacity=".7"/><circle cx="16" cy="24" r="1.9" fill="#3b0a1e"/><circle cx="24" cy="24" r="1.9" fill="#3b0a1e"/></svg>`);

/** What a pasted link's preview and a browser tab show besides the title: the card image and the icons. */
export const CARD = `<meta property="og:site_name" content="jelly">
<meta property="og:image" content="https://getjellyware.com/og.png">
<meta property="og:image:width" content="1200"><meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<link rel="icon" href="${FAVICON}">
<link rel="apple-touch-icon" href="https://getjellyware.com/apple-touch-icon.png">`;

/** The mark every relay page carries: most people who open a link do not have jelly.
 *  It is the relay page's own element, outside the view, so a view cannot remove or restyle it. */
export const MADE = `<a class="made" href="https://getjellyware.com" target="_blank" rel="noopener">Made with jelly</a>`;
export const MADE_CSS = `.made { font: 12px/1 -apple-system, system-ui, sans-serif; color: #7d606c; text-decoration: none; }
  .made:hover, .made:focus-visible { text-decoration: underline; }`;

/** The link page, titled with the jelly's about line so a pasted link previews as what it is.
 *  `report` is where a person reports this share; without it the page has no Report link. */
export const shell = (about: string, report?: string) => SHELL.replace("</footer>", report
  ? `<a class="made" href="${esc(report)}" target="_blank" rel="noopener">Report</a></footer>` : "</footer>").replace("<title>jelly</title>", `<title>${esc(about ? `${about} · jelly` : "jelly")}</title>
<meta property="og:title" content="${esc(about || "A jelly for you")}">
<meta property="og:description" content="A live page shared with you. Open it to use it: tap, answer, comment.">
${CARD}`);

const SHELL = String.raw`<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer"><meta name="robots" content="noindex, nofollow">
<title>jelly</title>
<style>
  html, body { margin: 0; height: 100%; background: #fff8f3; }
  body { display: flex; flex-direction: column; }
  iframe { border: 0; display: block; width: 100%; flex: 1; min-height: 0; }
  /* A strip under the view, not over it: nothing the view or the runtime draws can end up beneath it. */
  footer { flex: none; margin-top: auto; padding: 6px 12px; display: flex; flex-wrap: wrap; align-items: center; justify-content: center; gap: 6px 18px; border-top: 1px solid #f0dfe5; background: #fff8f3; }
  ${MADE_CSS}
  #room { display: flex; flex-wrap: wrap; align-items: center; justify-content: center; gap: 6px; font: 12px/1 -apple-system, system-ui, sans-serif; color: #7d606c; }
  #room[hidden] { display: none; }
  #room button, #room b { font: inherit; font-weight: 600; max-width: 14em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    border: 1px solid #e5cfd6; border-radius: 999px; padding: 5px 10px; background: #fff; color: #2a1620; cursor: pointer; }
  #room b { background: #2a1620; border-color: #2a1620; color: #fff8f3; cursor: default; }
  @media (prefers-color-scheme: dark) {
    footer { border-color: #2e2740; background: #14111c; }
    .made, #room { color: #a79dbb; }
    #room button { background: #1d1828; border-color: #3a3150; color: #e9e3f5; }
    #room b { background: #e9e3f5; border-color: #e9e3f5; color: #14111c; }
  }
  #note { position: fixed; left: 50%; bottom: 44px; transform: translateX(-50%); z-index: 1; max-width: calc(100% - 32px); box-sizing: border-box;
    background: #2a1620; color: #fff8f3; font: 14px -apple-system, system-ui, sans-serif; padding: 9px 15px; border-radius: 999px; }
  #note[hidden] { display: none; }
  body.over #note { position: static; transform: none; margin: 40vh auto 0; width: max-content; }
</style></head>
<body><iframe id="view" title="jelly" referrerpolicy="no-referrer"
  sandbox="allow-scripts allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox"></iframe><div id="note" role="status" hidden></div>
<footer><nav id="room" aria-label="Jellies in this room" hidden></nav>${MADE}</footer>
<script>
(() => {
  const frame = document.getElementById("view");
  const note = document.getElementById("note");
  const nav = document.getElementById("room");
  let doc, me, runtime, assets, viewRev, ws, n = 0, noteT, share, room = [];
  const pending = new Map();
  const post = (m) => frame.contentWindow && frame.contentWindow.postMessage(m, "*");
  const say = (text, stay) => {
    note.textContent = text; note.hidden = false; clearTimeout(noteT);
    if (!stay) noteT = setTimeout(() => { note.hidden = true; }, 3500);
  };
  const tag = (s) => "<script>" + s.replace(/<\/script/gi, "<\\/script") + "</" + "script>";
  function boot() {
    viewRev = doc.view_rev;
    document.title = doc.about || doc.name.split(":").pop();
    const b = { name: doc.name, about: doc.about, version: doc.version, viewRev: doc.view_rev, state: doc.state,
      comments: doc.comments, asks: doc.asks, unsent: doc.unsent || 0, share: me };
    // The base keeps assets/<file> working, and keeps this page's URL out of the view.
    const head = '<base href="' + assets + '">' + tag("window.__JELLY__=" + JSON.stringify(b).replace(/</g, "\\u003c")) + tag(runtime);
    const i = doc.view.search(/<head>/i);
    frame.srcdoc = i < 0 ? '<!doctype html><meta charset="utf-8">' + head + doc.view : doc.view.slice(0, i + 6) + head + doc.view.slice(i + 6);
  }
  // The room's jellies, this one marked. A jelly added while the page is open is said once.
  function showRoom(next, added) {
    const fresh = next.filter((j) => j.share !== share && !room.some((r) => r.share === j.share));
    if (added && fresh.length) say(fresh.map((j) => j.label).join(", ") + " is open in this room: tap it below.");
    room = next;
    nav.hidden = room.length < 2;
    nav.replaceChildren("In this room", ...room.map((j) => {
      const el = document.createElement(j.share === share ? "b" : "button");
      el.textContent = el.title = j.label;
      if (j.share === share) el.setAttribute("aria-current", "page");
      else el.onclick = () => go(j);
      return el;
    }));
  }
  async function go(j) {
    try {
      const r = await fetch("/room", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ from: location.pathname.split("/")[2], to: j.share }) });
      const body = await r.json().catch(() => ({}));
      if (r.ok && body.link) return location.assign(body.link);
      say(body.error || "That did not work. Try again.");
    } catch {
      say("Could not reach the jelly. Check your connection and try again.");
    }
  }
  function over(text) {
    frame.remove();
    nav.hidden = true;
    document.body.className = "over";
    say(text, true);
  }
  function connect() {
    ws = new WebSocket(location.href.split(/[?#]/)[0].replace(/^http/, "ws").replace(/\/$/, "") + "/ws");
    const ping = setInterval(() => ws.readyState === 1 && ws.send("ping"), 30000);
    ws.onmessage = (m) => {
      if (m.data === "pong") return;
      const msg = JSON.parse(m.data);
      if (msg.type === "hello") {
        ({ doc, me, runtime, assets, share } = msg);
        boot();
        showRoom(msg.room, false);
      } else if (msg.type === "reply") {
        const id = pending.get(msg.ref);
        pending.delete(msg.ref);
        if (!msg.ok && !me.can_act) say("This link is view only.");
        post({ jelly: "reply", id, ok: msg.ok, value: msg.value, error: msg.error });
      } else if (msg.type === "room") {
        showRoom(msg.room, true);
      } else if (msg.type === "events") {
        Object.assign(doc, msg.doc);
        if (doc.view_rev !== viewRev) return boot();
        for (const ev of msg.events) post({ jelly: "event", ev: { ...ev, jelly: doc.name, state: doc.state } });
      }
    };
    ws.onclose = (e) => {
      clearInterval(ping);
      for (const id of pending.values()) post({ jelly: "reply", id, ok: false, error: "The connection dropped. Try again." });
      pending.clear();
      if (e.code === 4403) return over("This link no longer works.");
      if (e.code === 4404) return over("This jelly is no longer shared.");
      setTimeout(connect, 2000);
    };
  }
  addEventListener("message", (e) => {
    const m = e.data;
    if (e.source !== frame.contentWindow || !m || m.jelly !== "rpc") return;
    if (m.op === "reload") return boot();
    if (m.op === "show") return post({ jelly: "reply", id: m.id, ok: true, value: { comments: doc.comments, asks: doc.asks, unsent: doc.unsent || 0 } });
    if (!ws || ws.readyState !== 1) return post({ jelly: "reply", id: m.id, ok: false, error: "Not connected. Try again in a moment." });
    pending.set(++n, m.id);
    ws.send(JSON.stringify({ ref: n, op: m.op, body: m.body }));
  });
  connect();
})();
</script></body></html>`;
