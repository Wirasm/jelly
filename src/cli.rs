//! The command line: every command an agent can run, with its help text.

use crate::startup;
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "jelly", version = crate::update::VERSION, about = "Small live apps that agents and people shape while they run")]
pub struct Cli {
    /// Daemon address for client commands.
    #[arg(long, env = "JELLY_URL", default_value = startup::DEFAULT_URL, global = true)]
    pub url: String,
    /// Who this change is from, recorded on every event (default: agent).
    #[arg(long = "as", env = "JELLY_AS", global = true)]
    pub by: Option<String>,
    /// The project new jellies belong to. Defaults to the name of the git repo
    /// you run from; the jelly page shows each project on its own page.
    #[arg(long, env = "JELLY_PROJECT", global = true)]
    pub project: Option<String>,
    /// Print JSON instead of text, for scripts. Changes print the daemon's event
    /// (seq, version, changed paths, the item's id for asks); lists print
    /// an array. Commands with nothing to report as JSON refuse it.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Run the daemon and the jelly page.
    Serve {
        /// Default: 7878, or a free port with --temp.
        #[arg(long)]
        port: Option<u16>,
        /// Where jellies are kept. Defaults to $JELLY_HOME or ~/.jelly.
        #[arg(long)]
        home: Option<PathBuf>,
        /// A throwaway daemon: a free port and a fresh home in the temp dir, removed
        /// when it stops (Ctrl-C or SIGTERM). The URL is on the first line.
        #[arg(long, conflicts_with = "home")]
        temp: bool,
    },
    /// Find jellies by words in their name or about line, in every project, archived
    /// ones included. Prints a short reference for each: what it is, who made it and
    /// when, and what waits on the person. Use it when someone names a jelly loosely.
    Find {
        #[arg(required = true)]
        words: Vec<String>,
    },
    /// List this project's jellies, one per line (all projects outside a repo).
    Ls {
        /// Every project, archived jellies included.
        #[arg(long)]
        all: bool,
    },
    /// Make a jelly from a view and verbs. Use - to read a file from stdin.
    Make {
        name: String,
        /// Start from a folder with view.html, verbs.js, state.json and assets/,
        /// such as what `jelly pull` or `jelly recipes NAME --out` wrote.
        /// --view, --verbs and --state replace the folder's own.
        #[arg(long, conflicts_with = "recipe")]
        from: Option<PathBuf>,
        /// Start from a built-in recipe (see `jelly recipes`). --view, --verbs and
        /// --state replace the recipe's own; its example assets come only with
        /// its example state.
        #[arg(long)]
        recipe: Option<String>,
        #[arg(long, required_unless_present_any = ["from", "recipe"])]
        view: Option<PathBuf>,
        #[arg(long, required_unless_present_any = ["from", "recipe"])]
        verbs: Option<PathBuf>,
        /// Starting state as a JSON file.
        #[arg(long)]
        state: Option<PathBuf>,
        /// Replace an existing jelly of the same name, losing its state.
        #[arg(long)]
        replace: bool,
        /// One line on what this jelly is for, shown above it on the jelly page.
        #[arg(long, default_value = "")]
        about: String,
    },
    /// Set jelly up for the coding agents on this machine: Claude Code, Codex
    /// and Pi. Each gets the jelly skill; Claude Code also gets the hook that
    /// reports what the person did and the one that asks for a plan jelly when
    /// they approve a plan, and Pi the extension that starts a turn when they
    /// press Send. `jelly update` runs it for you. A skill folder that is a
    /// link (your own checkout) is left alone.
    Setup {
        /// Print what would be written, and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Update jelly to the latest release. Installed with npm: installs it, runs
    /// `setup` again so every agent's skill is the new one, and restarts the
    /// daemon on the new build. Built from source: prints the commands to run.
    /// Already the latest: says so, and restarts a daemon that still runs an
    /// older build. Read the skill again afterwards.
    Update,
    /// Check this machine, one line per check: `ok`, `fix` with the command
    /// that fixes it, or `--` for what could not be told. Checks jelly against
    /// the latest release, the daemon's build, the browser for `look` and
    /// `try`, and each agent's skill, hook and extension. Exit 1 when a line
    /// says `fix`. --json prints [{ok, text, fix}].
    Doctor,
    /// What Claude Code's hook runs when the person approves a plan: it tells
    /// the agent to show the plan as a jelly. `jelly setup` writes the hook.
    #[command(hide = true)]
    PlanApproved,
    /// List the built-in recipes: starting points for jellies that are easy to
    /// get wrong. With a NAME, print its RECIPE.md; with --out, write its files.
    Recipes {
        name: Option<String>,
        /// Write the recipe's files into this folder to edit, then `jelly make NAME --from DIR`.
        #[arg(long, requires = "name")]
        out: Option<PathBuf>,
    },
    /// Call a verb, exactly as a button in the view would.
    Do {
        name: String,
        verb: String,
        /// Arguments as JSON, @FILE to read a file, or - for stdin.
        #[arg(default_value = "{}")]
        args: String,
    },
    /// Write state directly, without a verb: fix your own content or start over.
    /// Undoable. `jelly set plan '{"items":[]}'` replaces the whole state.
    Set {
        name: String,
        /// JSON, @FILE to read a file, or - for stdin. Leave out with --rm.
        value: Option<String>,
        /// Where in the state, as a JSON pointer: /title, /items/0, /items/- (append).
        #[arg(long, default_value = "")]
        at: String,
        /// Remove what is at --at instead.
        #[arg(long)]
        rm: bool,
    },
    /// Print state, or the part at a JSON pointer such as /labels.
    State { name: String, pointer: Option<String> },
    /// Swap the view and/or verbs while the jelly runs. State is kept.
    Shape {
        name: String,
        #[arg(long)]
        view: Option<PathBuf>,
        #[arg(long)]
        verbs: Option<PathBuf>,
        /// New one-line description, shown above it on the jelly page.
        #[arg(long)]
        about: Option<String>,
    },
    /// Undo the last change to the view, verbs or state, by anyone. Comments,
    /// answers and keeps are not undone.
    Undo { name: String },
    /// Keep a jelly: it stays on the jelly page instead of being put away after a quiet week.
    Keep { name: String },
    /// Stop keeping a jelly.
    Unkeep { name: String },
    /// Remove a jelly and its history.
    Rm { name: String },
    /// Stream events as JSON lines, for one jelly or all.
    Events {
        name: Option<String>,
        /// Include the full state in each event.
        #[arg(long)]
        state: bool,
    },
    /// Open the jelly page (this project's, if any), or one jelly, in the browser.
    /// Always prints the address, even if the browser cannot be opened.
    Open { name: Option<String> },
    /// Ask the person something: one question on a jelly, or a set of them.
    ///
    /// Three forms:
    ///
    /// `jelly ask NAME "Which header?" --option a="Bold" --option b="Quiet" --recommend b --why "…" --at "#hero"`
    /// puts one ask on an existing jelly, next to what it is about. It shows on
    /// the jelly and in the person's attention queue. With `--wait` it blocks
    /// until the person answers and prints `a1 answered: <option> · "<note>"`.
    /// With --json it prints the ask's event, or with --wait the answered item.
    ///
    /// `jelly ask NAME "Check the footer links" --seen --at "#footer"` asks the
    /// person to look at something; they answer "seen" or with a word back.
    ///
    /// `jelly ask NAME --spec spec.json` makes an ask page: one or more questions,
    /// each an ask with options (label, does, gains, costs, effort, a
    /// live HTML preview), a recommendation and why. Asking again with the same
    /// name starts a new round. Spec: {"title", "context", "questions": [{"id",
    /// "question", "context", "recommend", "why", "options": [...]}]}.
    Ask {
        name: String,
        /// The question, for an ask on an existing jelly.
        question: Option<String>,
        /// A spec file for an ask page.
        #[arg(long, conflicts_with = "question")]
        spec: Option<PathBuf>,
        /// An option as id=label, or id=label|what picking it does. Repeat it.
        #[arg(long = "option")]
        options: Vec<String>,
        /// No options: the person answers "seen", or with a word back.
        #[arg(long, conflicts_with_all = ["spec", "options", "recommend"])]
        seen: bool,
        /// The option you recommend. Needs --why and at least two options; three or more is better.
        #[arg(long)]
        recommend: Option<String>,
        /// Why you recommend it, in one sentence.
        #[arg(long, default_value = "")]
        why: String,
        /// The element it is about, as a CSS selector.
        #[arg(long)]
        at: Option<String>,
        /// More to read before answering, shown under the question.
        #[arg(long, default_value = "")]
        context: String,
        /// Block until the person answers or the ask is unasked, then print the answer.
        #[arg(long, conflicts_with = "spec")]
        wait: bool,
        /// With --wait: give up after this many seconds (exit 2).
        #[arg(long, requires = "wait")]
        timeout: Option<u64>,
    },
    /// Look at a jelly as the person will see it, before you show it or ask about it.
    ///
    /// `jelly look NAME` renders it at desktop and phone width, light and dark, in
    /// one call. It prints what the jelly is (its verbs and state, what waits on
    /// the person, what they did lately), the problems the browser measured, the
    /// desktop outline, what differs at phone width, and one png with desktop and
    /// phone side by side. --json prints {jelly, version, summary, problems, png,
    /// outline, phone: {width, height, added, removed, changed}}.
    ///
    /// `--text` renders one width and takes no screenshot. `--width`, `--full`
    /// and `--poster` also render one width, with a screenshot of it. --json
    /// prints {jelly, version, summary, problems, png, outline}.
    ///
    /// `--compare v12` shows what changed since version 12 (or since a version
    /// kept with `--save NAME`): the outline lines added, removed or changed, the
    /// problems that are new or fixed, and one png with before and after side by
    /// side. Its --json prints {jelly, version, against, against_version, summary,
    /// problems, new_problems, fixed_problems, png, added, removed, changed}.
    Look {
        name: String,
        /// One width, text only: no screenshot.
        #[arg(long, conflicts_with_all = ["full", "poster", "compare"])]
        text: bool,
        /// Render only this width (390 for a phone). With --compare, compare at it.
        #[arg(long)]
        width: Option<u32>,
        /// One width; screenshot the whole page, not just the first screen.
        #[arg(long)]
        full: bool,
        /// Print only what needs fixing: errors the page threw and `!` warnings.
        #[arg(long, conflicts_with = "json")]
        problems: bool,
        /// One width; also save the screenshot as assets/poster.png, the jelly page's thumbnail.
        #[arg(long)]
        poster: bool,
        /// Compare with an earlier version, `v12`, or one kept with --save. Earlier
        /// versions come from the undo history, back to the last undo.
        #[arg(long, value_name = "VERSION", conflicts_with_all = ["full", "poster"])]
        compare: Option<String>,
        /// Keep this version under a name, to --compare with later. It is kept
        /// outside the jelly, so the jelly does not change.
        #[arg(long, value_name = "NAME")]
        save: Option<String>,
    },
    /// What is waiting: the asks that wait on the person, then their open comments,
    /// which wait on you. One line each. This project's jellies, or one jelly's.
    Waiting {
        name: Option<String>,
        /// Every project.
        #[arg(long, conflicts_with = "name")]
        all: bool,
        /// Only your jellies: ones made or last shaped under your --as / JELLY_AS name.
        #[arg(long)]
        mine: bool,
    },
    /// Use a jelly as the person would, without changing it: do the steps in
    /// order, then print what is on screen, the verbs that ran and what they
    /// changed. Nothing is saved, recorded or undoable.
    ///
    /// A dialog the page opens (alert, confirm, prompt) is printed as a `dialog`
    /// line and does not stop the steps: a confirm is answered OK, a prompt cancelled.
    ///
    /// --json prints {jelly, version, steps, calls, errors, dialogs, failed,
    /// problems, png, state, outline}; `failed.step` counts from 0.
    Try {
        name: String,
        /// Click the element this CSS selector matches. Repeatable.
        #[arg(long, value_name = "SELECTOR")]
        click: Vec<String>,
        /// Fill a field: `--type "#title=Buy milk"`. Repeatable.
        #[arg(long = "type", value_name = "SELECTOR=TEXT")]
        type_: Vec<String>,
        /// Press a key in an element: `--press "#title=Enter"`. Repeatable.
        #[arg(long, value_name = "SELECTOR=KEY")]
        press: Vec<String>,
        /// Render at this width (390 for a phone).
        #[arg(long)]
        width: Option<u32>,
        /// Also save a screenshot after the steps and print its path.
        #[arg(long)]
        png: bool,
        /// The steps in the order given, filled in from the flags above by `main`.
        #[arg(skip)]
        steps: Vec<Value>,
    },
    /// Leave a note for the next agent who picks this jelly up.
    Note { name: String, text: String },
    /// Write the full jelly into a folder: view.html, verbs.js, state.json and
    /// assets/, the files `jelly make --from` and `jelly dev` read.
    Pull {
        name: String,
        /// Default: ./<name>
        dir: Option<PathBuf>,
    },
    /// Watch a pulled folder and shape the jelly on every save.
    Dev {
        name: String,
        dir: Option<PathBuf>,
    },
    /// Print this project's DESIGN.md, or set it with --set FILE. Views can use
    /// its tokens as CSS variables: <link rel="stylesheet" href="/p/PROJECT/design.css">.
    Design {
        #[arg(long)]
        set: Option<PathBuf>,
    },
    /// Take back an ask that no longer applies.
    Unask { name: String, id: String },
    /// Open a jelly full size on the person's screen. No name closes it.
    ///
    /// Give an HTML file instead of a name (`jelly show sketch.html`) to make a
    /// jelly from that one file and show it: no verbs, no state, named after
    /// the file. The same file again changes its view in place.
    Show { name: Option<String> },
    /// Keep a jelly's data fresh after your session ends: the daemon runs COMMAND
    /// on a timer and passes what it prints, as JSON, to VERB.
    ///
    /// `jelly feed prs refresh --every 120 -- gh pr list --json number,title`
    ///
    /// The command runs without a shell, from the folder you ran `jelly feed`
    /// in; use `sh -c '...'` for a pipe. A jelly has one feed: a new one
    /// replaces it. `jelly look NAME` says when it last ran and why it failed.
    /// The feed rests while the jelly is archived. Without a NAME, list feeds.
    Feed {
        name: Option<String>,
        verb: Option<String>,
        /// Seconds between runs (5 or more).
        #[arg(long, default_value_t = 60)]
        every: u64,
        /// Stop feeding NAME.
        #[arg(long, requires = "name", conflicts_with_all = ["verb", "command"])]
        stop: bool,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Say you are changing a jelly, so the person waits ("rebuilding the header").
    /// `--done` says you are finished.
    Working {
        name: String,
        #[arg(required_unless_present = "done")]
        what: Option<String>,
        #[arg(long, conflicts_with = "what")]
        done: bool,
    },
    /// Move a jelly to another project ("" for none). Its name becomes project:name.
    Move { name: String, to: String },
    /// Add files (images, video, models, data) next to a jelly. A view refers
    /// to them as `assets/<file>`. They stay out of state and undo.
    Asset {
        name: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Remove these assets (by name) instead of adding files.
        #[arg(long)]
        rm: bool,
    },
    /// Share a jelly. Without flags, write a read-only snapshot as one HTML file
    /// you can send anyone: state baked in, assets inlined.
    ///
    /// `--to ana@example.com` shares it live through your relay (`jelly relay`)
    /// and prints a link for Ana. She can use the jelly from any device: click,
    /// answer, comment. You hear her in `jelly hear` and `jelly hear --wait` as
    /// person:ana@example.com. Each person gets their own link; asking again
    /// prints the same one. jelly sends no email: pass the link on yourself.
    /// `--link` prints a view-only link: anyone with it watches the jelly change
    /// live, but cannot change it.
    ///
    /// `--pin` is for a room on a call: it prints a 6-digit PIN and the relay's
    /// join page. Each person opens the page and types the PIN with their email,
    /// and gets their own link, as with `--to`: you hear them as
    /// person:<email>, and `jelly unshare NAME --to <email>` stops them. The PIN
    /// works a few hours (`--hours`) and locks after a few wrong tries; running
    /// `--pin` again makes a new one and the old one stops.
    ///
    /// While a jelly is shared, the relay holds the live copy: it keeps working
    /// while this machine sleeps, and your changes (call, set, shape, ask,
    /// unask, reply) go there. Undo, move, rm and a new ask round wait until
    /// `jelly unshare`.
    Share {
        name: String,
        /// Where to write the file (default: <temp dir>/jelly-share/<name>.html, so it never lands in a repo).
        #[arg(long, conflicts_with_all = ["link", "to", "pin"])]
        out: Option<PathBuf>,
        /// A view-only live link.
        #[arg(long, conflicts_with_all = ["to", "pin"])]
        link: bool,
        /// A live link for this person, who can use the jelly through it.
        #[arg(long, value_name = "EMAIL", conflicts_with = "pin")]
        to: Option<String>,
        /// A PIN people type with their email to join, for a room on a call.
        #[arg(long)]
        pin: bool,
        /// Hours until the PIN stops letting people join (default 3, at most 24).
        /// People already in keep their links.
        #[arg(long, requires = "pin", value_parser = clap::value_parser!(u64).range(1..=24))]
        hours: Option<u64>,
        /// Days until the share stops working (default 1, at most 365). A share
        /// lasts as long as its longest link.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=365))]
        days: Option<u64>,
    },
    /// Put what people decided in a jelly on a pull request, as one comment:
    /// each answered ask with the option picked and by whom, the asks still
    /// open, the comments with their replies, the view-only share link if the
    /// jelly has one, and the page as text. It is posted through the GitHub
    /// CLI (`gh`), as the person `gh` is logged in as.
    ///
    /// Running it again replaces that comment instead of adding a second one.
    /// The comment has no screenshot: `gh` cannot upload an image.
    Pr {
        name: String,
        /// The pull request: a number, a URL or a branch. Default: the current branch's.
        #[arg(value_name = "PR")]
        pull: Option<String>,
        /// Print the comment and where it would go, and post nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop sharing a jelly. `--to ana@example.com` stops only Ana's links
    /// (from `--to` or from joining with the PIN), and her open pages close;
    /// `--link` stops the view-only link; `--pin` ends the PIN, so nobody new
    /// joins, and everyone in stays. With none, every link stops and the
    /// jelly's final state stays here.
    Unshare {
        name: String,
        #[arg(long, value_name = "EMAIL", conflicts_with_all = ["link", "pin"])]
        to: Option<String>,
        #[arg(long, conflicts_with = "pin")]
        link: bool,
        #[arg(long)]
        pin: bool,
    },
    /// Use the relay you deployed (relay/ in the jelly repo) for share links.
    /// Saves the URL with a new publish secret in $JELLY_HOME/relay.json and
    /// prints the command that gives the relay the same secret.
    Relay {
        /// The relay's URL, e.g. https://jelly-relay.you.workers.dev. Without it, prints the current one.
        /// (Not named `url`: that would collide with the global --url.)
        address: Option<String>,
    },
    /// Take a share down on your relay, by its link or share id, when a shared
    /// page was reported. Unlike `jelly unshare` it needs no jelly here: it
    /// works on any share on the relay in $JELLY_HOME/relay.json, with that
    /// file's publish secret. Every link to the share stops at once, open
    /// pages close, and the relay deletes its copy. Nothing brings it back.
    Takedown {
        /// A link (https://<relay>/j/<44 characters>) or a share id (its first 22 characters after /j/).
        link: String,
    },
    /// What the person did in this project's jellies since you last listened.
    /// With --hook, starts silently and prints nothing when there is nothing new,
    /// so it can run at the start of every agent turn.
    ///
    /// `--wait` blocks instead, until the person presses Send in NAME (or in any
    /// jelly): they worked through it and hand the turn back. Then run `jelly
    /// hear` for what they did. `--every` returns on the first change by anyone
    /// but you. Exit 2 on --timeout. Run it in the background and it wakes you.
    ///
    /// `--wake` is for an agent that cannot keep a command running after its turn
    /// (Codex): it returns at once, and when the person presses Send in NAME (or
    /// in any jelly of this project) the daemon queues a message to your session,
    /// which starts your next turn.
    ///
    /// Who is listening: --as / JELLY_AS, then --session-id / JELLY_SESSION_ID,
    /// Claude's session, then the invoking parent process.
    Hear {
        /// With --wait or --wake: the jelly to listen to; leave out for any jelly.
        name: Option<String>,
        #[arg(long, conflicts_with_all = ["hook", "mine"])]
        wait: bool,
        #[arg(long, conflicts_with_all = ["wait", "hook", "mine"])]
        wake: bool,
        /// With --wait: start after this change (seq:N), replaying what was recorded since.
        #[arg(long, requires = "wait")]
        since: Option<SequenceToken>,
        /// With --wait: give up after this many seconds (exit 2).
        #[arg(long, requires = "wait")]
        timeout: Option<u64>,
        /// With --wait: return on any change, not only when the person presses Send.
        #[arg(long, requires = "wait")]
        every: bool,
        #[arg(long)]
        hook: bool,
        /// Only your jellies: ones made or last shaped under your --as / JELLY_AS name.
        #[arg(long)]
        mine: bool,
        /// Stable conversation ID, shared by commands, hooks and resumed sessions.
        #[arg(long, env = "JELLY_SESSION_ID")]
        session_id: Option<String>,
    },
    /// Answer on a comment's thread. The person sees it on the pin.
    Reply {
        name: String,
        id: String,
        /// Leave out with --resolve to close the thread without a word.
        #[arg(required_unless_present = "resolve")]
        text: Option<String>,
        /// Close the thread: you dealt with it.
        #[arg(long)]
        resolve: bool,
    },
}


#[derive(Clone, Debug)]
pub struct SequenceToken(pub u64);

impl std::str::FromStr for SequenceToken {
    type Err = String;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        token.strip_prefix("seq:")
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse().ok())
            .map(Self)
            .ok_or_else(|| "use seq:N from a mutation's output (N is an unsigned 64-bit sequence)".into())
    }
}
