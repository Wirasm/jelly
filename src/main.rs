//! `jelly`: the daemon and the CLI agents use to make and shape jellies.

mod cli;
mod design;
mod disk;
mod doctor;
mod feed;
mod pr;
mod recipes;
mod relay;
mod server;
mod setup;
mod store;
mod startup;
mod update;
mod verbs;

use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Cmd, SequenceToken};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug)]
enum CommandError {
    Message(String),
    Output(std::io::Error),
}

impl From<String> for CommandError {
    fn from(message: String) -> Self { Self::Message(message) }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self { Self::Message(message.into()) }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => message.fmt(f),
            Self::Output(error) => error.fmt(f),
        }
    }
}

fn write_output(writer: &mut impl Write, args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    writer.write_fmt(args)?;
    writer.flush()
}

fn output(args: std::fmt::Arguments<'_>) -> Result<(), CommandError> {
    write_output(&mut std::io::stdout().lock(), args).map_err(CommandError::Output)
}

fn main() -> ExitCode {
    // Exit 2 means one thing: nobody acted in time (`hear --wait`, `ask --wait`).
    // A mistyped command exits 1, so a script cannot take it for a timeout, and a
    // prompt hook that runs a command this build no longer has does not block the prompt.
    let matches = Cli::command().try_get_matches().unwrap_or_else(|e| {
        let _ = e.print();
        std::process::exit(if e.use_stderr() { 1 } else { 0 })
    });
    let mut cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if let (Cmd::Try { steps, .. }, Some(m)) = (&mut cli.cmd, matches.subcommand_matches("try")) {
        match try_steps(m) {
            Ok(s) => *steps = s,
            Err(e) => {
                eprintln!("jelly: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CommandError::Output(e)) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jelly: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `jelly try`'s steps in the order they were given, which separate flags lose.
fn try_steps(m: &clap::ArgMatches) -> Result<Vec<Value>, String> {
    let mut steps: Vec<(usize, Value)> = Vec::new();
    for (flag, field) in [("click", ""), ("type_", "text"), ("press", "key")] {
        let (Some(values), Some(at)) = (m.get_many::<String>(flag), m.indices_of(flag)) else { continue };
        for (value, i) in values.zip(at) {
            let step = if field.is_empty() {
                json!({"do": "click", "at": value})
            } else {
                let (sel, rest) = split_target(value).ok_or_else(|| format!("--{} wants SELECTOR={}, e.g. \"#title={}\"", flag.trim_end_matches('_'), field.to_uppercase(), if field == "key" { "Enter" } else { "Buy milk" }))?;
                json!({"do": flag.trim_end_matches('_'), "at": sel, field: rest})
            };
            steps.push((i, step));
        }
    }
    if steps.is_empty() {
        return Err("give jelly try at least one --click, --type or --press".into());
    }
    steps.sort_by_key(|(i, _)| *i);
    Ok(steps.into_iter().map(|(_, s)| s).collect())
}

/// Splits `SELECTOR=VALUE` at the first `=` outside brackets, so `[name=q]=hi` works.
fn split_target(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            '=' if depth == 0 && i > 0 => return Some((&s[..i], &s[i + 1..])),
            _ => {}
        }
    }
    None
}

fn daemon_command() -> Result<std::process::Command, String> {
    let mut command = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("serve");
    Ok(command)
}

fn run(cli: Cli) -> Result<(), CommandError> {
    run_with_start(cli, |url, quiet| startup::ensure_running(url, quiet, daemon_command()?))
}

fn run_with_start(mut cli: Cli, start: impl FnOnce(&str, bool) -> Result<(), String>) -> Result<(), CommandError> {
    if !matches!(cli.cmd, Cmd::Serve { .. } | Cmd::Recipes { .. } | Cmd::Relay { .. } | Cmd::Setup { .. } | Cmd::Update | Cmd::Doctor | Cmd::PlanApproved) && startup::is_default_endpoint(&cli.url) {
        cli.url = startup::DEFAULT_URL.into();
        let quiet = matches!(cli.cmd, Cmd::Hear { hook: true, .. });
        if let Err(e) = start(&cli.url, quiet) {
            return if quiet { Ok(()) } else { Err(e.into()) };
        }
    }
    let api = |path: &str| format!("{}/api/jellies{path}", cli.url);
    let by = cli.by.clone().unwrap_or_else(|| "agent".into());
    let json = cli.json;
    if json && matches!(cli.cmd, Cmd::Serve { .. } | Cmd::Recipes { .. } | Cmd::Setup { .. } | Cmd::Update | Cmd::Open { .. } | Cmd::Pull { .. } | Cmd::Dev { .. }
        | Cmd::Design { .. } | Cmd::Asset { .. } | Cmd::Pr { .. } | Cmd::Share { .. } | Cmd::Unshare { .. } | Cmd::Relay { .. } | Cmd::Hear { wait: false, .. }) {
        return Err("this command has no --json output".into());
    }
    // A change prints the daemon's event as it is, or one line about it.
    let report = |ev: Value| if json { print(ev) } else { report(ev) };
    // Only commands that make or open jellies need it, so git runs only for those.
    let project = || detect_project(cli.project.clone());
    // Names are unique per project: a short name means this project's jelly
    // when there is one, and `project:name` always means exactly that jelly.
    let id = |name: &str| -> Result<String, String> {
        if name.contains(':') || name == store::PAGE {
            return Ok(name.into());
        }
        let list = get(&api(""))?;
        Ok(store::resolve_name(name, &project(), list.as_array().into_iter().flatten()
            .filter_map(|j| j["name"].as_str())))
    };
    // `show` moves the jelly page, so it reaches nobody unless that page is open.
    let show = |name: Value| -> Result<(), CommandError> {
        let ev = send("POST", &format!("{}/api/show", cli.url), json!({"name": name, "by": by}))?;
        if ev["visible"] == 0 && !name.is_null() {
            eprintln!("jelly: nobody has the jelly page open, so nobody sees this yet. Give the person {}/", cli.url);
        }
        report(ev)
    };
    match cli.cmd {
        Cmd::Serve { port, home, temp } => {
            let port = port.unwrap_or(if temp { 0 } else { 7878 });
            let home = if temp {
                let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
                std::env::temp_dir().join(format!("jelly-temp-{}-{nanos}", std::process::id()))
            } else {
                match home { Some(h) => h, None => default_home()? }
            };
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            rt.block_on(async {
                // Reserve the endpoint before store initialization can write anything.
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| e.to_string())?;
                // Two daemons on one home both write events and jelly files, and each
                // loses the other's changes. The lock lives as long as this daemon.
                let _lock = lock_home(&home, listener.local_addr().map_err(|e| e.to_string())?.port())?;
                let store = store::Store::open(&home).map_err(|e| format!("{}: {e}", home.display()))?;
                let note = temp.then(|| {
                    // A clean stop removes the home; a crash leaves it as evidence.
                    let dir = home.clone();
                    tokio::spawn(async move {
                        stopped().await;
                        let _ = std::fs::remove_dir_all(&dir);
                        std::process::exit(0);
                    });
                    format!("jelly: temporary home {}; removed when this stops", home.display())
                });
                server::serve(store, listener, note).await.map_err(|e| e.to_string().into())
            })
        }
        Cmd::Find { words } => {
            let words: Vec<String> = words.iter().flat_map(|w| w.split_whitespace()).map(str::to_lowercase).collect();
            let hay = |j: &Value| format!("{} {}", j["name"].as_str().unwrap_or(""), j["about"].as_str().unwrap_or("")).to_lowercase();
            let mut found: Vec<Value> = get(&api(""))?.as_array().cloned().unwrap_or_default().into_iter()
                .filter(|j| j["name"].as_str() != Some(store::PAGE))
                .filter(|j| { let h = hay(j); words.iter().all(|w| h.contains(w.as_str())) })
                .collect();
            // Live jellies first, then the most recently changed.
            found.sort_by_key(|j| (j["archived"].as_bool() == Some(true), std::cmp::Reverse(j["updated_ms"].as_u64().unwrap_or(0))));
            if json {
                return print(json!(found));
            }
            if found.is_empty() {
                return Err(format!("no jelly matches {:?}; `jelly ls --all` lists them all", words.join(" ")).into());
            }
            for j in &found {
                output(format_args!("{}\n", reference(j)))?;
            }
            Ok(())
        }
        Cmd::Ls { all } => {
            let p = if all { String::new() } else { project() };
            let list: Vec<Value> = get(&api(""))?.as_array().cloned().unwrap_or_default().into_iter()
                .filter(|j| all || !j["archived"].as_bool().unwrap_or(false))
                .filter(|j| p.is_empty() || j["project"].as_str() == Some(p.as_str()))
                .collect();
            if json {
                return print(json!(list));
            }
            if list.is_empty() {
                output(format_args!("No jellies{}. The daemon is up.\n", if p.is_empty() { String::new() } else { format!(" in project {p} (--all for every project)") }))?;
            }
            for j in &list {
                let mut tags = vec![format!("v{}", j["version"])];
                let n = |k: &str| j[k].as_u64().unwrap_or(0);
                if n("open_asks") > 0 { tags.push(format!("{} waiting on the person", n("open_asks"))); }
                if n("open_comments") > 0 { tags.push(format!("{} open comments", n("open_comments"))); }
                if j["kept"].as_bool() == Some(true) { tags.push("kept".into()); }
                if j["archived"].as_bool() == Some(true) { tags.push("archived".into()); }
                output(format_args!("{}  {}  ({})\n", j["name"].as_str().unwrap_or("?"), j["about"].as_str().unwrap_or(""), tags.join(", ")))?;
            }
            Ok(())
        }
        Cmd::Make { name, from, recipe, view, verbs, state, replace, about } => {
            let recipe = recipe.map(|r| recipes::get(&r)).transpose()?;
            sync_design(&cli.url, &project());
            let in_from = |file: &str| from.as_ref().map(|d| d.join(file));
            // A flag wins, then the folder, then the recipe. Only state.json is optional.
            let text = |flag: Option<PathBuf>, file: &str| -> Result<Option<String>, String> {
                match flag.or_else(|| in_from(file).filter(|p| file != "state.json" || p.exists())) {
                    Some(p) => read(&p).map(Some),
                    None => Ok(recipe.and_then(|r| r.text(file)).map(String::from)),
                }
            };
            let view = text(view, "view.html")?.ok_or("give --view, --from or --recipe")?;
            let verbs = text(verbs, "verbs.js")?.ok_or("give --verbs, --from or --recipe")?;
            // A recipe's assets belong to its example state: the review recipe's
            // full.diff would otherwise show beside your own review.
            let recipe_assets = recipe.filter(|_| state.is_none()).into_iter().flat_map(|r| r.assets());
            let state: Value = match text(state, "state.json")? {
                Some(s) => serde_json::from_str(&s).map_err(|e| format!("state is not JSON: {e}"))?,
                None => json!({}),
            };
            let project = project();
            let body = json!({"name": name, "view": view, "verbs": verbs, "state": state, "by": by, "replace": replace, "about": about, "project": project});
            let ev = send("POST", &api(""), body)?;
            // `inbox`, `ls` and `attention` show the project you run them from, so
            // from the person's repo they would never show this jelly.
            if project.is_empty() && cli.project.is_none() && !name.contains(':') {
                eprintln!("jelly: {name} is in no project, because this folder is not in a git repo. `jelly hear` run from a repo will not show what the person does in it. Put it in one with `jelly move {name} PROJECT`, or make jellies from the repo: `jelly make {name} --from FOLDER`.");
            }
            // The folder's or recipe's assets come along, as `jelly asset` would add them.
            let made = ev["jelly"].as_str().unwrap_or(&name).to_string();
            let jelly = api(&format!("/{made}"));
            let mut assets: Vec<PathBuf> = in_from("assets").and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().flatten()
                .map(|f| f.path()).filter(|p| p.is_file()).collect();
            assets.sort();
            let mut added = add_assets(&jelly, &assets, &by)?;
            for (file, bytes) in recipe_assets {
                added.push(upload_asset(&jelly, file, bytes, &by)?);
            }
            if json {
                return print(ev);
            }
            report(ev)?;
            for line in added {
                output(format_args!("{line}\n"))?;
            }
            Ok(())
        }
        Cmd::PlanApproved => output(format_args!("{}", setup::plan_approved())),
        Cmd::Setup { dry_run } => {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok_or("could not find your home directory")?;
            for line in setup::run(Path::new(&home), dry_run)? {
                output(format_args!("{line}\n"))?;
            }
            Ok(())
        }
        Cmd::Update => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let latest = update::latest()?;
            let version = format!("{}/api/version", cli.url);
            // None when no daemon runs there, or one from before it had a version.
            let daemon = ureq::get(&version).call().ok().and_then(|r| r.into_json::<Value>().ok());
            // Waits for the daemon to be up again, which its new start time shows.
            let restart = |daemon: &Value| -> Result<(), String> {
                send("POST", &format!("{}/api/restart", cli.url), json!({}))?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if get(&version).is_ok_and(|now| now["started_ms"] != daemon["started_ms"]) {
                        return Ok(());
                    }
                }
                Err(format!("the daemon at {} did not come back; run `jelly serve` for details", cli.url))
            };
            if !update::newer(&latest, update::VERSION) {
                output(format_args!("jelly {} is up to date (latest release: {latest}).\n", update::VERSION))?;
                if let Some(daemon) = daemon.filter(update::daemon_differs) {
                    restart(&daemon)?;
                    output(format_args!("Restarted the daemon at {}, which ran another build.\n", cli.url))?;
                }
                return Ok(());
            }
            if update::Install::of(&exe) == update::Install::Source {
                let [pull, install] = update::source_commands();
                return output(format_args!("jelly {latest} is out; this is {}, built from source. Update it with:\n  {pull}\n  {install}\nThen `jelly doctor` says what else is behind.\n", update::VERSION));
            }
            let setup = update::apply(&exe, &latest)?;
            output(format_args!("{setup}\n"))?;
            if let Some(daemon) = daemon {
                restart(&daemon)?;
                output(format_args!("Restarted the daemon at {}.\n", cli.url))?;
            }
            output(format_args!("jelly is now {latest}. Read the jelly skill again before you use jelly: commands and `window.jelly` may have changed.\n"))
        }
        Cmd::Doctor => {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok_or("could not find your home directory")?;
            let checks = doctor::run(Path::new(&home), &cli.url);
            if json {
                print(serde_json::to_value(&checks).map_err(|e| e.to_string())?)?;
            } else {
                for check in &checks {
                    output(format_args!("{}\n", check.line()))?;
                }
            }
            let fixes = checks.iter().filter(|c| c.ok == Some(false)).count();
            if fixes > 0 {
                return Err(format!("{fixes} to fix").into());
            }
            Ok(())
        }
        Cmd::Recipes { name: None, .. } => {
            for r in recipes::RECIPES {
                output(format_args!("{:<9} {}\n", r.name, r.use_when()))?;
            }
            output(format_args!("\nRead one: jelly recipes plan. Make a jelly from it: jelly make NAME --recipe plan.\n"))
        }
        Cmd::Recipes { name: Some(name), out: None } => {
            output(format_args!("{}", recipes::get(&name)?.text("RECIPE.md").unwrap_or("")))
        }
        Cmd::Recipes { name: Some(name), out: Some(dir) } => {
            let r = recipes::get(&name)?;
            r.write(&dir)?;
            let files: Vec<&str> = r.files.iter().map(|(p, _)| *p).filter(|p| !p.starts_with("assets/")).collect();
            output(format_args!("{}/  {}  assets/ ({})\nEdit, then: jelly make {name} --from {}\n",
                dir.display(), files.join("  "), r.assets().count(), dir.display()))
        }
        Cmd::Do { name, verb, args } => {
            let args = json_arg(&args, "args")?;
            report(send("POST", &api(&format!("/{}/do", id(&name)?)), json!({"verb": verb, "args": args, "by": by}))?)
        }
        Cmd::Set { name, value, at, rm } => {
            let value = match (rm, value) {
                (true, _) => Value::Null,
                (false, Some(v)) => json_arg(&v, "value")?,
                (false, None) => return Err("give a value, or --rm".into()),
            };
            report(send("POST", &api(&format!("/{}/set", id(&name)?)), json!({"pointer": at, "value": value, "remove": rm, "by": by}))?)
        }
        Cmd::State { name, pointer } => {
            let p = pointer.map(|p| format!("?pointer={}", encode(&p))).unwrap_or_default();
            print(get(&api(&format!("/{}/state{p}", id(&name)?)))?)
        }
        Cmd::Shape { name, view, verbs, about } => {
            let body = json!({"view": view.map(|p| read(&p)).transpose()?, "verbs": verbs.map(|p| read(&p)).transpose()?, "about": about, "by": by});
            report(send("POST", &api(&format!("/{}/shape", id(&name)?)), body)?)
        }
        Cmd::Undo { name } => report(send("POST", &api(&format!("/{}/undo", id(&name)?)), json!({"by": by}))?),
        Cmd::Keep { name } => report(send("POST", &api(&format!("/{}/keep", id(&name)?)), json!({"by": by, "kept": true}))?),
        Cmd::Unkeep { name } => report(send("POST", &api(&format!("/{}/keep", id(&name)?)), json!({"by": by, "kept": false}))?),
        Cmd::Rm { name } => report(send("DELETE", &api(&format!("/{}?by={}", id(&name)?, encode(&by))), Value::Null)?),
        Cmd::Events { name, state } => {
            let name = name.map(|n| id(&n)).transpose()?;
            let q = name.map(|n| format!("?jelly={}", encode(&n))).unwrap_or_default();
            let resp = ureq::get(&format!("{}/api/events{q}", cli.url)).call().map_err(describe)?;
            for line in BufReader::new(resp.into_reader()).lines() {
                let line = line.map_err(|e| e.to_string())?;
                if let Some(data) = line.strip_prefix("data:") {
                    let mut ev: Value = serde_json::from_str(data.trim()).map_err(|e| e.to_string())?;
                    if !state {
                        ev.as_object_mut().map(|o| o.remove("state"));
                    }
                    output(format_args!("{ev}\n"))?;
                }
            }
            Ok(())
        }
        Cmd::Ask { name, question: Some(q), options, seen, recommend, why, at, context, wait, timeout, .. } => {
            let options: Vec<Value> = options.iter().map(|o| {
                let (id, rest) = o.split_once('=').ok_or_else(|| format!("--option {o:?}: use id=label"))?;
                let (label, does) = rest.split_once('|').unwrap_or((rest, ""));
                Ok(json!({"id": id.trim(), "label": label.trim(), "does": does.trim()}))
            }).collect::<Result<_, String>>()?;
            let anchor = at.map(|sel| json!({"selector": sel})).unwrap_or(Value::Null);
            let body = json!({"title": q, "options": options, "seen": seen, "recommend": recommend, "why": why, "anchor": anchor, "context": context, "by": by});
            let ev = send("POST", &api(&format!("/{}/asks", id(&name)?)), body)?;
            if !wait {
                return report(ev);
            }
            // stdout carries only the answer; the ask line still says where the card shows.
            eprintln!("{}", report_line(&ev));
            let (Some(jelly), Some(item)) = (ev["jelly"].as_str(), ev["args"]["id"].as_str()) else {
                return Err("the daemon did not say which ask it made".into());
            };
            let item = await_answer(&cli.url, jelly, item, timeout)?;
            if !json {
                output(format_args!("{}\n", answer_line(&item)))?;
                return Ok(());
            }
            let mut out = json!(item);
            out["jelly"] = json!(jelly);
            print(out)
        }
        Cmd::Waiting { name, all, mine } => {
            let mine = mine_name(mine, &cli.by)?;
            let one = name.map(|n| id(&n)).transpose()?;
            // One jelly is asked for by its full name, whatever project you are in.
            let p = if all || one.is_some() { String::new() } else { project() };
            let wanted = |jelly: &Value| one.as_ref().is_none_or(|n| jelly == n.as_str());
            let only = mine.as_ref().map(|m| format!("&mine={}", encode(m))).unwrap_or_default();
            let asks: Vec<Value> = get(&format!("{}/api/asks?project={}{only}", cli.url, encode(&p)))?.as_array().cloned().unwrap_or_default()
                .into_iter().filter(|a| wanted(&a["jelly"])).collect();
            let commented: Vec<String> = get(&api(""))?.as_array().into_iter().flatten()
                .filter(|j| wanted(&j["name"]) && (p.is_empty() || j["project"] == p.as_str()))
                .filter(|j| mine.as_ref().is_none_or(|m| j["owner"] == m.as_str()))
                .filter(|j| j["open_comments"].as_u64().unwrap_or(0) > 0)
                .filter_map(|j| j["name"].as_str().map(String::from)).collect();
            let mut comments = Vec::new();
            for n in &commented {
                for mut c in get(&api(&format!("/{n}")))?["comments"].as_array().cloned().unwrap_or_default() {
                    if c["resolved"] != true {
                        c["jelly"] = json!(n);
                        comments.push(c);
                    }
                }
            }
            if json {
                return print(json!({"asks": asks, "comments": comments}));
            }
            if asks.is_empty() && comments.is_empty() {
                let place = match &one { Some(n) => format!(" on {n}"), None if p.is_empty() => String::new(), None => format!(" in project {p} (--all for every project)") };
                return output(format_args!("Nothing is waiting{place}.\n"));
            }
            // Asks wait on the person; their comments wait on you.
            for a in &asks {
                let ask = store::Ask::deserialize(a).map_err(|e| e.to_string())?;
                output(format_args!("on them  {}  {}\n", a["jelly"].as_str().unwrap_or("?"), ask.summary()))?;
            }
            for c in &comments {
                let comment = store::Comment::deserialize(c).map_err(|e| e.to_string())?;
                output(format_args!("on you   {}\n", comment_line(c["jelly"].as_str().unwrap_or("?"), &comment)))?;
            }
            Ok(())
        }
        Cmd::Unask { name, id: aid } => report(send("POST", &api(&format!("/{}/asks/{}/unask", id(&name)?, encode(&aid))), json!({"by": by}))?),
        // A file instead of a name: one command from an HTML file to the person's screen.
        Cmd::Show { name: Some(file) } if file.ends_with(".html") && std::path::Path::new(&file).is_file() => {
            let path = PathBuf::from(&file);
            let short = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let view = read(&path)?;
            let full = id(&short)?;
            let exists = get(&api(""))?.as_array().into_iter().flatten().any(|j| j["name"] == full.as_str());
            let name = if exists {
                // The same file again changes the view in place; comments and state stay.
                if get(&api(&format!("/{full}")))?["view"] != view.as_str() {
                    send("POST", &api(&format!("/{full}/shape")), json!({"view": view, "by": by}))?;
                }
                if !json { output(format_args!("{full}: view taken from {file}\n"))?; }
                full
            } else {
                let about = view.split_once("<title>").and_then(|(_, rest)| rest.split_once("</title>")).map_or("", |(t, _)| t.trim());
                let body = json!({"name": short, "view": view, "verbs": "const verbs = {};", "state": {}, "by": by, "replace": false, "about": about, "project": project()});
                let made = send("POST", &api(""), body)?["jelly"].as_str().unwrap_or(&short).to_string();
                if !json { output(format_args!("{made}: made from {file}\n"))?; }
                made
            };
            show(json!(name))
        }
        Cmd::Show { name } => show(json!(name.map(|n| id(&n)).transpose()?)),
        Cmd::Feed { name: None, .. } => {
            let feeds = feed::read(&default_home()?)?;
            if feeds.is_empty() {
                return output(format_args!("No feeds.\n")).map_err(Into::into);
            }
            for f in feeds {
                output(format_args!("{}  {} · in {}\n", f.jelly, f.line(), f.cwd.display()))?;
            }
            Ok(())
        }
        Cmd::Feed { name: Some(name), verb, every, stop, command } => {
            // The file in the home is the feed: the daemon reads it and never
            // takes a command over HTTP.
            let home = default_home()?;
            let mut feeds = feed::read(&home)?;
            let jelly = id(&name)?;
            let had = feeds.len();
            feeds.retain(|f| f.jelly != jelly && f.jelly != name);
            if stop {
                if feeds.len() == had { return Err(format!("{jelly} has no feed").into()); }
                feed::write(&home, &feeds)?;
                return output(format_args!("{jelly}: feed stopped\n")).map_err(Into::into);
            }
            let verb = verb.ok_or("give the verb and the command: jelly feed NAME VERB -- COMMAND (or --stop)")?;
            if command.is_empty() { return Err("give the command after `--`: jelly feed NAME VERB -- COMMAND".into()); }
            if every < feed::FASTEST { return Err(format!("--every is in seconds, {} or more", feed::FASTEST).into()); }
            get(&api(&format!("/{jelly}/state")))?; // fail on a wrong name
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            let new = feed::Feed { jelly: jelly.clone(), verb, every, command, cwd };
            let line = new.line();
            feeds.push(new);
            feed::write(&home, &feeds)?;
            output(format_args!("{jelly}: feed {line}. `jelly look {name}` says how the last run went\n"))?;
            Ok(())
        }
        // `--done` sends no `what`, which clears it.
        Cmd::Working { name, what, .. } => report(send("POST", &api(&format!("/{}/working", id(&name)?)), json!({"what": what, "by": by}))?),
        Cmd::Ask { spec: None, .. } => Err("give a question (an ask on that jelly) or --spec FILE (an ask jelly)".into()),
        Cmd::Ask { name, spec: Some(spec), .. } => {
            let spec: Value = serde_json::from_str(&read(&spec)?).map_err(|e| format!("spec is not JSON: {e}"))?;
            // The store's rule: a spec without `questions` is one question.
            let questions = spec.get("questions").map_or(1, |q| q.as_array().map_or(0, Vec::len));
            let r = send("POST", &api(&format!("/{}/asks/round", id(&name)?)), json!({"spec": spec, "project": project(), "by": by}))?;
            let jelly = r["jelly"].as_str().unwrap_or(&name).to_string();
            let round = &r["state"]["round"];
            let since = format!("seq:{}", r["seq"]);
            if json {
                return print(json!({"jelly": jelly, "round": round, "questions": questions, "since": since, "next": format!("jelly hear --wait {jelly} --since {since} --json")}));
            }
            output(format_args!("{}\n", ask_line(&jelly, round, questions, &since)))?;
            Ok(())
        }
        Cmd::Hear { wait: false, wake: false, name: Some(_), .. } => Err("a name goes with --wait or --wake; `jelly hear` alone covers this project".into()),
        Cmd::Hear { wake: true, name, .. } => {
            let thread = std::env::var("CODEX_THREAD_ID").map_err(|_| "--wake needs an agent the daemon can wake, which is Codex today (CODEX_THREAD_ID is not set). Run `jelly hear --wait` in the background instead")?;
            let name = name.map(|n| id(&n)).transpose()?;
            send("POST", &format!("{}/api/wake", cli.url), json!({"jelly": name, "project": project(), "codex_thread": thread}))?;
            let what = name.unwrap_or_else(|| "one of this project's jellies".into());
            output(format_args!("When the person presses Send in {what}, the daemon queues a message to this Codex session, which starts your next turn. End your turn now.\n"))?;
            Ok(())
        }
        Cmd::Hear { wait: true, name, timeout, since, every, .. } => {
            let project_jelly = name.as_ref().filter(|_| since.is_some()).filter(|n| !n.contains(':') && *n != store::PAGE)
                .map(|n| { let p = project(); if p.is_empty() { n.clone() } else { format!("{p}:{n}") } });
            let name = name.map(|n| id(&n)).transpose()?;
            if let Some(SequenceToken(seq)) = since {
                let mut req = ureq::get(&format!("{}/api/wait", cli.url))
                    .query("since", &seq.to_string()).query("by", &by);
                if let Some(n) = &name { req = req.query("jelly", n); }
                if let Some(preferred) = project_jelly {
                    req = req.query("project_jelly", &preferred);
                }
                if let Some(t) = timeout { req = req.query("timeout", &t.to_string()); }
                if every { req = req.query("every", "true"); }
                let resp = req.call().map_err(describe)?;
                if resp.status() == 204 {
                    let what = name.as_deref().unwrap_or("any jelly");
                    eprintln!("jelly: nobody {} {what} in time", if every { "changed" } else { "pressed Send in" });
                    std::process::exit(2);
                }
                let ev = resp.into_json().map_err(|e| e.to_string())?;
                if json { return print(ev); }
                output(format_args!("{}\n", wait_line(&store::Event::deserialize(&ev).map_err(|e| e.to_string())?)))?;
                return Ok(());
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
                        eprintln!("jelly: nobody {} {what} in time", if every { "changed" } else { "pressed Send in" });
                        std::process::exit(2);
                    }
                    Err(e) => return Err(e.to_string().into()),
                };
                let Some(data) = line.strip_prefix("data:") else { continue };
                let ev: Value = serde_json::from_str(data.trim()).map_err(|e| e.to_string())?;
                if ev["by"] != by.as_str() && (every || ev["kind"] == "send") && (name.is_some() || ev["jelly"] != store::PAGE) {
                    if json {
                        return print(ev);
                    }
                    output(format_args!("{}\n", wait_line(&store::Event::deserialize(&ev).map_err(|e| e.to_string())?)))?;
                    return Ok(());
                }
            }
            Err("the event stream ended".into())
        }
        Cmd::Reply { name, id: cid, text: None, .. } => report(send("POST", &api(&format!("/{}/resolve", id(&name)?)), json!({"id": cid, "by": by}))?),
        Cmd::Reply { name, id: cid, text: Some(text), resolve } => report(send("POST", &api(&format!("/{}/reply", id(&name)?)), json!({"id": cid, "text": text, "resolve": resolve, "by": by}))?),
        Cmd::Move { name, to } => report(send("POST", &api(&format!("/{}/move", id(&name)?)), json!({"project": to, "by": by}))?),
        Cmd::Look { name, text, width, full, problems, poster, compare, save } => {
            let mut q = vec![];
            if let Some(c) = &compare { q.push(format!("compare={}", encode(c))); }
            if let Some(s) = &save { q.push(format!("save={}", encode(s))); }
            // Without a width the daemon renders desktop and phone side by side;
            // these flags are about one render, so they pick one width.
            let width = width.or((text || full || poster).then_some(1280));
            if let Some(w) = width { q.push(format!("width={w}")); }
            if width.is_some() && compare.is_none() && !text { q.push("png=true".into()); }
            if full { q.push("full=true".into()); }
            if json { q.push("json=true".into()); }
            let url = api(&format!("/{}/look?{}", id(&name)?, q.join("&")));
            // A jelly that changes while it renders (a live log, a monitor) answers 409;
            // a fresh render usually lands between changes.
            let mut tries = 0;
            let resp = loop {
                match ureq::get(&url).timeout(std::time::Duration::from_secs(60)).call() {
                    Err(ureq::Error::Status(409, _)) if tries < 2 => tries += 1,
                    other => break other.map_err(describe)?,
                }
            };
            let text = resp.into_string().map_err(|e| e.to_string())?;
            let seen: Option<Value> = json.then(|| serde_json::from_str(&text)).transpose().map_err(|e| e.to_string())?;
            if poster {
                let shot = match &seen {
                    Some(v) => v["png"].as_str(),
                    None => text.lines().find_map(|l| l.strip_prefix("png ")).map(str::trim),
                }.ok_or("no screenshot to use as the poster; see the `!` lines")?;
                let bytes = std::fs::read(shot).map_err(|e| format!("{shot}: {e}"))?;
                ureq::post(&api(&format!("/{}/assets/poster.png?by={}", id(&name)?, encode(&by))))
                    .set("content-type", "application/octet-stream")
                    .send_bytes(&bytes)
                    .map_err(describe)?;
            }
            if let Some(v) = seen {
                return print(v);
            } else if problems {
                let found: String = text.lines().filter(|l| l.starts_with("error ") || l.starts_with("! ")).map(|l| format!("{l}\n")).collect();
                output(format_args!("{}", if found.is_empty() { "no problems\n" } else { &found }))?;
            } else {
                output(format_args!("{text}"))?;
            }
            Ok(())
        }
        Cmd::Try { name, width, png, steps, .. } => {
            let url = api(&format!("/{}/try", id(&name)?));
            // Two renders with --png, each up to the daemon's 20 s limit.
            let resp = ureq::post(&url).timeout(std::time::Duration::from_secs(90))
                .send_json(json!({"steps": steps, "width": width, "png": png, "json": json})).map_err(describe)?;
            if json {
                return print(resp.into_json().map_err(|e| e.to_string())?);
            }
            output(format_args!("{}", resp.into_string().map_err(|e| e.to_string())?))?;
            Ok(())
        }
        Cmd::Note { name, text } => report(send("POST", &api(&format!("/{}/note", id(&name)?)), json!({"text": text, "by": by}))?),
        Cmd::Pull { name, dir } => {
            let name = id(&name)?;
            let short = name.split_once(':').map_or(name.as_str(), |(_, n)| n).to_string();
            let dir = dir.unwrap_or_else(|| PathBuf::from(&short));
            let j = get(&api(&format!("/{name}")))?;
            let inventory = get(&api(&format!("/{name}/assets")))
                .map_err(|e| format!("could not read asset inventory: {e}"))?;
            let files = serde_json::from_value::<Vec<String>>(inventory)
                .map_err(|e| format!("invalid asset inventory: {e}"))?;
            // This machine decides what it can store: a Unix daemon may hold a name Windows cannot.
            for file in &files {
                store::valid_file(file).map_err(|e| format!("cannot store asset {file} here: {e}"))?;
            }
            std::fs::create_dir_all(dir.join("assets")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("view.html"), j["view"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("verbs.js"), j["verbs"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("state.json"), serde_json::to_string_pretty(&j["state"]).unwrap_or_default()).map_err(|e| e.to_string())?;
            let assets_dir = format!("{}/j/{}/assets", cli.url, name);
            for file in &files {
                let response = ureq::get(&format!("{assets_dir}/{}", encode(file))).call()
                    .map_err(|e| format!("could not download asset {file}: {}", describe(e)))?;
                let mut bytes = Vec::new();
                response.into_reader().read_to_end(&mut bytes)
                    .map_err(|e| format!("could not read asset {file}: {e}"))?;
                std::fs::write(dir.join("assets").join(file), bytes)
                    .map_err(|e| format!("could not write asset {file}: {e}"))?;
            }
            let n = files.len();
            output(format_args!("{}/  view.html  verbs.js  state.json  assets/ ({n})\nEdit, then: jelly dev {name} {}\n", dir.display(), dir.display()))?;
            Ok(())
        }
        Cmd::Dev { name, dir } => {
            let name = id(&name)?;
            let short = name.split_once(':').map_or(name.as_str(), |(_, n)| n).to_string();
            let dir = dir.unwrap_or_else(|| PathBuf::from(&short));
            let stamp = |f: &str| std::fs::metadata(dir.join(f)).and_then(|m| m.modified()).ok();
            let (mut v, mut b) = (stamp("view.html"), stamp("verbs.js"));
            if v.is_none() && b.is_none() {
                return Err(format!("nothing to watch in {}: run jelly pull {name} first", dir.display()).into());
            }
            output(format_args!("watching {} for {name} (Ctrl-C to stop)\n", dir.display()))?;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let (nv, nb) = (stamp("view.html"), stamp("verbs.js"));
                if nv == v && nb == b { continue; }
                let view = (nv != v).then(|| std::fs::read_to_string(dir.join("view.html"))).transpose().map_err(|e| e.to_string())?;
                let verbs = (nb != b).then(|| std::fs::read_to_string(dir.join("verbs.js"))).transpose().map_err(|e| e.to_string())?;
                (v, b) = (nv, nb);
                match send("POST", &api(&format!("/{name}/shape")), json!({"view": view, "verbs": verbs, "by": by})) {
                    Ok(ev) => output(format_args!("shaped v{} ({})\n", ev["version"], [view.is_some().then_some("view"), verbs.is_some().then_some("verbs")].into_iter().flatten().collect::<Vec<_>>().join(" + ")))?,
                    Err(e) => { eprintln!("not shaped: {e}"); },
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
                    output(format_args!("{p}: DESIGN.md set. Views use it with <link rel=\"stylesheet\" href=\"/p/{p}/design.css\">\n"))?;
                    Ok(())
                }
                None => { output(format_args!("{}", ureq::get(&url).call().map_err(describe)?.into_string().map_err(|e| e.to_string())?))?; Ok(()) }
            }
        }
        Cmd::Asset { name, files, rm: true } => {
            for f in files {
                let file = f.to_string_lossy();
                ureq::delete(&api(&format!("/{}/assets/{}?by={}", id(&name)?, encode(&file), encode(&by)))).call().map_err(describe)?;
                output(format_args!("removed assets/{file}\n"))?;
            }
            Ok(())
        }
        Cmd::Asset { name, files, rm: false } => {
            let added = add_assets(&api(&format!("/{}", id(&name)?)), &files, &by)?;
            for line in added {
                output(format_args!("{line}\n"))?;
            }
            Ok(())
        }
        Cmd::Share { name, out, link, to, pin, hours, days } => {
            let name = id(&name)?;
            if pin {
                let hours = hours.unwrap_or(3);
                let made = send("POST", &api(&format!("/{name}/share/pin")), json!({"hours": hours, "days": days.unwrap_or(30)}))?;
                let (code, url) = (made["pin"].as_str().ok_or("the daemon did not say the PIN")?, made["url"].as_str().unwrap_or(""));
                let s = if hours == 1 { "" } else { "s" };
                return output(format_args!("PIN {code} at {url}\n  for a room: each person opens that page and types the PIN with their email; you hear them as person:<email>. The PIN lets people join for {hours} hour{s} and locks after {} wrong tries. `jelly unshare {name} --pin` ends it; people already in stay.\n", made["tries"]));
            }
            if link || to.is_some() {
                let days = days.unwrap_or(30);
                let made = send("POST", &api(&format!("/{name}/share")), json!({"to": to, "days": days}))?;
                let url = made["url"].as_str().ok_or("the daemon did not say the link")?;
                let s = if days == 1 { "" } else { "s" };
                let who = made["who"].as_str().unwrap_or("");
                return match to {
                    Some(_) => output(format_args!("{url}\n  for {who}: they can use {name} through it, and you hear them as person:{who}. Works {days} day{s}. `jelly unshare {name} --to {who}` stops it.\n")),
                    None => output(format_args!("{url}\n  view only: anyone with it watches {name} live but cannot change it. Works {days} day{s}. `jelly unshare {name} --link` stops it.\n")),
                };
            }
            if days.is_some() {
                return Err("--days is for live shares: add --to EMAIL, --link or --pin".into());
            }
            let resp = ureq::get(&api(&format!("/{name}/share"))).call().map_err(describe)?;
            let mut html = String::new();
            resp.into_reader().read_to_string(&mut html).map_err(|e| e.to_string())?;
            let short = name.split_once(':').map_or(name.as_str(), |(_, n)| n);
            let out = match out {
                Some(out) => out,
                None => {
                    let dir = std::env::temp_dir().join("jelly-share");
                    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                    dir.join(format!("{short}.html"))
                }
            };
            std::fs::write(&out, &html).map_err(|e| format!("{}: {e}", out.display()))?;
            output(format_args!("{}  ({} KB, read-only, opens anywhere)\n", out.display(), html.len().div_ceil(1024)))?;
            Ok(())
        }
        Cmd::Pr { name, pull, dry_run } => {
            let name = id(&name)?;
            let jelly = store::Jelly::deserialize(get(&api(&format!("/{name}")))?).map_err(|e| e.to_string())?;
            // Only the view-only link goes on a pull request: a person's own
            // link lets whoever opens it act as them.
            let shares = get(&format!("{}/api/shares", cli.url))?;
            let link = shares.as_array().into_iter().flatten().find(|s| s["jelly"] == name.as_str()).and_then(|s| s["links"][""].as_str());
            let look = ureq::get(&api(&format!("/{name}/look?width=1280&json=true"))).timeout(std::time::Duration::from_secs(60)).call()
                .map_err(describe).and_then(|r| r.into_json::<Value>().map_err(|e| e.to_string()));
            let outline = look.as_ref().ok().and_then(|seen| seen["outline"].as_str());
            if outline.is_none() {
                let why = look.as_ref().err().cloned().unwrap_or_else(|| format!("the page did not render; `jelly look {name}` says why"));
                eprintln!("jelly: the comment has no outline of the page: {why}");
            }
            let body = pr::comment(&jelly, link, outline);
            if dry_run {
                output(format_args!("{body}"))?;
            }
            let done = pr::post(&body, pull.as_deref(), &pr::marker(&name), dry_run)?;
            if dry_run {
                eprintln!("jelly: {name}: {done}");
                return Ok(());
            }
            output(format_args!("{name}: {done}\n"))
        }
        Cmd::Unshare { name, to, link, pin } => {
            let name = id(&name)?;
            if pin {
                send("DELETE", &api(&format!("/{name}/share/pin")), Value::Null)?;
                return output(format_args!("the PIN for {name} no longer works; people who joined with it stay\n"));
            }
            let q = match (&to, link) {
                (Some(who), _) => format!("?to={}", encode(who)),
                (None, true) => "?to=".into(),
                (None, false) => String::new(),
            };
            send("DELETE", &api(&format!("/{name}/share{q}")), Value::Null)?;
            match (to, link) {
                (Some(who), _) => output(format_args!("{who} can no longer use {name}; their open pages closed\n")),
                (None, true) => output(format_args!("the view-only link to {name} no longer works\n")),
                (None, false) => output(format_args!("{name} is no longer shared: every link stopped, and its final state is here\n")),
            }
        }
        Cmd::Relay { address } => {
            let home = relay::Home(default_home()?);
            match address {
                None => output(format_args!("{}\n", home.relay()?.url))?,
                Some(url) => {
                    let secret = home.set_relay(&url)?;
                    output(format_args!("relay: {}\nGive the relay the same publish secret (in relay/):\n  echo {secret} | npx wrangler secret put RELAY_SECRET\n", url.trim_end_matches('/')))?;
                }
            }
            Ok(())
        }
        Cmd::Hear { hook, mine, session_id, .. } => {
            let mine = mine_name(mine, &cli.by)?;
            inbox(&cli.url, &project(), hook, cli.by, mine, session_id)
        }
        Cmd::Open { name } => {
            let url = match (name.as_deref(), project().as_str()) {
                (Some(n), _) => format!("{}/j/{}/", cli.url, id(n)?),
                (None, "") => format!("{}/", cli.url),
                (None, p) => format!("{}/p/{p}", cli.url),
            };
            let browser = open_browser(&url);
            let printed = output(format_args!("{url}\n"));
            if let Err(e) = browser {
                eprintln!("jelly: could not open a browser ({e}); open {url} manually.");
            }
            printed
        }
    }
}

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(not(target_os = "windows"))]
    command.arg(url);

    #[cfg(target_os = "windows")]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("cmd.exe");
        // Delayed expansion keeps URL characters out of cmd's command parsing.
        command.args(["/d", "/v:on", "/c"])
            .raw_arg("start \"\" \"!JELLY_OPEN_URL!\"")
            .env("JELLY_OPEN_URL", url);
        command
    };

    let opener = command.get_program().to_string_lossy().into_owned();
    let status = command.status().map_err(|e| format!("{opener}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{opener}: {status}")) }
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

