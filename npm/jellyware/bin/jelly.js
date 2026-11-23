#!/usr/bin/env node
// Starts the jelly binary that npm installed for this platform. The binary is
// in its own package (@getjellyware/jelly-darwin-arm64 and so on), picked by npm from
// this package's optionalDependencies.
const { spawn, spawnSync } = require("node:child_process");

const name = `@getjellyware/jelly-${process.platform}-${process.arch}`;
let binary;
try {
  binary = require.resolve(`${name}/bin/jelly${process.platform === "win32" ? ".exe" : ""}`);
} catch {
  console.error(`jelly has no prebuilt binary for ${process.platform} ${process.arch} (${name} is not installed).\nBuild it from source: https://github.com/Wirasm/jelly#set-it-up`);
  process.exit(1);
}

// `npx jellyware setup` runs from npm's temporary folder, and the hooks setup
// writes call `jelly` by name: install it for good first.
if (process.argv[2] === "setup" && !process.argv.includes("--dry-run") && __dirname.split(/[\\/]/).includes("_npx")) {
  const { version } = require("../package.json");
  console.error(`Installing jellyware ${version}, so the \`jelly\` command stays after npx is done`);
  const npm = spawnSync("npm", ["install", "-g", `jellyware@${version}`], { stdio: "inherit", shell: process.platform === "win32" });
  if (npm.status !== 0) {
    console.error("jelly was not installed. Run `npm install -g jellyware` yourself, then `jelly setup`.");
    process.exit(1);
  }
}

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });
// A wait such as `jelly hear --wait` must stop when the agent stops this process.
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(signal, () => child.kill(signal));
child.on("error", (err) => { console.error(`jelly could not start: ${err.message}`); process.exit(1); });
child.on("exit", (code, signal) => (signal ? process.kill(process.pid, signal) : process.exit(code ?? 1)));
