//! `--json`: what scripts read instead of parsing the text.
#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};

struct Daemon(Child, String);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn daemon(home: &std::path::Path) -> Daemon {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut child);
    Daemon(child, url)
}

impl Daemon {
    /// Runs `jelly ARGS --json` in project `t` and parses what it printed.
    fn json(&self, args: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["--url", &self.1, "--project", "t"]).args(args).arg("--json")
            .env_remove("JELLY_AS").stdin(Stdio::null()).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stdout)))
    }
}

/// Every change prints the daemon's event: its kind, seq and version, what it
/// changed, and for an ask or look the item's id.
#[test]
fn changes_print_their_event() {
    let dir = tempfile::tempdir().unwrap();
    let d = daemon(&dir.path().join("home"));
    let file = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path.to_str().unwrap().to_string()
    };
    let view = file("view.html", r#"<ul id="list"></ul>"#);
    let verbs = file("verbs.js", "const verbs = { add(s, { title }) { s.items = (s.items || []).concat([title]); } };");

    let made = d.json(&["new", "x", "--view", &view, "--verbs", &verbs]);
    assert_eq!((made["kind"].as_str(), made["jelly"].as_str(), made["version"].as_u64()), (Some("new"), Some("t:x"), Some(1)), "{made}");
    let called = d.json(&["call", "x", "add", r#"{"title":"Milk"}"#]);
    assert_eq!((called["kind"].as_str(), called["verb"].as_str()), (Some("call"), Some("add")), "{called}");
    assert_eq!(called["changed"], json!(["/items"]));
    assert!(called["seq"].as_u64() > made["seq"].as_u64(), "{called}");
    let set = d.json(&["set", "x", "--at", "/title", r#""Shop""#]);
    assert_eq!((set["kind"].as_str(), &set["changed"]), (Some("set"), &json!(["/title"])), "{set}");
    let reshaped = d.json(&["reshape", "x", "--about", "Groceries"]);
    assert_eq!(reshaped["kind"], "reshape", "{reshaped}");
    let undone = d.json(&["undo", "x"]);
    assert_eq!(undone["kind"], "undo", "{undone}");

    let asked = d.json(&["ask", "x", "Which first?", "--option", "a=Milk", "--option", "b=Eggs"]);
    assert_eq!((asked["kind"].as_str(), asked["args"]["id"].as_str()), (Some("attention"), Some("a1")), "{asked}");
    let looked = d.json(&["look", "x", "The list"]);
    assert_eq!(looked["args"]["id"], "a2", "{looked}");
    let withdrawn = d.json(&["withdraw", "x", "a1"]);
    assert_eq!(withdrawn["kind"], "withdraw", "{withdrawn}");

    // A command with nothing to print as JSON says so instead of printing text.
    let share = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1, "share", "t:x", "--json"]).output().unwrap();
    assert!(!share.status.success() && share.stdout.is_empty(), "{share:?}");
}

/// `see --json` splits what `see` prints: the summary, the problem lines, the
/// screenshot's path and the outline of what is on screen.
#[test]
fn see_prints_its_parts() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    let view = r#"<p id="x">fine</p><script>throw new Error("boom in view")</script>"#;
    ureq::post(&format!("{}/api/jellies", d.1))
        .send_json(json!({"name": "t:p", "view": view, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let seen = d.json(&["see", "p", "--png"]);
    assert_eq!((seen["jelly"].as_str(), seen["version"].as_u64()), (Some("t:p"), Some(1)), "{seen}");
    assert!(seen["summary"].as_str().unwrap().starts_with("t:p · "), "{seen}");
    let problems = seen["problems"].as_array().unwrap();
    assert!(problems.len() == 1 && problems[0].as_str().unwrap().starts_with("error ") && problems[0].as_str().unwrap().contains("boom in view"), "{seen}");
    assert!(std::path::Path::new(seen["png"].as_str().unwrap()).is_file(), "{seen}");
    assert!(seen["outline"].as_str().unwrap().contains("fine"), "{seen}");
}

/// `try --json` gives the steps, each verb call with what it changed or why it
/// was refused, the step that failed, and the state the trial ended with.
#[test]
fn try_prints_steps_calls_and_errors() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    let view = r#"<button id="add" onclick="jelly.call('add', {title: 'Milk'}).catch(() => {})">Add</button>
        <button id="bad" onclick="jelly.call('add', {}).catch(() => {})">Bad</button>"#;
    ureq::post(&format!("{}/api/jellies", d.1))
        .send_json(json!({"name": "t:x", "view": view, "verbs": "const verbs = { add(s, { title }) { if (!title) throw new Error('no title'); s.items.push(title); } };", "state": {"items": []}, "by": "test"}))
        .unwrap();
    let tried = d.json(&["try", "x", "--click", "#add", "--click", "#bad", "--click", "#nope"]);
    assert_eq!(tried["steps"], json!([{"do": "click", "at": "#add"}, {"do": "click", "at": "#bad"}, {"do": "click", "at": "#nope"}]), "{tried}");
    let calls = tried["calls"].as_array().unwrap();
    assert_eq!((&calls[0]["verb"], &calls[0]["changed"], &calls[0]["refused"]), (&json!("add"), &json!(["/items"]), &Value::Null), "{tried}");
    assert!(calls[1]["refused"].as_str().unwrap().contains("no title"), "{tried}");
    assert_eq!(tried["failed"]["step"], 2, "{tried}");
    assert_eq!(tried["state"], json!({"items": ["Milk"]}), "{tried}");
    assert!(tried["outline"].as_str().unwrap().contains("Add"), "{tried}");
}