fn select_inbox_reader(
    named: Option<String>,
    session: Option<String>,
    claude: Option<String>,
    hook: impl FnOnce() -> Option<String>,
    parent: impl FnOnce() -> Result<store::InboxReader, String>,
) -> Result<store::InboxReader, String> {
    if let Some(name) = named {
        return Ok(store::InboxReader::Named { name });
    }
    if let Some(id) = session.filter(|s| !s.is_empty())
        .or_else(|| claude.filter(|s| !s.is_empty()))
        .or_else(|| hook().filter(|s| !s.is_empty()))
    {
        return Ok(store::InboxReader::Conversation { id });
    }
    parent()
}

fn hook_session(input: impl Read) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct HookInput { session_id: Option<String> }
    serde_json::from_reader::<_, HookInput>(input).ok()?.session_id
}

fn parent_reader() -> Result<store::InboxReader, String> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let current = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[current]), true);
    let parent = system.process(current).and_then(|p| p.parent()).ok_or("could not find invoking parent process")?;
    system.refresh_processes(ProcessesToUpdate::Some(&[parent]), true);
    let process = system.process(parent).ok_or("could not find parent process start time")?;
    Ok(store::InboxReader::Parent { pid: parent.as_u32(), start_time: process.start_time() })
}

/// Prints what the person did since the last call, then moves the cursor.
fn inbox(url: &str, project: &str, hook: bool, named: Option<String>, mine: Option<String>, session_id: Option<String>) -> Result<(), CommandError> {
    if hook && project.is_empty() {
        return Ok(()); // not in a repo: this session has no jellies of its own
    }
    let reader = select_inbox_reader(
        named,
        session_id,
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
        || {
            if !hook || std::io::stdin().is_terminal() { return None; }
            hook_session(std::io::stdin().lock())
        },
        parent_reader,
    );
    // A Claude Code session also hears, in one line, that its skill is behind.
    let session = match &reader {
        Ok(store::InboxReader::Conversation { id }) if hook => Some(id.clone()),
        _ => None,
    };
    deliver_inbox(url, project, hook, reader.map(|reader| store::InboxRequest { project: project.into(), reader, mine }), &mut std::io::stdout().lock())?;
    let homes = std::env::home_dir().zip(default_home().ok());
    if let Some(line) = session.zip(homes).and_then(|(session, (user, jelly))| update::hook_notice(url, &session, &user, &jelly, server::now_ms())) {
        output(format_args!("{line}\n"))?;
    }
    Ok(())
}

