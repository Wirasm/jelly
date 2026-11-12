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
    // CSS escapes are decoded only inside URL tokens, never in strings or comments.
    return value.replace(/\/\*[\s\S]*?\*\/|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|(?<![\w-])url\(\s*(?:"((?:\\[\s\S]|[^"\\])*)"|'((?:\\[\s\S]|[^'\\])*)'|((?:\\(?:[0-9a-f]{1,6}(?:\r\n|[ \t\n\r\f])?|[^\n\r\f])|[^\s)'"\\])*))\s*\)/gi,
      (token, double, single, bare) => {
        const raw = double ?? single ?? bare;
        if (raw === undefined) return token;
        const url = raw.replace(/\\(?:([0-9a-f]{1,6})(?:\r\n|[ \t\n\r\f])?|(\r\n|[\s\S]))/gi, (_, hex, char) => {
          if (!hex) return /[\n\r\f]/.test(char) ? "" : char;
          const point = parseInt(hex, 16);
          return String.fromCodePoint(!point || point > 0x10ffff || (point >= 0xd800 && point <= 0xdfff) ? 0xfffd : point);
        });
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
  // Detached images can finish loading before ever joining the observed tree.
  for (const name of ["src", "srcset"]) {
    const descriptor = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, name);
    Object.defineProperty(HTMLImageElement.prototype, name, {
      ...descriptor,
      set(value) { descriptor.set.call(this, name === "srcset" ? srcset(String(value)) : resource(String(value))); },
    });
  }
  const setAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function(name, value) {
    const key = String(name).toLowerCase();
    if (this instanceof HTMLImageElement) {
      if (key === "src") value = resource(String(value));
      if (key === "srcset") value = srcset(String(value));
    }
    return setAttribute.call(this, name, value);
  };
  const setAttributeNS = Element.prototype.setAttributeNS;
  Element.prototype.setAttributeNS = function(namespace, name, value) {
    if (!namespace && this instanceof HTMLImageElement) return this.setAttribute(name, value);
    return setAttributeNS.call(this, namespace, name, value);
  };
  function reconcile(element) {
    for (const attr of [...element.attributes]) {
      if (!["src", "srcset", "href", "xlink:href", "poster", "style"].includes(attr.name)) continue;
      const value = attr.value;
      const resolved = attr.name === "style" ? css(value) : attr.name === "srcset" ? srcset(value) : resource(value);
      if (resolved !== value) {
        if (attr.namespaceURI) element.setAttributeNS(attr.namespaceURI, attr.name, resolved);
        else element.setAttribute(attr.name, resolved);
      }
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
  }).observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true });
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
