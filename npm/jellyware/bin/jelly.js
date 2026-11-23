#!/usr/bin/env node
// Starts the jelly binary that npm installed for this platform. The binary is
// in its own package (@getjellyware/jelly-darwin-arm64 and so on), picked by npm from
// this package's optionalDependencies.
const { spawn } = require("node:child_process");

const name = `@getjellyware/jelly-${process.platform}-${process.arch}`;
let binary;
try {
  binary = require.resolve(`${name}/bin/jelly${process.platform === "win32" ? ".exe" : ""}`);
} catch {
  console.error(`jelly has no prebuilt binary for ${process.platform} ${process.arch} (${name} is not installed).\nBuild it from source: https://github.com/Wirasm/jelly#set-it-up`);
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });
// A wait such as `jelly hear --wait` must stop when the agent stops this process.
for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) process.on(signal, () => child.kill(signal));
child.on("error", (err) => { console.error(`jelly could not start: ${err.message}`); process.exit(1); });
child.on("exit", (code, signal) => (signal ? process.kill(process.pid, signal) : process.exit(code ?? 1)));
