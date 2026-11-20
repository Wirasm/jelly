import { defineConfig } from "astro/config";

// A static site: Cloudflare serves dist/ as Workers static assets (wrangler.jsonc).
export default defineConfig({
  site: "https://getjellyware.com",
});
