// Stands in for runtime.js in a shared snapshot: the state is baked in, and
// nothing can change it. Views render exactly as they did when it was shared.
(() => {
  const boot = window.__JELLY__;
  const assets = new Map(boot.assets);
  const assetBase = new URL("assets/", document.baseURI);
  function resource(value) {
    let url;
    try { url = new URL(value, document.baseURI); } catch { return value; }
    if (url.protocol !== assetBase.protocol || url.host !== assetBase.host || !url.pathname.startsWith(assetBase.pathname)) return value;
    let file;
    try { file = decodeURIComponent(url.pathname.slice(assetBase.pathname.length)); } catch { return value; }
    const uri = assets.get(file);
    return uri ? uri + url.hash : value;
  }
  function css(value) {
    // Skip comments and strings so only URL tokens are resolved.
    return value.replace(/\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|(?<![\w-])url\(\s*(?:"([^"\\]*)"|'([^'\\]*)'|([^\s)'"\\]*))\s*\)/gi,
      (token, double, single, bare) => {
        const url = double ?? single ?? bare;
        if (url === undefined) return token;
        const resolved = resource(url);
        return resolved === url ? token : `url("${resolved}")`;
      });
  }
  function srcset(value) {
    // A URL can contain commas (data URIs); descriptors end at the next comma.
    return value.replace(/([^\s,]\S*)(\s+[^,]*)?/g, (candidate, url, descriptor = "") => {
      const clean = url.replace(/,+$/, "");
      return resource(clean) + url.slice(clean.length) + descriptor;
    });
  }
  function reconcile(element) {
    for (const attr of ["src", "srcset", "href", "xlink:href", "poster", "style"]) {
      const value = element.getAttribute(attr);
      if (value === null) continue;
      const resolved = attr === "style" ? css(value) : attr === "srcset" ? srcset(value) : resource(value);
      if (resolved !== value) element.setAttribute(attr, resolved);
    }
    if (element.localName === "style") {
      const value = element.textContent;
      const resolved = css(value);
      if (resolved !== value) element.textContent = resolved;
    }
  }
  function subtree(node) {
    if (node.nodeType !== Node.ELEMENT_NODE) return;
    reconcile(node);
    node.querySelectorAll("*").forEach(reconcile);
  }
  new MutationObserver((changes) => {
    for (const change of changes) {
      if (change.type === "attributes") reconcile(change.target);
      else {
        const parent = change.target.nodeType === Node.TEXT_NODE ? change.target.parentElement : change.target;
        if (parent?.localName === "style") reconcile(parent);
        change.addedNodes.forEach(subtree);
      }
    }
  }).observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true,
    attributeFilter: ["src", "srcset", "href", "xlink:href", "poster", "style"] });
  subtree(document.documentElement);
  const fetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    const url = input instanceof Request ? input.url : String(input);
    const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
    const resolved = resource(url);
    if (resolved === url || !["GET", "HEAD"].includes(method)) return fetch(input, init);
    return fetch(input instanceof Request ? new Request(resolved, input) : resolved, init);
  };
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
    asset: (file) => assets.get(file) ?? `assets/${encodeURIComponent(file)}`,
    get items() { return boot.attention; },
    get attention() { return boot.attention.filter((a) => !a.answer && !a.withdrawn); },
    itemsAt: (selector) => boot.attention.filter((a) => a.anchor?.selector === selector && !a.withdrawn).reverse(),
    onAttention(fn) { fn(); },
    comments: [],
    feedback: [],
    onState(fn) { fn(boot.state, null); },
    onAttend() { return () => {}; },
    events() { return () => {}; },
    watch() { return () => {}; },
    call: readOnly,
    read: readOnly,
    comment: readOnly,
    reply: readOnly,
    resolve: readOnly,
    answer: readOnly,
    focus() { note(); },
    show() { note(); },
  };
})();
