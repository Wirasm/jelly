// Stands in for runtime.js in a shared snapshot: the state is baked in, and
// nothing can change it. Views render exactly as they did when it was shared.
(() => {
  const boot = window.__JELLY__;
  const readOnly = () => {
    note();
    return Promise.reject(new Error("This is a read-only snapshot of a jelly."));
  };
  let shown = false;
  function note() {
    if (shown || !document.body) return;
    shown = true;
    const n = document.createElement("div");
    n.textContent = "Read-only snapshot. Clicks that change things do nothing here.";
    n.style.cssText = "position:fixed;left:50%;bottom:16px;transform:translateX(-50%);z-index:2147483647;background:#1a2021;color:#e8eeec;font:13px -apple-system,system-ui,sans-serif;padding:8px 14px;border-radius:999px;box-shadow:0 8px 24px rgba(0,0,0,.35)";
    document.body.appendChild(n);
    setTimeout(() => n.remove(), 3500);
    setTimeout(() => (shown = false), 3600);
  }
  window.jelly = {
    name: boot.name,
    about: boot.about,
    get state() { return boot.state; },
    feedback: [],
    onState(fn) { fn(boot.state, null); },
    events() { return () => {}; },
    watch() { return () => {}; },
    call: readOnly,
    read: readOnly,
    comment: readOnly,
    reply: readOnly,
    resolve: readOnly,
    show() { note(); },
  };
})();
