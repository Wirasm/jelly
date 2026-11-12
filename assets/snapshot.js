// Stands in for runtime.js in a shared snapshot: the state is baked in, and
// nothing can change it. Views render exactly as they did when it was shared.
(() => {
  const boot = window.__JELLY__;
  const assets = new Map(boot.assets);
  const stylesheets = new Map();
  const assetBase = new URL("assets/", document.baseURI);
  function resource(value, base = document.baseURI, imports = []) {
    let url;
    try { url = new URL(value, stylesheets.get(base)?.url ?? base); } catch { return value; }
    if (url.protocol !== assetBase.protocol || url.host !== assetBase.host || !url.pathname.startsWith(assetBase.pathname)) return value;
    let file;
    try { file = decodeURIComponent(url.pathname.slice(assetBase.pathname.length)); } catch { return value; }
    let uri = assets.get(file);
    if (uri?.startsWith("data:text/css;")) {
      // CSS data URLs have no directory: resolve nested resources against the uploaded file.
      // Browsers ignore cyclic imports; the embedded graph must terminate too.
      if (imports.includes(file)) return "data:text/css,";
      const source = uri;
      const bytes = Uint8Array.from(atob(uri.split(",")[1]), c => c.charCodeAt(0));
      const text = css(new TextDecoder().decode(bytes), url, [...imports, file]);
      uri = "data:text/css;charset=utf-8," + encodeURIComponent(text);
      stylesheets.set(uri + url.hash, {url: url.href, source});
    }
    return uri ? uri + url.hash : value;
  }
  function unescapeCss(value) {
    return value.replace(/\\(?:([0-9a-f]{1,6})(?:\r\n|[ \t\n\r\f])?|(\r\n|[\s\S]))/gi, (_, hex, char) => {
      if (!hex) return /[\n\r\f]/.test(char) ? "" : char;
      const point = parseInt(hex, 16);
      return String.fromCodePoint(!point || point > 0x10ffff || (point >= 0xd800 && point <= 0xdfff) ? 0xfffd : point);
    });
  }
  function css(value, base = document.baseURI, imports = []) {
    const tokens = /\/\*[\s\S]*?\*\/|"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|@?(?:[-\w\u0080-\uffff]|\\(?:[0-9a-f]{1,6}(?:\r\n|[ \t\n\r\f])?|[^\n\r\f]))+\(?|\s+|[\s\S]/gi;
    const functions = [];
    let importing = false, result = "", match;
    while ((match = tokens.exec(value))) {
      let token = match[0];
      const decoded = unescapeCss(token).toLowerCase();
      const context = functions.at(-1);
      if (/^\s|^\/\*/.test(token)) { result += token; continue; }
      if (decoded === "url(") {
        const argument = /^\s*(?:"((?:\\[\s\S]|[^"\\])*)"|'((?:\\[\s\S]|[^'\\])*)'|((?:\\(?:[0-9a-f]{1,6}(?:\r\n|[ \t\n\r\f])?|[^\n\r\f])|[^\s)'"\\])*))\s*\)/i.exec(value.slice(tokens.lastIndex));
        if (argument) {
          const url = unescapeCss(argument[1] ?? argument[2] ?? argument[3]);
          const resolved = resource(url, base, imports);
          token = resolved === url ? token + argument[0] : `url("${resolved}")`;
          tokens.lastIndex += argument[0].length;
        }
      } else if (/^["']/.test(token) && (importing || context?.candidate)) {
        const url = unescapeCss(token.slice(1, -1));
        const resolved = resource(url, base, imports);
        if (resolved !== url) token = `"${resolved}"`;
      }
      if (context) context.candidate = decoded === "," && context.imageSet;
      if (decoded.endsWith("(") && decoded !== "url(") {
        const imageSet = ["image-set(", "-webkit-image-set("].includes(decoded);
        functions.push({imageSet, candidate: imageSet});
      } else if (decoded === ")") functions.pop();
      importing = decoded === "@import";
      result += token;
    }
    return result;
  }
  for (const prototype of [CSSStyleSheet.prototype, CSSGroupingRule.prototype]) {
    const insertRule = prototype.insertRule;
    prototype.insertRule = function(rule, index) {
      return insertRule.call(this, css(String(rule), this.href || this.parentStyleSheet?.href || document.baseURI), index);
    };
  }
  for (const name of ["replace", "replaceSync"]) {
    const replace = CSSStyleSheet.prototype[name];
    CSSStyleSheet.prototype[name] = function(text) {
      return replace.call(this, css(String(text)));
    };
  }
  function declaration(value, style) {
    return typeof value === "string" ? css(value, style.parentRule?.parentStyleSheet?.href || document.baseURI) : value;
  }
  const setProperty = CSSStyleDeclaration.prototype.setProperty;
  CSSStyleDeclaration.prototype.setProperty = function(name, value, priority) {
    return setProperty.call(this, name, declaration(value, this), priority);
  };
  const ruleStyle = Object.getOwnPropertyDescriptor(CSSStyleRule.prototype, "style");
  const styles = new WeakMap();
  Object.defineProperty(CSSStyleRule.prototype, "style", {
    ...ruleStyle,
    set(value) { ruleStyle.set.call(this, declaration(value, ruleStyle.get.call(this))); },
    get() {
      const style = ruleStyle.get.call(this);
      if (!styles.has(style)) styles.set(style, new Proxy(style, {
        get(target, key) {
          const value = Reflect.get(target, key, target);
          return typeof value === "function" ? value.bind(target) : value;
        },
        set(target, key, value) { return Reflect.set(target, key, declaration(value, target), target); },
      }));
      return styles.get(style);
    },
  });
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
      if (!["src", "srcset", "href", "xlink:href", "poster", "style", "data"].includes(attr.name)) continue;
      if (attr.name === "data" && element.localName !== "object") continue;
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
  const observer = new MutationObserver((changes) => {
    for (const change of changes) {
      if (change.type === "attributes") reconcile(change.target);
      else {
        const parent = change.target.nodeType === Node.TEXT_NODE ? change.target.parentElement : change.target;
        if (parent?.localName === "style") reconcile(parent);
        change.addedNodes.forEach(subtree);
      }
    }
  });
  function observe(root) {
    observer.observe(root, { subtree: true, childList: true, characterData: true, attributes: true });
  }
  const attachShadow = Element.prototype.attachShadow;
  Element.prototype.attachShadow = function(options) {
    const root = attachShadow.call(this, options);
    observe(root);
    return root;
  };
  observe(document.documentElement);
  subtree(document.documentElement);
  const fetch = window.fetch.bind(window);
  window.fetch = (input, init) => {
    const url = input instanceof Request ? input.url : String(input);
    const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
    const resolved = resource(url);
    const source = stylesheets.get(resolved)?.source ?? resolved;
    if (source === url || !["GET", "HEAD"].includes(method)) return fetch(input, init);
    return fetch(input instanceof Request ? new Request(source, input) : source, init);
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
    asset: (file) => resource(`assets/${encodeURIComponent(file)}`),
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
