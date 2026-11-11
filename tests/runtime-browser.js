// Minimal browser boundary for executing the complete bundled runtime in QuickJS.
// Timers, requests and WebSocket messages are driven explicitly by the Rust test.
globalThis.window = globalThis;
window.parent = window;
const handlers = {};
const timers = [];
const requests = [];
let reloads = 0;
let responseStatus = 204;
const location = { search: '', protocol: 'http:', host: 'localhost', reload() { reloads++; } };
const console = { error() {} };
class URLSearchParams { has() { return false; } }
class WebSocket { constructor() { globalThis.socket = this; } }
class MutationObserver { observe() {} }
const document = {
  body: { children: [] },
  documentElement: { scrollHeight: 900 },
  createElement() { return {}; },
};
const innerWidth = 1280, innerHeight = 900;
function addEventListener(type, fn) { (handlers[type] ||= []).push(fn); }
function setTimeout(fn) { timers.push(fn); return timers.length; }
function fetch(url, options) {
  requests.push({ url, body: JSON.parse(options.body) });
  return Promise.resolve({ status: responseStatus });
}
function loaded() {
  for (const fn of handlers.load) fn();
  while (timers.length) timers.shift()();
}
function message(event) { socket.onmessage({ data: JSON.stringify(event) }); }
