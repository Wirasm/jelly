// The page a link opens. It holds the link's WebSocket to the shared jelly and
// shows the jelly's view in a sandboxed iframe with an opaque origin: the view
// cannot read this page, its URL (the link) or the socket, and reaches the
// jelly only through postMessage, which this page turns into the few things a
// link may do. The view runs the daemon's own runtime.js, sent by the daemon.
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
export const FAVICON = "data:image/svg+xml," + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><path d="M8 30 C4 22 6 10 20 9 C34 10 36 22 32 30 C29 35 11 35 8 30Z" fill="#ff4f8b"/><ellipse cx="15" cy="16" rx="4" ry="2.4" fill="#fff" opacity=".7"/><circle cx="16" cy="24" r="1.9" fill="#3b0a1e"/><circle cx="24" cy="24" r="1.9" fill="#3b0a1e"/></svg>`);

/** The link page, titled with the jelly's about line so a pasted link previews as what it is. */
export const shell = (about: string) => SHELL.replace("<title>jelly</title>", `<title>${esc(about ? `${about} · jelly` : "jelly")}</title>
<meta property="og:title" content="${esc(about || "A jelly for you")}">
<meta property="og:description" content="A live page shared with you. Open it to use it: tap, answer, comment.">
<meta property="og:site_name" content="jelly">
<meta property="og:image" content="https://getjellyware.com/og.png">
<meta property="og:image:width" content="1200"><meta property="og:image:height" content="630">
<meta name="twitter:card" content="summary_large_image">
<link rel="icon" href="${FAVICON}">
<link rel="apple-touch-icon" href="https://getjellyware.com/apple-touch-icon.png">`);

const SHELL = String.raw`<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer"><meta name="robots" content="noindex, nofollow">
<title>jelly</title>
<style>
  html, body { margin: 0; height: 100%; background: #fff8f3; }
  iframe { border: 0; display: block; width: 100%; height: 100%; }
  #note { position: fixed; left: 50%; bottom: 16px; transform: translateX(-50%); z-index: 1; max-width: calc(100% - 32px); box-sizing: border-box;
    background: #2a1620; color: #fff8f3; font: 14px -apple-system, system-ui, sans-serif; padding: 9px 15px; border-radius: 999px; }
  #note[hidden] { display: none; }
  body.over #note { position: static; transform: none; margin: 40vh auto 0; width: max-content; }
</style></head>
<body><iframe id="view" title="jelly" referrerpolicy="no-referrer"
  sandbox="allow-scripts allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox"></iframe><div id="note" role="status" hidden></div>
<script>
(() => {
  const frame = document.getElementById("view");
  const note = document.getElementById("note");
  let doc, me, runtime, assets, viewRev, ws, n = 0, noteT;
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
      comments: doc.comments, attention: doc.attention, share: me };
    // The base keeps assets/<file> working, and keeps this page's URL out of the view.
    const head = '<base href="' + assets + '">' + tag("window.__JELLY__=" + JSON.stringify(b).replace(/</g, "\\u003c")) + tag(runtime);
    const i = doc.view.search(/<head>/i);
    frame.srcdoc = i < 0 ? '<!doctype html><meta charset="utf-8">' + head + doc.view : doc.view.slice(0, i + 6) + head + doc.view.slice(i + 6);
  }
  function over(text) {
    frame.remove();
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
        ({ doc, me, runtime, assets } = msg);
        boot();
      } else if (msg.type === "reply") {
        const id = pending.get(msg.ref);
        pending.delete(msg.ref);
        if (!msg.ok && !me.can_act) say("This link is view only.");
        post({ jelly: "reply", id, ok: msg.ok, value: msg.value, error: msg.error });
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
    if (m.op === "show") return post({ jelly: "reply", id: m.id, ok: true, value: { comments: doc.comments, attention: doc.attention } });
    if (!ws || ws.readyState !== 1) return post({ jelly: "reply", id: m.id, ok: false, error: "Not connected. Try again in a moment." });
    pending.set(++n, m.id);
    ws.send(JSON.stringify({ ref: n, op: m.op, body: m.body }));
  });
  connect();
})();
</script></body></html>`;
