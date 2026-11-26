// jelly for Pi: when the person presses Send in one of this project's jellies,
// the idle session starts a turn.
import { spawn, type ChildProcess } from "node:child_process";
import { basename } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	let waiting: ChildProcess | undefined;
	let stopped = false;
	pi.on("session_start", async (_event, ctx) => {
		const top = await pi.exec("git", ["rev-parse", "--show-toplevel"]);
		const project = top.code === 0 ? basename(top.stdout.trim()) : "";
		// One `jelly hear --wait` at a time, each from the Send the last one
		// returned, so a Send between two waits is still heard.
		let since: string | undefined;
		const listen = () => {
			if (stopped) return;
			const proc = spawn("jelly", ["hear", "--wait", "--json", ...(since ? ["--since", since] : [])], { stdio: ["ignore", "pipe", "ignore"] });
			waiting = proc;
			let out = "";
			proc.stdout!.on("data", (chunk: Buffer) => { out += chunk; });
			proc.on("close", (code) => {
				if (stopped || waiting !== proc) return;
				if (code !== 0) { setTimeout(listen, 5000); return; } // no daemon yet: try again
				const ev: { seq: number; jelly: string } = JSON.parse(out);
				since = `seq:${ev.seq}`;
				if (ev.jelly.startsWith(`${project}:`)) {
					pi.sendUserMessage(`The person pressed Send on ${ev.jelly}: they are done there and hand the turn back. Run \`jelly waiting\` and \`jelly state ${ev.jelly}\` for what they did.`, { deliverAs: "followUp" });
				}
				listen();
			});
		};
		waiting?.kill();
		waiting = undefined;
		listen();
		if (ctx.hasUI) ctx.ui.setStatus("jelly", `jelly: listening for Send in ${project}`);
	});
	pi.on("session_shutdown", async () => { stopped = true; waiting?.kill(); });
}