/// The owner name `--mine` filters by. Without a name every agent is "agent",
/// so "mine" would mean every unnamed agent's jellies; refuse instead.
fn mine_name(mine: bool, by: &Option<String>) -> Result<Option<String>, String> {
    match (mine, by) {
        (false, _) => Ok(None),
        (true, Some(name)) => Ok(Some(name.clone())),
        (true, None) => Err("--mine needs your name: --as NAME or JELLY_AS".into()),
    }
}

fn deliver_inbox(
    url: &str,
    project: &str,
    hook: bool,
    request: Result<store::InboxRequest, String>,
    output: &mut impl Write,
) -> Result<(), CommandError> {
    let result = (|| {
        let request = request?;
        let response = ureq::post(&format!("{url}/api/inbox"))
            .timeout(std::time::Duration::from_secs(1))
            .send_json(&request);
        let response = match response {
            // No daemon is silence; a daemon that does not answer is worth a line.
            Err(ureq::Error::Transport(t)) if hook && t.kind() == ureq::ErrorKind::ConnectionFailed => return Ok(()),
            result => result.map_err(describe)?,
        };
        let batch: store::InboxBatch = response.into_json().map_err(|e| e.to_string())?;
        write_inbox(output, project, hook, &batch).map_err(CommandError::Output)?;
        ureq::post(&format!("{url}/api/inbox/commit"))
            .timeout(std::time::Duration::from_secs(1))
            .send_json(store::InboxCommit { inbox: request, through: batch.through })
            .map_err(describe)?;
        Ok(())
    })();
    match result {
        Err(CommandError::Message(e)) if hook => writeln!(output, "jelly hear failed: {e}").map_err(CommandError::Output),
        result => result,
    }
}

