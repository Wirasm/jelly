// jelly for Pi: when the person presses Send in one of this project's jellies,
// the idle session starts a turn.
import { spawn, type ChildProcess } from "node:child_process";
import { basename } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	let events: ChildProcess | undefined;
	pi.on("session_start", async (_event, ctx) => {
		const top = await pi.exec("git", ["rev-parse", "--show-toplevel"]);
		const project = top.code === 0 ? basename(top.stdout.trim()) : "";
		events?.kill();
		events = spawn("jelly", ["events"], { stdio: ["ignore", "pipe", "ignore"] });
		let rest = "";
		events.stdout!.on("data", (chunk: Buffer) => {
			const lines = (rest + chunk).split("\n");
			rest = lines.pop() ?? "";
			for (const line of lines) {
				let ev: { jelly?: string; kind?: string; by?: string };
				try { ev = JSON.parse(line); } catch { continue; }
				if (!ev.jelly?.startsWith(`${project}:`) || ev.kind !== "send" || ev.by === "agent") continue;
				pi.sendUserMessage(`The person pressed Send on ${ev.jelly}: they are done there and hand the turn back. Run \`jelly waiting\` and \`jelly state ${ev.jelly}\` for what they did.`, { deliverAs: "followUp" });
			}
		});
		if (ctx.hasUI) ctx.ui.setStatus("jelly", `jelly: listening for Send in ${project}`);
	});
	pi.on("session_shutdown", async () => { events?.kill(); });
}
