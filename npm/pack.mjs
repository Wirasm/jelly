// Writes the npm packages for one release into an output folder, ready for
// `npm publish`: one package per platform holding the binary, and `jellyware`,
// whose launcher starts the one npm installed.
//
//   node npm/pack.mjs platform <version> <rust-target> <binary> <out>
//   node npm/pack.mjs main <version> <out>
import { chmodSync, copyFileSync, cpSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

// Rust target -> the names Node uses for the same platform.
export const TARGETS = {
  "aarch64-apple-darwin": { os: "darwin", cpu: "arm64" },
  "x86_64-apple-darwin": { os: "darwin", cpu: "x64" },
  "aarch64-unknown-linux-gnu": { os: "linux", cpu: "arm64" },
  "x86_64-unknown-linux-gnu": { os: "linux", cpu: "x64" },
  "x86_64-pc-windows-msvc": { os: "win32", cpu: "x64" },
};
// Scoped, because npm refuses unscoped names such as jellyware-win32-x64 as spam.
const folder = ({ os, cpu }) => `jelly-${os}-${cpu}`;
const packageName = (p) => `@getjellyware/${folder(p)}`;
const shared = {
  license: "FSL-1.1-MIT",
  repository: { type: "git", url: "git+https://github.com/Wirasm/jelly.git" },
  homepage: "https://getjellyware.com",
};
const write = (dir, pkg) => writeFileSync(join(dir, "package.json"), JSON.stringify({ ...pkg, ...shared }, null, 2) + "\n");
const here = dirname(fileURLToPath(import.meta.url));

const [kind, version, ...rest] = process.argv.slice(2);
if (kind === "platform") {
  const [target, binary, out] = rest;
  const platform = TARGETS[target];
  if (!platform) throw new Error(`no npm platform for ${target}; there are ${Object.keys(TARGETS).join(", ")}`);
  const dir = join(out, folder(platform));
  mkdirSync(join(dir, "bin"), { recursive: true });
  const file = join(dir, "bin", platform.os === "win32" ? "jelly.exe" : "jelly");
  copyFileSync(binary, file);
  chmodSync(file, 0o755);
  copyFileSync(join(here, "..", "LICENSE.md"), join(dir, "LICENSE.md"));
  write(dir, {
    name: packageName(platform), version,
    description: `The jelly binary for ${platform.os} ${platform.cpu}. Install jellyware instead.`,
    os: [platform.os], cpu: [platform.cpu], files: ["bin"],
  });
} else if (kind === "main") {
  const [out] = rest;
  const dir = join(out, "jellyware");
  cpSync(join(here, "jellyware"), dir, { recursive: true });
  copyFileSync(join(here, "..", "LICENSE.md"), join(dir, "LICENSE.md"));
  write(dir, {
    name: "jellyware", version,
    description: "Show your user a live page instead of a wall of text: plans, choices, dashboards, prototypes. For Claude Code, Codex, Pi and any agent with a shell.",
    keywords: ["agent", "ai-agent", "coding-agent", "claude-code", "codex", "agent-skill", "human-in-the-loop",
      "generative-ui", "agent-ui", "live-preview", "dashboard", "prototype", "cli"],
    bin: { jelly: "bin/jelly.js" }, files: ["bin"], engines: { node: ">=18" },
    optionalDependencies: Object.fromEntries(Object.values(TARGETS).map((p) => [packageName(p), version])),
  });
} else {
  throw new Error("usage: pack.mjs platform <version> <rust-target> <binary> <out> | main <version> <out>");
}