fn write_inbox(output: &mut impl Write, project: &str, hook: bool, batch: &store::InboxBatch) -> std::io::Result<()> {
    if batch.events.is_empty() {
        if !hook {
            writeln!(output, "Nothing new from the person.")?;
        }
    } else {
        let scope = if project.is_empty() { String::new() } else { format!(" (project {project})") };
        writeln!(output, "The person acted on your jellies{scope} since you last checked:")?;
        for item in &batch.events {
            writeln!(output, "- {}: {}", item.event.jelly, item.text)?;
        }
        writeln!(output, "Details: `jelly state NAME`, `jelly waiting`. Answer on a comment with `jelly reply`.")?;
    }
    output.flush()
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

/// Blocks until ask `item` on `jelly` is answered or unasked, and
/// returns it. It subscribes before reading the item, and reads it again on
/// every event of that jelly or lag notification, so no answer is missed.
fn await_answer(url: &str, jelly: &str, item: &str, timeout: Option<u64>) -> Result<store::Ask, String> {
    let mut req = ureq::get(&format!("{url}/api/events?jelly={}", encode(jelly)));
    if let Some(t) = timeout {
        req = req.timeout(std::time::Duration::from_secs(t));
    }
    let settled = || -> Result<Option<store::Ask>, String> {
        let j = get(&format!("{url}/api/jellies/{jelly}"))?;
        let items = Vec::<store::Ask>::deserialize(&j["asks"]).map_err(|e| e.to_string())?;
        let found = items.into_iter().find(|a| a.id == item).ok_or_else(|| format!("{jelly} has no ask {item}"))?;
        Ok((!found.is_open()).then_some(found))
    };
    let resp = req.call().map_err(describe)?;
    read_answer(BufReader::new(resp.into_reader()), item, settled)
}

fn read_answer(
    events: impl BufRead,
    item: &str,
    mut settled: impl FnMut() -> Result<Option<store::Ask>, String>,
) -> Result<store::Ask, String> {
    if let Some(done) = settled()? {
        return Ok(done);
    }
    for line in events.lines() {
        match line {
            Ok(l) if l.starts_with("data:") || l == ": resync" => {
                if let Some(done) = settled()? {
                    return Ok(done);
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock => {
                eprintln!("jelly: nobody answered {item} in time");
                std::process::exit(2);
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("the event stream ended".into())
}

/// What the person made of an ask: `a1 answered: roomy · "keep the table"`.
fn answer_line(item: &store::Ask) -> String {
    let answer = match (&item.unasked, &item.answer) {
        (None, Some(answer)) => answer,
        _ => return format!("{} unasked", item.id),
    };
    let option = answer.option.clone().filter(|o| !o.is_empty());
    let note = Some(&answer.text).filter(|t| !t.is_empty()).map(|t| format!("{t:?}"));
    let what: Vec<String> = option.into_iter().chain(note).collect();
    if what.is_empty() {
        return format!("{} answered: seen", item.id);
    }
    format!("{} answered: {}", item.id, what.join(" · "))
}

/// One line on what a write did. The full event is in `jelly events`.
fn report(ev: Value) -> Result<(), CommandError> {
    output(format_args!("{}\n", report_line(&ev)))?;
    Ok(())
}

fn report_line(ev: &Value) -> String {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let what = match ev["kind"].as_str().unwrap_or("") {
        "do" => format!("{} ran", s(&ev["verb"])),
        "ask" => format!("asked {} {:?}; {}. Answers arrive in `jelly hear`{}", s(&ev["args"]["id"]), s(&ev["args"]["title"]), shown(ev),
            if ev["visible"] != 0 { "" }
            else if ev["notified"] == true { ". Nobody has it on screen, but their browser showed a notification" }
            else { ". Nobody has it on screen: tell the person, or `jelly show` it" }),
        "set" => if s(&ev["args"]["pointer"]).is_empty() { "state replaced".into() } else { format!("set {}", s(&ev["args"]["pointer"])) },
        "make" => "made".into(),
        k => k.to_string(),
    };
    let changed: Vec<String> = ev["changed"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).filter(|c| !c.is_empty()).map(String::from).collect();
    let changed = match changed.len() { 0 => String::new(), 1..=6 => format!(" · changed {}", changed.join(" ")), n => format!(" · changed {n} places") };
    format!("{} v{}: {what}{changed} · since seq:{}", s(&ev["jelly"]), ev["version"], ev["seq"])
}

/// What `jelly ask --spec` did, in one line.
fn ask_line(jelly: &str, round: &Value, questions: usize, since: &str) -> String {
    format!("{jelly} round {round}: asked {questions} question{}. Answers arrive in `jelly hear`, or block on `jelly hear --wait {jelly} --since {since}`", if questions == 1 { "" } else { "s" })
}

/// One comment and its thread, for an agent's to-do list.
fn comment_line(jelly: &str, c: &store::Comment) -> String {
    let thread = c.replies.last().map(|last| format!(" · {} repl{}, last by {}: {:?}", c.replies.len(), if c.replies.len() == 1 { "y" } else { "ies" }, last.by, last.text)).unwrap_or_default();
    format!("{jelly}  {} on {}{}: {:?}{thread}{}", c.id, store::comment_place(&c.anchor), store::named(&c.by), c.text, if c.resolved { " (resolved)" } else { "" })
}

/// The change that ended `jelly hear --wait`, in one line.
fn wait_line(e: &store::Event) -> String {
    format!("{} v{}: {} {}", e.jelly, e.version, e.by, e.describe())
}

/// Where an ask shows on the page, as the daemon found it: `anchor`
/// says what became of its `--at`, and is absent when nothing could check.
fn shown(ev: &Value) -> String {
    const STACK: &str = "it shows in jelly's stack of questions";
    let Some(at) = ev["args"]["anchor"]["selector"].as_str() else { return format!("{STACK}, unless the view draws it") };
    let drawn = ev["drawn"] == true;
    let place = if drawn { "the view draws it" } else { STACK };
    match server::AskAnchor::deserialize(&ev["anchor"]) {
        Ok(server::AskAnchor::Matched) if drawn => place.into(),
        Ok(server::AskAnchor::Matched) => format!("{place}, with a pin on {at}"),
        Ok(server::AskAnchor::Missing) if drawn => format!("{place}; {at} matches nothing on the page"),
        Ok(server::AskAnchor::Missing) => format!("{at} matches nothing on the page, so it has no pin; {place}"),
        Ok(server::AskAnchor::Invalid) if drawn => format!("{place}; {at} has invalid CSS selector syntax"),
        Ok(server::AskAnchor::Invalid) => format!("{at} has invalid CSS selector syntax, so it has no pin; {place}"),
        _ => format!("{STACK}, unless the view draws it; whether {at} is on the page was not checked ({})",
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
/// Resolves on Ctrl-C, or SIGTERM on Unix.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => { tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} } }
            Err(_) => { let _ = tokio::signal::ctrl_c().await; }
        }
    }
    #[cfg(not(unix))]
    { let _ = tokio::signal::ctrl_c().await; }
}

/// Holds `<home>/daemon.lock` exclusively, or says which daemon already does.
fn lock_home(home: &Path, port: u16) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(home).map_err(|e| format!("{}: {e}", home.display()))?;
    let path = home.join("daemon.lock");
    let mut file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if file.try_lock().is_err() {
        let mut other = String::new();
        let _ = file.read_to_string(&mut other);
        let at = other.trim();
        let at = if at.is_empty() { String::new() } else { format!(" at {at}") };
        return Err(format!("another jelly daemon{at} is already using {}; stop it, or pass a different --home", home.display()));
    }
    file.set_len(0).and_then(|()| file.write_all(format!("http://127.0.0.1:{port}\n").as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file)
}

/// A jelly in two lines: which one, then its state. The same block the person can
/// copy from the jelly page, so either side can point at a jelly exactly.
fn reference(j: &Value) -> String {
    let n = |k: &str| j[k].as_u64().unwrap_or(0);
    let mut facts = vec![
        format!("made by {} {}", j["made_by"].as_str().unwrap_or("?"), server::ago(n("created_ms"))),
        format!("updated {}", server::ago(n("updated_ms"))),
        format!("v{}", n("version")),
    ];
    if n("open_asks") > 0 { facts.push(format!("{} waiting on the person", n("open_asks"))); }
    if n("open_comments") > 0 { facts.push(format!("{} open comments", n("open_comments"))); }
    if j["kept"].as_bool() == Some(true) { facts.push("kept".into()); }
    if j["archived"].as_bool() == Some(true) { facts.push("archived".into()); }
    format!("{} · {}\n  {}", j["name"].as_str().unwrap_or("?"), j["about"].as_str().unwrap_or(""), facts.join(" · "))
}

fn add_assets(jelly: &str, files: &[PathBuf], by: &str) -> Result<Vec<String>, String> {
    files.iter().map(|f| {
        let file = f.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{}: not a file name", f.display()))?;
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        upload_asset(jelly, file, &bytes, by)
    }).collect()
}

fn upload_asset(jelly: &str, file: &str, bytes: &[u8], by: &str) -> Result<String, String> {
    ureq::post(&format!("{jelly}/assets/{}?by={}", encode(file), encode(by)))
        .set("content-type", "application/octet-stream")
        .send_bytes(bytes)
        .map_err(describe)?;
    Ok(format!("assets/{file}  ({} KB)", bytes.len().div_ceil(1024)))
}

/// Where jellies are kept when no --home is given: $JELLY_HOME or ~/.jelly.
fn default_home() -> Result<PathBuf, String> {
    std::env::var_os("JELLY_HOME").map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".jelly")))
        .ok_or_else(|| "could not find your home directory: set --home or JELLY_HOME".into())
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
        ureq::Error::Transport(t) => format!("cannot reach the selected daemon ({t}); check --url / JELLY_URL and the server"),
    }
}

fn print(v: Value) -> Result<(), CommandError> {
    output(format_args!("{}\n", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?))?;
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
    use super::{answer_line, shown, wait_line, write_output};
    use std::io::{self, Write};
    use serde_json::json;

    #[test]
    fn try_steps_keep_their_order_across_flags() {
        use clap::CommandFactory;
        let m = super::Cli::command().get_matches_from(["jelly", "try", "x", "--click", "#a", "--type", "[name=q]=x=y", "--press", "#q=Enter", "--click", "#b"]);
        let steps = super::try_steps(m.subcommand_matches("try").unwrap()).unwrap();
        assert_eq!(steps, [
            json!({"do": "click", "at": "#a"}),
            json!({"do": "type", "at": "[name=q]", "text": "x=y"}),
            json!({"do": "press", "at": "#q", "key": "Enter"}),
            json!({"do": "click", "at": "#b"}),
        ]);
    }

    #[test]
    fn no_subcommand_argument_shadows_a_global_one() {
        // A subcommand argument with a global's id silently takes the global's value
        // (`jelly relay` once saved --url as the relay).
        use clap::CommandFactory;
        let cmd = super::Cli::command();
        let globals: Vec<_> = cmd.get_arguments().filter(|a| a.is_global_set()).map(|a| a.get_id().clone()).collect();
        for sub in cmd.get_subcommands() {
            for arg in sub.get_arguments() {
                assert!(!globals.contains(arg.get_id()), "jelly {}: argument {:?} shadows the global one", sub.get_name(), arg.get_id());
            }
        }
    }

    #[test]
    fn startup_precedes_dispatch_and_hooks_suppress_its_errors() {
        use super::{run_with_start, Cli};
        use clap::Parser;
        let parse = |args: &[&str]| Cli::try_parse_from(args).unwrap();
        let command = super::daemon_command().unwrap();
        assert_eq!(command.get_program(), std::env::current_exe().unwrap());
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["serve"]);
        assert_eq!(command.get_envs().count(), 0, "inherit home and other environment defaults");
        let cli = parse(&["jelly", "--url", super::startup::DEFAULT_URL, "open"]);
        let result = run_with_start(cli, |url, quiet| {
            assert_eq!(url, super::startup::DEFAULT_URL);
            assert!(!quiet);
            Err("startup failed before opening a browser".into())
        });
        assert_eq!(result.unwrap_err().to_string(), "startup failed before opening a browser");
        for alias in ["http://localhost:7878/", "HTTP://127.1:7878"] {
            let cli = parse(&["jelly", "--url", alias, "open"]);
            assert_eq!(run_with_start(cli, |url, quiet| {
                assert_eq!(url, super::startup::DEFAULT_URL);
                assert!(!quiet);
                Err("alias startup".into())
            }).unwrap_err().to_string(), "alias startup");
        }
        for project in ["", "test"] {
            let cli = parse(&["jelly", "--url", super::startup::DEFAULT_URL, "--project", project, "hear", "--hook"]);
            assert!(run_with_start(cli, |_, quiet| {
                assert!(quiet);
                Err("startup failed".into())
            }).is_ok());
        }
        let cli = parse(&["jelly", "--url", super::startup::DEFAULT_URL, "--project", "", "hear", "--hook"]);
        let mut called = false;
        assert!(run_with_start(cli, |_, quiet| { called = true; assert!(quiet); Ok(()) }).is_ok());
        assert!(called, "outside-project hook must start before its early return");

        let home = tempfile::NamedTempFile::new().unwrap();
        let cli = parse(&["jelly", "serve", "--port", "0", "--home", home.path().to_str().unwrap()]);
        assert!(run_with_start(cli, |_, _| panic!("serve must bypass startup")).is_err());
        let cli = parse(&["jelly", "--url", "http://127.0.0.1:9999", "--project", "", "hear", "--hook"]);
        assert!(run_with_start(cli, |_, _| panic!("custom URL must bypass startup")).is_ok());
    }

    #[test]
    fn output_preserves_write_and_flush_errors() {
        struct Fault { kind: io::ErrorKind, on_flush: bool }
        impl Write for Fault {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.on_flush { Ok(bytes.len()) } else { Err(self.kind.into()) }
            }
            fn flush(&mut self) -> io::Result<()> { Err(self.kind.into()) }
        }
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::PermissionDenied] {
            for on_flush in [false, true] {
                let error = write_output(&mut Fault { kind, on_flush }, format_args!("no newline")).unwrap_err();
                assert_eq!(error.kind(), kind);
            }
        }
    }

    #[test]
    fn wait_prints_the_change_in_one_line() {
        let ev: crate::store::Event = serde_json::from_value(json!({"seq": 9, "at_ms": 1, "jelly": "p:x", "kind": "answer", "by": "person", "version": 4, "view_rev": 1,
            "args": {"id": "a1", "title": "Which order?", "option": "b", "text": "keep it"}, "state": {"big": "state"}})).unwrap();
        assert_eq!(wait_line(&ev), "p:x v4: person answered a1 \"Which order?\" → b: \"keep it\"");
    }

    #[test]
    fn comments_name_a_person_who_named_themselves() {
        let comment = |by: &str| serde_json::from_value::<crate::store::Comment>(json!({"id": "c1", "anchor": {"selector": "#a"}, "text": "bigger",
            "by": by, "at_ms": 1, "resolved": false})).unwrap();
        assert_eq!(super::comment_line("p:x", &comment("person:ana")), r#"p:x  c1 on #a by ana: "bigger""#);
        assert_eq!(super::comment_line("p:x", &comment("person")), r#"p:x  c1 on #a: "bigger""#);
    }

    fn inbox_response(body: &str) -> (String, std::thread::JoinHandle<std::net::TcpListener>) {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_owned();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut input = std::io::BufReader::new(&stream);
            let mut line = String::new();
            input.read_line(&mut line).unwrap();
            assert_eq!(line.trim(), "POST /api/inbox HTTP/1.1");
            let mut length = 0;
            loop {
                line.clear();
                input.read_line(&mut line).unwrap();
                if line == "\r\n" { break; }
                if let Some((key, value)) = line.split_once(':') {
                    if key.eq_ignore_ascii_case("content-length") { length = value.trim().parse().unwrap(); }
                }
            }
            input.read_exact(&mut vec![0; length]).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            listener
        });
        (url, server)
    }

    fn assert_no_inbox_commit(server: std::thread::JoinHandle<std::net::TcpListener>) {
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn inbox_reader_failures_are_visible_in_hooks() {
        for error in ["could not find invoking parent process", "could not find parent process start time"] {
            let mut output = Vec::new();
            super::deliver_inbox("unused", "p", true, Err(error.into()), &mut output).unwrap();
            assert_eq!(String::from_utf8(output).unwrap(), format!("jelly hear failed: {error}\n"));
            assert_eq!(super::deliver_inbox("unused", "p", false, Err(error.into()), &mut Vec::new()).unwrap_err().to_string(), error);
        }
    }

    #[test]
    fn inbox_undecodable_responses_do_not_commit() {
        for body in ["broken", "", "[]", r#"{"events":[{"jelly":"p:sample"}],"through":4}"#] {
            let (url, server) = inbox_response(body);
            let request = crate::store::InboxRequest { project: "p".into(), reader: crate::store::InboxReader::Named { name: "a".into() }, mine: None };
            assert!(super::deliver_inbox(&url, "p", false, Ok(request), &mut Vec::new()).is_err());
            assert_no_inbox_commit(server);
        }
    }

    #[test]
    fn inbox_output_failures_do_not_commit() {
        use std::io::{self, Write};
        struct FailingOutput { fail_write: bool }
        impl Write for FailingOutput {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.fail_write { Err(io::Error::other("output failed")) } else { Ok(bytes.len()) }
            }
            fn flush(&mut self) -> io::Result<()> { Err(io::Error::other("flush failed")) }
        }
        for fail_write in [true, false] {
            let body = json!({"events": [{"seq": 4, "at_ms": 1, "jelly": "p:sample", "kind": "do",
                "by": "person", "version": 1, "view_rev": 1, "text": "click {}"}], "through": 4}).to_string();
            let (url, server) = inbox_response(&body);
            let request = crate::store::InboxRequest { project: "p".into(), reader: crate::store::InboxReader::Named { name: "a".into() }, mine: None };
            let error = super::deliver_inbox(&url, "p", false, Ok(request), &mut FailingOutput { fail_write }).unwrap_err();
            assert_eq!(error.to_string(), if fail_write { "output failed" } else { "flush failed" });
            assert_no_inbox_commit(server);
        }
    }

    #[test]
    fn inbox_identity_precedence_and_namespaces() {
        use super::{hook_session, select_inbox_reader};
        use crate::store::InboxReader;
        let named = |name: &str| InboxReader::Named { name: name.into() };
        let conversation = |id: &str| InboxReader::Conversation { id: id.into() };
        let parent = || Ok(InboxReader::Parent { pid: 42, start_time: 100 });
        let resolve = |name: Option<&str>, session: Option<&str>, claude: Option<&str>, hook: Option<&str>| {
            select_inbox_reader(name.map(str::to_string), session.map(str::to_string), claude.map(str::to_string), || hook.map(str::to_string), parent).unwrap()
        };
        assert_eq!(resolve(Some("agent"), Some("session"), Some("claude"), Some("hook")), named("agent"));
        assert_eq!(resolve(None, Some("session"), Some("claude"), Some("hook")), conversation("session"));
        assert_eq!(resolve(None, Some(""), Some("claude"), Some("hook")), conversation("claude"));
        assert_eq!(resolve(None, None, Some(""), Some("hook")), conversation("hook"));
        assert_eq!(resolve(None, None, None, None), parent().unwrap());
        assert_ne!(named("agent"), conversation("agent"));
        assert_ne!(parent().unwrap(), InboxReader::Parent { pid: 42, start_time: 101 });
        assert_eq!(select_inbox_reader(Some("a".into()), None, None, || panic!("must not read stdin"), || panic!("must not inspect processes")).unwrap(), named("a"));
        assert_eq!(select_inbox_reader(None, Some("a".into()), None, || panic!("must not read stdin"), || panic!("must not inspect processes")).unwrap(), conversation("a"));
        assert!(select_inbox_reader(None, None, None, || None, || Err("no parent".into())).is_err());
        assert_eq!(hook_session(&b"{\"session_id\":\"resumed\",\"prompt\":\"ignored\"}"[..]), Some("resumed".into()));
        for input in ["", "broken", "{}"] {
            assert_eq!(hook_session(input.as_bytes()), None);
        }
    }

    #[test]
    fn an_answer_is_reread_after_an_sse_resync() {
        for notification in [": resync", "data: {}"] {
            let events = format!("{notification}\n\n");
            let mut reads = 0;
            let answer = super::read_answer(events.as_bytes(), "a1", || {
                reads += 1;
                match reads {
                    1 => Ok(None),
                    2 => Ok(Some(serde_json::from_value(json!({
                        "id":"a1", "kind":"ask", "title":"Q", "by":"agent", "at_ms":1,
                        "answer":{"option":"b", "text":"", "by":"person", "at_ms":2}
                    })).unwrap())),
                    _ => panic!("unexpected attention read"),
                }
            }).unwrap();
            assert_eq!(reads, 2);
            assert_eq!(answer.answer.unwrap().option.as_deref(), Some("b"));
        }
    }

    #[test]
    fn an_answer_is_one_line() {
        let item = |extra: serde_json::Value| {
            let mut a = json!({"id": "a1", "title": "Q", "by": "agent", "at_ms": 1});
            a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            answer_line(&serde_json::from_value(a).unwrap())
        };
        let answered = |option: serde_json::Value, text: &str| item(json!({"answer": {"option": option, "text": text, "by": "person", "at_ms": 2}}));
        assert_eq!(answered(json!("roomy"), "but keep the dense table"), r#"a1 answered: roomy · "but keep the dense table""#);
        assert_eq!(answered(json!("roomy"), ""), "a1 answered: roomy");
        assert_eq!(answered(json!(null), "neither"), r#"a1 answered: "neither""#);
        assert_eq!(item(json!({"unasked": {"by": "agent", "at_ms": 2}})), "a1 unasked");
    }

    #[test]
    fn an_ask_that_was_not_checked_says_so() {
        let unchecked = shown(&json!({"args": {"anchor": {"selector": "#list"}}, "anchor_unchecked": "no Chrome found"}));
        assert!(unchecked.contains("whether #list is on the page was not checked (no Chrome found)"), "{unchecked}");
        assert_eq!(shown(&json!({"args": {"anchor": null}})), "it shows in jelly's stack of questions, unless the view draws it");
    }
}
