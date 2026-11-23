// The page at /pin: a person types the PIN they were given with their email,
// and goes on to their own link (/j/...), the same kind `--to` mints.
import { CARD, FAVICON, MADE, MADE_CSS } from "./shell";

export const joinPage = String.raw`<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="referrer" content="no-referrer"><meta name="robots" content="noindex, nofollow">
<title>Join a jelly</title>
<meta property="og:title" content="Join a jelly">
<meta property="og:description" content="Type the PIN you were given and your email.">
${CARD}
<style>
  * { box-sizing: border-box; }
  html, body { margin: 0; min-height: 100%; background: #fff8f3; color: #2a1620; }
  body { font: 16px/1.45 -apple-system, system-ui, sans-serif; display: grid; place-items: start center; padding: 12vh 16px 32px; }
  main { width: 100%; max-width: 360px; }
  img { width: 56px; height: 56px; display: block; margin-bottom: 12px; }
  h1 { font-size: 26px; line-height: 1.2; margin: 0 0 6px; }
  p { margin: 0 0 22px; color: #6b4a58; }
  label { display: block; font-weight: 600; font-size: 14px; margin: 0 0 6px; }
  input { width: 100%; font: inherit; color: inherit; background: #fff; border: 1.5px solid #e5cfd6; border-radius: 12px; padding: 12px 14px; margin: 0 0 16px; }
  input:focus { outline: none; border-color: #ff4f8b; box-shadow: 0 0 0 3px #ff4f8b33; }
  #pin { font-size: 28px; letter-spacing: .35em; text-align: center; font-variant-numeric: tabular-nums; padding-left: calc(14px + .35em); }
  button { width: 100%; font: inherit; font-weight: 600; color: #fff; background: #ff4f8b; border: 0; border-radius: 12px; padding: 13px; cursor: pointer; }
  button:disabled { opacity: .6; cursor: default; }
  #error { min-height: 1.45em; margin: 14px 0 0; color: #b3123f; }
  footer { margin-top: 28px; text-align: center; }
  ${MADE_CSS}
</style></head>
<body><main>
<img alt="" src="${FAVICON}">
<h1>Join a jelly</h1>
<p>Type the PIN you were given and your email. Your email is your name in it.</p>
<form id="join" novalidate>
  <label for="pin">PIN</label>
  <input id="pin" name="pin" inputmode="numeric" autocomplete="off" maxlength="6" pattern="[0-9]{6}" placeholder="000000" required autofocus>
  <label for="email">Email</label>
  <input id="email" name="email" type="email" autocomplete="email" autocapitalize="off" spellcheck="false" placeholder="ana@example.com" required>
  <button type="submit">Join</button>
  <p id="error" role="alert"></p>
</form>
<footer>${MADE}</footer>
</main>
<script>
(() => {
  const form = document.getElementById("join");
  const error = document.getElementById("error");
  const button = form.querySelector("button");
  const pin = document.getElementById("pin");
  pin.addEventListener("input", () => { pin.value = pin.value.replace(/\D/g, "").slice(0, 6); });
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    error.textContent = "";
    button.disabled = true;
    try {
      const r = await fetch("/pin", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ pin: pin.value, email: form.email.value }) });
      const body = await r.json().catch(() => ({}));
      if (r.ok && body.link) return location.replace(body.link);
      error.textContent = body.error || "That did not work. Try again.";
    } catch {
      error.textContent = "Could not reach the jelly. Check your connection and try again.";
    }
    button.disabled = false;
  });
})();
</script></body></html>`;
