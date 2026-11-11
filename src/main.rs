//! `jelly`: the daemon and the CLI agents use to make and shape jellies.

mod design;
mod server;
mod store;
mod verbs;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "jelly", about = "Small live apps that agents and people shape while they run")]
struct Cli {
    /// Daemon address for client commands.
    #[arg(long, env = "JELLY_URL", default_value = "http://127.0.0.1:7878", global = true)]
    url: String,
    /// Who this change is from, recorded on every event.
    #[arg(long = "as", env = "JELLY_AS", default_value = "agent", global = true)]
    by: String,
    /// The project new jellies belong to. Defaults to the name of the git repo
    /// you run from; the jelly page shows each project on its own page.
    #[arg(long, env = "JELLY_PROJECT", global = true)]
    project: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon and the jelly page.
    Serve {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Where jellies are kept. Defaults to $JELLY_HOME or ~/.jelly.
        #[arg(long)]
        home: Option<PathBuf>,
    },
    /// List this project's jellies, one per line (all projects outside a repo).
    Ls {
        /// Every project, archived jellies included.
        #[arg(long)]
        all: bool,
        /// Print the full JSON.
        #[arg(long)]
        json: bool,
    },
    /// Make a jelly from a view and verbs. Use - to read a file from stdin.
    New {
        name: String,
        /// Start from a folder with view.html, verbs.js, state.json and assets/:
        /// a recipe, or what `jelly pull` wrote. --view, --verbs and --state
        /// replace the folder's own.
        #[arg(long)]
        from: Option<PathBuf>,
        #[arg(long, required_unless_present = "from")]
        view: Option<PathBuf>,
        #[arg(long, required_unless_present = "from")]
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
    /// Call a verb, exactly as a button in the view would.
    Call {
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
    Reshape {
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
    /// answers and pins are not undone.
    Undo { name: String },
    /// Keep a jelly.
    Pin { name: String },
    /// Stop keeping a jelly.
    Unpin { name: String },
    /// Remove a jelly and its history.
    Rm { name: String },
    /// Print the full jelly: view, verbs, state.
    Show { name: String },
    /// Stream events as JSON lines, for one jelly or all.
    #[command(alias = "watch")]
    Events {
        name: Option<String>,
        /// Include the full state in each event.
        #[arg(long)]
        state: bool,
    },
    /// Open the jelly page (this project's, if any), or one jelly, in the browser.
    Open { name: Option<String> },
    /// Ask the person something: one question on a jelly, or a set of them.
    ///
    /// Two forms:
    ///
    /// `jelly ask NAME "Which header?" --option a="Bold" --option b="Quiet" --recommend b --why "…" --at "#hero"`
    /// puts one ask on an existing jelly, next to what it is about. It shows on
    /// the jelly and in the person's attention queue.
    ///
    /// `jelly ask NAME --spec spec.json` makes an ask page: one or more questions,
    /// each an attention item with options (label, does, gains, costs, effort, a
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
        #[arg(long)]
        recommend: Option<String>,
        /// Why you recommend it, in one sentence.
        #[arg(long, default_value = "")]
        why: String,
        /// The element it is about, as a CSS selector.
        #[arg(long)]
        at: Option<String>,
        /// More to read before answering, shown under the question.
        #[arg(long, alias = "detail", default_value = "")]
        context: String,
    },
    /// Ask the person to look at something: it lands in their attention queue,
    /// and they answer "seen" or with a word back.
    Look {
        name: String,
        what: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long, alias = "detail", default_value = "")]
        context: String,
    },
    /// Everything waiting on the person in this project (or everywhere with --all).
    Attention {
        #[arg(long)]
        all: bool,
    },
    /// See a jelly as an agent: what it is, its verbs and state, what waits on
    /// the person, what they did lately, and what is on screen as text.
    See {
        name: String,
        /// Render at this width (390 for a phone). Default: an open view, else 1280.
        #[arg(long)]
        width: Option<u32>,
        /// Also save a screenshot and print its path.
        #[arg(long)]
        png: bool,
        /// Screenshot the whole page, not just the first screen (implies --png).
        #[arg(long)]
        full: bool,
    },
    /// Leave a note for the next agent who picks this jelly up.
    Note { name: String, text: String },
    /// Write a jelly's view, verbs, state and assets into a folder to edit.
    Pull {
        name: String,
        /// Default: ./<name>
        dir: Option<PathBuf>,
    },
    /// Watch a pulled folder and reshape the jelly on every save.
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
    /// Take back an ask or look that no longer applies.
    Withdraw { name: String, id: String },
    /// Open a jelly full size on the person's screen. No name closes it.
    Focus { name: Option<String> },
    /// Say you are changing a jelly, so the person waits ("rebuilding the header").
    Working { name: String, what: String },
    /// Say you are done changing a jelly.
    Done { name: String },
    /// Block until someone other than you changes the jelly, then print that
    /// live event and state. With --since, print the first recorded change without state.
    Wait {
        /// Exclusive starting point from a mutation (seq:N). Replays recorded events, without state.
        #[arg(long)]
        since: Option<SequenceToken>,
        /// Leave out to wait on every jelly (the jelly page's own changes are skipped).
        name: Option<String>,
        /// Give up after this many seconds (exit 2).
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// List comments pinned to parts of a jelly's view, with their threads.
    /// Leave out NAME for every jelly: this is the person's to-do list for you.
    Comments {
        name: Option<String>,
        /// Include resolved comments.
        #[arg(long)]
        all: bool,
    },
    /// Move a jelly to another project ("" for none). Its name becomes project:name.
    #[command(alias = "project")]
    Move { name: String, project: String },
    /// Mark a comment as dealt with.
    Resolve { name: String, id: String },
    /// Hide a jelly from the jelly page without deleting it.
    Archive { name: String },
    /// Bring an archived jelly back to the jelly page.
    Restore { name: String },
    /// Add files (images, video, models, data) next to a jelly. A view refers
    /// to them as `assets/<file>`. They stay out of state and undo.
    Asset {
        name: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Write a read-only snapshot of a jelly as one HTML file you can send
    /// anyone: state baked in, assets inlined.
    Share {
        name: String,
        /// Where to write it (default: ./<name>.html).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// What the person did in this project's jellies since you last asked.
    /// With --hook, prints nothing when there is nothing new or no daemon, so
    /// it can run at the start of every agent turn.
    Inbox {
        #[arg(long)]
        hook: bool,
    },
    /// Answer on a comment's thread. The person sees it on the pin.
    Reply {
        name: String,
        id: String,
        text: String,
        /// Close the thread: you dealt with it.
        #[arg(long)]
        resolve: bool,
    },
}


#[derive(Clone, Debug)]
struct SequenceToken(u64);

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


fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jelly: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let api = |path: &str| format!("{}/api/jellies{path}", cli.url);
    let by = cli.by.clone();
    // Only commands that make or open jellies need it, so git runs only for those.
    let project = || detect_project(cli.project.clone());
    // Names are unique per project: a short name means this project's jelly
    // when there is one, and `project:name` always means exactly that jelly.
    let id = |name: &str| -> String {
        if name.contains(':') || name == store::PAGE {
            return name.into();
        }
        let list = get(&api("")).unwrap_or(Value::Null);
        store::resolve_name(name, &project(), list.as_array().into_iter().flatten()
            .filter_map(|j| j["name"].as_str()))
    };
    match cli.cmd {
        Cmd::Serve { port, home } => {
            let home = home
                .or_else(|| std::env::var_os("JELLY_HOME").map(PathBuf::from))
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".jelly")))
                .ok_or("set --home, JELLY_HOME or HOME")?;
            let store = store::Store::open(&home).map_err(|e| format!("{}: {e}", home.display()))?;
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            rt.block_on(server::serve(store, port)).map_err(|e| e.to_string())
        }
        Cmd::Ls { all, json } => {
            let p = if all { String::new() } else { project() };
            let list: Vec<Value> = get(&api(""))?.as_array().cloned().unwrap_or_default().into_iter()
                .filter(|j| all || !j["archived"].as_bool().unwrap_or(false))
                .filter(|j| p.is_empty() || j["project"].as_str() == Some(p.as_str()))
                .collect();
            if json {
                return print(json!(list));
            }
            if list.is_empty() {
                println!("No jellies{}. The daemon is up.", if p.is_empty() { String::new() } else { format!(" in project {p} (--all for every project)") });
            }
            for j in &list {
                let mut tags = vec![format!("v{}", j["version"])];
                let n = |k: &str| j[k].as_u64().unwrap_or(0);
                if n("attention") > 0 { tags.push(format!("{} waiting on the person", n("attention"))); }
                if n("open_comments") > 0 { tags.push(format!("{} open comments", n("open_comments"))); }
                if j["pinned"].as_bool() == Some(true) { tags.push("pinned".into()); }
                if j["archived"].as_bool() == Some(true) { tags.push("archived".into()); }
                println!("{}  {}  ({})", j["name"].as_str().unwrap_or("?"), j["about"].as_str().unwrap_or(""), tags.join(", "));
            }
            Ok(())
        }
        Cmd::New { name, from, view, verbs, state, replace, about } => {
            sync_design(&cli.url, &project());
            let in_from = |file: &str| from.as_ref().map(|d| d.join(file));
            let view = view.or_else(|| in_from("view.html")).ok_or("give --view or --from")?;
            let verbs = verbs.or_else(|| in_from("verbs.js")).ok_or("give --verbs or --from")?;
            let state: Value = match state.or_else(|| in_from("state.json").filter(|p| p.exists())) {
                Some(p) => serde_json::from_str(&read(&p)?).map_err(|e| format!("{}: {e}", p.display()))?,
                None => json!({}),
            };
            let body = json!({"name": name, "view": read(&view)?, "verbs": read(&verbs)?, "state": state, "by": by, "replace": replace, "about": about, "project": project()});
            let ev = send("POST", &api(""), body)?;
            // The folder's assets come along, as `jelly asset` would add them.
            let made = ev["jelly"].as_str().unwrap_or(&name).to_string();
            let mut assets: Vec<PathBuf> = in_from("assets").and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().flatten()
                .map(|f| f.path()).filter(|p| p.is_file()).collect();
            assets.sort();
            report(ev)?;
            add_assets(&api(&format!("/{made}")), &assets, &by)
        }
        Cmd::Call { name, verb, args } => {
            let args = json_arg(&args, "args")?;
            report(send("POST", &api(&format!("/{}/call", id(&name))), json!({"verb": verb, "args": args, "by": by}))?)
        }
        Cmd::Set { name, value, at, rm } => {
            let value = match (rm, value) {
                (true, _) => Value::Null,
                (false, Some(v)) => json_arg(&v, "value")?,
                (false, None) => return Err("give a value, or --rm".into()),
            };
            report(send("POST", &api(&format!("/{}/set", id(&name))), json!({"pointer": at, "value": value, "remove": rm, "by": by}))?)
        }
        Cmd::State { name, pointer } => {
            let p = pointer.map(|p| format!("?pointer={}", encode(&p))).unwrap_or_default();
            print(get(&api(&format!("/{}/state{p}", id(&name))))?)
        }
        Cmd::Reshape { name, view, verbs, about } => {
            let body = json!({"view": view.map(|p| read(&p)).transpose()?, "verbs": verbs.map(|p| read(&p)).transpose()?, "about": about, "by": by});
            report(send("POST", &api(&format!("/{}/reshape", id(&name))), body)?)
        }
        Cmd::Undo { name } => report(send("POST", &api(&format!("/{}/undo", id(&name))), json!({"by": by}))?),
        Cmd::Pin { name } => report(send("POST", &api(&format!("/{}/pin", id(&name))), json!({"by": by, "pinned": true}))?),
        Cmd::Unpin { name } => report(send("POST", &api(&format!("/{}/pin", id(&name))), json!({"by": by, "pinned": false}))?),
        Cmd::Rm { name } => report(send("DELETE", &api(&format!("/{}?by={}", id(&name), encode(&by))), Value::Null)?),
        Cmd::Show { name } => print(get(&api(&format!("/{}", id(&name))))?),
        Cmd::Events { name, state } => {
            let name = name.map(|n| id(&n));
            let q = name.map(|n| format!("?jelly={}", encode(&n))).unwrap_or_default();
            let resp = ureq::get(&format!("{}/api/events{q}", cli.url)).call().map_err(describe)?;
            for line in BufReader::new(resp.into_reader()).lines() {
                let line = line.map_err(|e| e.to_string())?;
                if let Some(data) = line.strip_prefix("data:") {
                    let mut ev: Value = serde_json::from_str(data.trim()).map_err(|e| e.to_string())?;
                    if !state {
                        ev.as_object_mut().map(|o| o.remove("state"));
                    }
                    println!("{ev}");
                }
            }
            Ok(())
        }
        Cmd::Ask { name, question: Some(q), options, recommend, why, at, context, .. } => {
            let options: Vec<Value> = options.iter().map(|o| {
                let (id, rest) = o.split_once('=').ok_or_else(|| format!("--option {o:?}: use id=label"))?;
                let (label, does) = rest.split_once('|').unwrap_or((rest, ""));
                Ok(json!({"id": id.trim(), "label": label.trim(), "does": does.trim()}))
            }).collect::<Result<_, String>>()?;
            let anchor = at.map(|sel| json!({"selector": sel})).unwrap_or(Value::Null);
            let body = json!({"kind": "ask", "title": q, "options": options, "recommend": recommend, "why": why, "anchor": anchor, "context": context, "by": by});
            report(send("POST", &api(&format!("/{}/attention", id(&name))), body)?)
        }
        Cmd::Look { name, what, at, context } => {
            let anchor = at.map(|sel| json!({"selector": sel})).unwrap_or(Value::Null);
            let body = json!({"kind": "look", "title": what, "anchor": anchor, "context": context, "by": by});
            report(send("POST", &api(&format!("/{}/attention", id(&name))), body)?)
        }
        Cmd::Attention { all } => {
            let p = if all { String::new() } else { project() };
            print(get(&format!("{}/api/attention?project={}", cli.url, encode(&p)))?)
        }
        Cmd::Withdraw { name, id: aid } => report(send("POST", &api(&format!("/{}/attention/{}/withdraw", id(&name), encode(&aid))), json!({"by": by}))?),
        Cmd::Focus { name } => report(send("POST", &format!("{}/api/focus", cli.url), json!({"name": name.map(|n| id(&n)), "by": by}))?),
        Cmd::Working { name, what } => report(send("POST", &api(&format!("/{}/working", id(&name))), json!({"what": what, "by": by}))?),
        Cmd::Done { name } => report(send("POST", &api(&format!("/{}/working", id(&name))), json!({"what": null, "by": by}))?),
        Cmd::Ask { spec: None, .. } => Err("give a question (an ask on that jelly) or --spec FILE (an ask jelly)".into()),
        Cmd::Ask { name, spec: Some(spec), .. } => {
            let spec: Value = serde_json::from_str(&read(&spec)?).map_err(|e| format!("spec is not JSON: {e}"))?;
            let r = send("POST", &api(&format!("/{}/ask", id(&name))), json!({"spec": spec, "project": project(), "by": by}))?;
            let jelly = r["jelly"].as_str().unwrap_or(&name).to_string();
            let since = format!("seq:{}", r["seq"]);
            print(json!({"jelly": jelly, "round": r["state"]["round"], "since": since, "next": format!("jelly wait {jelly} --since {since}: the answers arrive as answer events")}))
        }
        Cmd::Wait { name, timeout, since } => {
            let project_jelly = name.as_ref().filter(|_| since.is_some()).filter(|n| !n.contains(':') && *n != store::PAGE)
                .map(|n| { let p = project(); if p.is_empty() { n.clone() } else { format!("{p}:{n}") } });
            let name = name.map(|n| id(&n));
            if let Some(SequenceToken(seq)) = since {
                let mut req = ureq::get(&format!("{}/api/wait", cli.url))
                    .query("since", &seq.to_string()).query("by", &by);
                if let Some(n) = &name { req = req.query("jelly", n); }
                if let Some(preferred) = project_jelly {
                    req = req.query("project_jelly", &preferred);
                }
                if let Some(t) = timeout { req = req.query("timeout", &t.to_string()); }
                let resp = req.call().map_err(describe)?;
                if resp.status() == 204 {
                    let what = name.as_deref().unwrap_or("any jelly");
                    eprintln!("jelly: nobody changed {what} in time");
                    std::process::exit(2);
                }
                return print(resp.into_json().map_err(|e| e.to_string())?);
            }
            let filter = name.as_deref().map(|n| format!("?jelly={}", encode(n))).unwrap_or_default();
            let mut req = ureq::get(&format!("{}/api/events{filter}", cli.url));
            if let Some(t) = timeout {
                req = req.timeout(std::time::Duration::from_secs(t));
            }
            if let Some(n) = &name {
                get(&api(&format!("/{n}/state")))?; // fail fast on a wrong name
            }
            let what = name.as_deref().unwrap_or("any jelly");
            let resp = req.call().map_err(describe)?;
            for line in BufReader::new(resp.into_reader()).lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                        eprintln!("jelly: nobody changed {what} in time");
                        std::process::exit(2);
                    }
                    Err(e) => return Err(e.to_string()),
                };
                let Some(data) = line.strip_prefix("data:") else { continue };
                let ev: Value = serde_json::from_str(data.trim()).map_err(|e| e.to_string())?;
                if ev["by"] != by.as_str() && (name.is_some() || ev["jelly"] != store::PAGE) {
                    return print(ev);
                }
            }
            Err("the event stream ended".into())
        }
        Cmd::Comments { name, all } => {
            let names: Vec<String> = match name {
                Some(n) => vec![id(&n)],
                None => {
                    let p = project();
                    get(&api(""))?.as_array().into_iter().flatten()
                    .filter(|j| p.is_empty() || j["project"] == p.as_str())
                    .filter(|j| all || j["open_comments"].as_u64().unwrap_or(0) > 0)
                    .filter_map(|j| j["name"].as_str().map(String::from)).collect()
                }
            };
            let mut list = Vec::new();
            for n in names {
                let j = get(&api(&format!("/{n}")))?;
                for mut f in j["feedback"].as_array().cloned().unwrap_or_default() {
                    if all || f["resolved"] != true {
                        f["jelly"] = json!(n);
                        list.push(f);
                    }
                }
            }
            print(json!(list))
        }
        Cmd::Reply { name, id: cid, text, resolve } => report(send("POST", &api(&format!("/{}/reply", id(&name))), json!({"id": cid, "text": text, "resolve": resolve, "by": by}))?),
        Cmd::Move { name, project } => report(send("POST", &api(&format!("/{}/move", id(&name))), json!({"project": project, "by": by}))?),
        Cmd::See { name, width, png, full } => {
            let mut q = vec![];
            if let Some(w) = width { q.push(format!("width={w}")); }
            if png || full { q.push("png=true".into()); }
            if full { q.push("full=true".into()); }
            let url = api(&format!("/{}/see?{}", id(&name), q.join("&")));
            let resp = ureq::get(&url).timeout(std::time::Duration::from_secs(60)).call().map_err(describe)?;
            print!("{}", resp.into_string().map_err(|e| e.to_string())?);
            Ok(())
        }
        Cmd::Note { name, text } => report(send("POST", &api(&format!("/{}/note", id(&name))), json!({"text": text, "by": by}))?),
        Cmd::Pull { name, dir } => {
            let name = id(&name);
            let short = name.split_once(':').map_or(name.as_str(), |(_, n)| n).to_string();
            let dir = dir.unwrap_or_else(|| PathBuf::from(&short));
            let j = get(&api(&format!("/{name}")))?;
            std::fs::create_dir_all(dir.join("assets")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("view.html"), j["view"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("verbs.js"), j["verbs"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("state.json"), serde_json::to_string_pretty(&j["state"]).unwrap_or_default()).map_err(|e| e.to_string())?;
            let assets_dir = format!("{}/j/{}/assets", cli.url, name);
            let list = std::fs::read_dir(std::env::var_os("JELLY_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".jelly")).join("assets").join(&name));
            let mut n = 0;
            for f in list.into_iter().flatten().flatten() {
                let file = f.file_name().to_string_lossy().to_string();
                if let Ok(r) = ureq::get(&format!("{assets_dir}/{}", encode(&file))).call() {
                    let mut bytes = Vec::new();
                    r.into_reader().read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                    std::fs::write(dir.join("assets").join(&file), bytes).map_err(|e| e.to_string())?;
                    n += 1;
                }
            }
            println!("{}/  view.html  verbs.js  state.json  assets/ ({n})\nEdit, then: jelly dev {name} {}", dir.display(), dir.display());
            Ok(())
        }
        Cmd::Dev { name, dir } => {
            let name = id(&name);
            let short = name.split_once(':').map_or(name.as_str(), |(_, n)| n).to_string();
            let dir = dir.unwrap_or_else(|| PathBuf::from(&short));
            let stamp = |f: &str| std::fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok();
            let (mut v, mut b) = (stamp("view.html"), stamp("verbs.js"));
            if v.is_none() && b.is_none() {
                return Err(format!("nothing to watch in {}: run jelly pull {name} first", dir.display()));
            }
            println!("watching {} for {name} (Ctrl-C to stop)", dir.display());
            loop {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let (nv, nb) = (stamp("view.html"), stamp("verbs.js"));
                if nv == v && nb == b { continue; }
                let view = (nv != v).then(|| std::fs::read_to_string(dir.join("view.html"))).transpose().map_err(|e| e.to_string())?;
                let verbs = (nb != b).then(|| std::fs::read_to_string(dir.join("verbs.js"))).transpose().map_err(|e| e.to_string())?;
                (v, b) = (nv, nb);
                match send("POST", &api(&format!("/{name}/reshape")), json!({"view": view, "verbs": verbs, "by": by})) {
                    Ok(ev) => println!("reshaped v{} ({})", ev["version"], [view.is_some().then_some("view"), verbs.is_some().then_some("verbs")].into_iter().flatten().collect::<Vec<_>>().join(" + ")),
                    Err(e) => eprintln!("not reshaped: {e}"),
                }
            }
        }
        Cmd::Design { set } => {
            let p = project();
            if p.is_empty() { return Err("not in a project: run from a git repo or pass --project".into()); }
            let url = format!("{}/api/projects/{}/design", cli.url, encode(&p));
            match set {
                Some(f) => {
                    ureq::put(&url).send_string(&read(&f)?).map_err(describe)?;
                    println!("{p}: DESIGN.md set. Views use it with <link rel=\"stylesheet\" href=\"/p/{p}/design.css\">");
                    Ok(())
                }
                None => { print!("{}", ureq::get(&url).call().map_err(describe)?.into_string().map_err(|e| e.to_string())?); Ok(()) }
            }
        }
        Cmd::Archive { name } => report(send("POST", &api(&format!("/{}/archive", id(&name))), json!({"by": by}))?),
        Cmd::Restore { name } => report(send("POST", &api(&format!("/{}/restore", id(&name))), json!({"by": by}))?),
        Cmd::Asset { name, files } => {
            add_assets(&api(&format!("/{}", id(&name))), &files, &by)
        }
        Cmd::Share { name, out } => {
            let name = id(&name);
            let resp = ureq::get(&api(&format!("/{name}/share"))).call().map_err(describe)?;
            let mut html = String::new();
            resp.into_reader().read_to_string(&mut html).map_err(|e| e.to_string())?;
            let short = name.split_once(':').map_or(name.as_str(), |(_, n)| n);
            let out = out.unwrap_or_else(|| PathBuf::from(format!("{short}.html")));
            std::fs::write(&out, &html).map_err(|e| format!("{}: {e}", out.display()))?;
            println!("{}  ({} KB, read-only, opens anywhere)", out.display(), html.len().div_ceil(1024));
            Ok(())
        }
        Cmd::Inbox { hook } => inbox(&cli.url, &project(), hook),
        Cmd::Resolve { name, id: cid } => report(send("POST", &api(&format!("/{}/resolve", id(&name))), json!({"id": cid, "by": by}))?),
        Cmd::Open { name } => {
            let url = match (name.as_deref(), project().as_str()) {
                (Some(n), _) => format!("{}/j/{}/", cli.url, id(n)),
                (None, "") => format!("{}/", cli.url),
                (None, p) => format!("{}/p/{p}", cli.url),
            };
            std::process::Command::new("open").arg(&url).status().map_err(|e| e.to_string())?;
            println!("{url}");
            Ok(())
        }
    }
}

/// If the repo has a DESIGN.md, give it to the project so views can use it.
fn sync_design(url: &str, project: &str) {
    if project.is_empty() { return; }
    let Ok(out) = std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output() else { return };
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Ok(md) = std::fs::read_to_string(Path::new(&top).join("DESIGN.md")) {
        let _ = ureq::put(&format!("{url}/api/projects/{}/design", encode(project))).send_string(&md);
    }
}

/// Prints what the person did since the last call, then moves the cursor.
fn inbox(url: &str, project: &str, hook: bool) -> Result<(), String> {
    if hook && project.is_empty() {
        return Ok(()); // not in a repo: this session has no jellies of its own
    }
    let home = std::env::var_os("JELLY_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".jelly")))
        .ok_or("set JELLY_HOME or HOME")?;
    let cursor = home.join("inbox").join(if project.is_empty() { "_all" } else { project });
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    // First run: the last hour, not all history.
    let since = std::fs::read_to_string(&cursor).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(now.saturating_sub(3_600_000));
    let resp = ureq::get(&format!("{url}/api/inbox?project={}&since={since}", encode(project)))
        .timeout(std::time::Duration::from_secs(1))
        .call();
    let items: Vec<Value> = match resp.map_err(describe).and_then(|r| r.into_json().map_err(|e| e.to_string())) {
        Ok(v) => v,
        Err(_) if hook => return Ok(()),
        Err(e) => return Err(e),
    };
    let _ = std::fs::create_dir_all(cursor.parent().expect("has a parent"));
    let _ = std::fs::write(&cursor, now.to_string());
    if items.is_empty() {
        if !hook {
            println!("Nothing new from the person.");
        }
        return Ok(());
    }
    let scope = if project.is_empty() { String::new() } else { format!(" (project {project})") };
    println!("The person acted on your jellies{scope} since you last checked:");
    for e in &items {
        println!("- {}: {}", e["jelly"].as_str().unwrap_or("?"), e["text"].as_str().unwrap_or(""));
    }
    println!("Details: `jelly state NAME`, `jelly comments`. Answer on a comment with `jelly reply`.");
    Ok(())
}

/// The project from `--project`, else the git repo's folder name, made into a
/// valid name ("Archon" -> "archon"). Empty when neither applies.
fn detect_project(given: Option<String>) -> String {
    let raw = given.or_else(|| {
        let out = std::process::Command::new("git").args(["rev-parse", "--show-toplevel"]).output().ok()?;
        let top = String::from_utf8(out.stdout).ok()?;
        out.status.success().then(|| Path::new(top.trim()).file_name()?.to_str().map(String::from)).flatten()
    });
    let name: String = raw.unwrap_or_default().to_lowercase().chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' { c } else { '-' }).collect();
    name.trim_matches('-').chars().take(64).collect()
}

/// One line on what a write did. The full event is in `jelly events`.
fn report(ev: Value) -> Result<(), String> {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let what = match ev["kind"].as_str().unwrap_or("") {
        "call" => format!("{} ran", s(&ev["verb"])),
        "attention" => format!("asked {} {:?}; {}. Answers arrive in `jelly inbox`", s(&ev["args"]["id"]), s(&ev["args"]["title"]), placement(&ev)),
        "set" => if s(&ev["args"]["pointer"]).is_empty() { "state replaced".into() } else { format!("set {}", s(&ev["args"]["pointer"])) },
        "new" => "made".into(),
        k => k.to_string(),
    };
    let changed: Vec<String> = ev["changed"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).filter(|c| !c.is_empty()).map(String::from).collect();
    let changed = match changed.len() { 0 => String::new(), 1..=6 => format!(" · changed {}", changed.join(" ")), n => format!(" · changed {n} places") };
    println!("{} v{}: {what}{changed} · since seq:{}", s(&ev["jelly"]), ev["version"], ev["seq"]);
    Ok(())
}

/// Where an ask or look shows on the page, as the daemon found it: `anchor`
/// says what became of its `--at`, and is absent when nothing could check.
fn placement(ev: &Value) -> String {
    let Some(at) = ev["args"]["anchor"]["selector"].as_str() else { return "it shows at the top of the page".into() };
    match ev["anchor"].as_str() {
        Some("below") => format!("it shows on the page below {at}"),
        Some("missing") => format!("{at} matches nothing on the page, so it shows at the top"),
        Some("floating") => format!("{at} is in a fixed or sticky part of the page, so it shows at the top"),
        Some("hidden") => format!("it shows below {at}, which is not on screen right now"),
        _ => format!("it shows below {at} if that matches something, else at the top: not checked ({})",
            ev["anchor_unchecked"].as_str().unwrap_or("no snapshot")),
    }
}

/// A JSON argument: literal JSON, @FILE, or - for stdin (for data too big for a command line).
fn json_arg(arg: &str, what: &str) -> Result<Value, String> {
    let text = if arg == "-" { read(&PathBuf::from("-"))? } else if let Some(f) = arg.strip_prefix('@') { read(&PathBuf::from(f))? } else { arg.to_string() };
    serde_json::from_str(&text).map_err(|e| format!("{what} are not JSON: {e}").replace("value are", "value is"))
}

fn read(p: &PathBuf) -> Result<String, String> {
    if p.as_os_str() == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
        return Ok(s);
    }
    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Adds files beside a jelly; `jelly` is its API address.
fn add_assets(jelly: &str, files: &[PathBuf], by: &str) -> Result<(), String> {
    // Add every file before printing anything: a reader that closes the pipe
    // early (`| head -1`) must not leave the jelly with half its assets.
    let mut added = Vec::new();
    for f in files {
        let file = f.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{}: not a file name", f.display()))?.to_string();
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        ureq::post(&format!("{jelly}/assets/{}?by={}", encode(&file), encode(by)))
            .set("content-type", "application/octet-stream")
            .send_bytes(&bytes)
            .map_err(describe)?;
        added.push(format!("assets/{file}  ({} KB)", bytes.len().div_ceil(1024)));
    }
    for line in added {
        println!("{line}");
    }
    Ok(())
}

fn get(url: &str) -> Result<Value, String> {
    ureq::get(url).call().map_err(describe)?.into_json().map_err(|e| e.to_string())
}

fn send(method: &str, url: &str, body: Value) -> Result<Value, String> {
    let req = ureq::request(method, url);
    let resp = if body.is_null() { req.call() } else { req.send_json(body) };
    resp.map_err(describe)?.into_json().map_err(|e| e.to_string())
}

fn describe(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(_, resp) => resp
            .into_json::<Value>()
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(String::from))
            .unwrap_or_else(|| "request failed".into()),
        ureq::Error::Transport(t) => format!("cannot reach the daemon ({t}); start it with `jelly serve`"),
    }
}

fn print(v: Value) -> Result<(), String> {
    println!("{}", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
    Ok(())
}

/// Percent-encodes a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::placement;
    use serde_json::json;

    #[test]
    fn an_ask_says_where_its_card_really_shows() {
        let at = |extra: serde_json::Value| {
            let mut ev = json!({"args": {"anchor": {"selector": "#list"}}});
            ev.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            placement(&ev)
        };
        assert_eq!(at(json!({"anchor": "below"})), "it shows on the page below #list");
        assert_eq!(at(json!({"anchor": "missing"})), "#list matches nothing on the page, so it shows at the top");
        assert_eq!(at(json!({"anchor": "floating"})), "#list is in a fixed or sticky part of the page, so it shows at the top");
        assert_eq!(at(json!({"anchor": "hidden"})), "it shows below #list, which is not on screen right now");
        let unchecked = at(json!({"anchor_unchecked": "no Chrome found"}));
        assert!(unchecked.contains("not checked (no Chrome found)") && !unchecked.starts_with("it shows on the page below"));
        assert_eq!(placement(&json!({"args": {"anchor": null}})), "it shows at the top of the page");
    }
}
