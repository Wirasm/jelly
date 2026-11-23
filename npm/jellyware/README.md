# jellyware

Show your user a live page instead of a wall of text. A jelly is a small app an
agent makes in seconds and both of you change while it runs: the person clicks,
picks and comments in the browser, and the agent reads what they did.

Use it when a picture, a prototype, a dashboard, a plan, a diff or a choice
between options explains better than prose, or when you need the person to
decide something.

```bash
npx jellyware setup
```

That installs the `jelly` command (a prebuilt binary, no Rust needed) and writes
the jelly skill for Claude Code, Codex and Pi, whichever are on the machine.
It is the same as `npm install -g jellyware`, then `jelly setup`.

After it, the agent knows when and how to use jelly. Any other agent
that runs shell commands can use the `jelly` command directly:

```bash
jelly recipes                    # starting points: plan, review, compare, monitor and more
jelly make plan --recipe plan    # make a jelly from one
jelly ask plan "Ship it?" --option yes="Ship" --option no="Wait"
jelly hear --wait                # returns when the person presses Send
```

The daemon starts by itself and serves the jellies at http://127.0.0.1:7878.

This package holds a launcher. The `jelly` binary for your platform comes from
one of its optional dependencies: macOS and Linux on arm64 and x64, and Windows
on x64. On another platform, build from source.

Docs and source: https://github.com/Wirasm/jelly
