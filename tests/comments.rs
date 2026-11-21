#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::process::{Child, Command};

/// A daemon on a port it picked itself and a throwaway home, killed on drop.
struct Daemon {
    url: String,
    child: Child,
    _home: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn daemon() -> Daemon {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path().join("jelly"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut child);
    Daemon { url, child, _home: home }
}

fn post(d: &Daemon, path: &str, body: Value) {
    ureq::post(&format!("{}/api/jellies{path}", d.url)).send_json(body).unwrap();
}

fn new_jelly(d: &Daemon, name: &str) {
    post(d, "", json!({"name": name, "view": "<p>x</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}));
}

/// Runs `jelly waiting --json` in project `t` and returns `(jelly, id)` pairs.
fn comments(d: &Daemon, extra: &[&str]) -> Vec<(String, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--url", &d.url, "--project", "t", "waiting", "--json"])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let waiting: Value = serde_json::from_slice(&out.stdout).unwrap();
    waiting["comments"].as_array().unwrap().iter().map(|c| (c["jelly"].as_str().unwrap().into(), c["id"].as_str().unwrap().into())).collect()
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter().map(|(j, c)| (j.to_string(), c.to_string())).collect()
}

/// The CLI reads comments from the daemon's serialized jelly, so a rename on
/// either side shows up here as missing comments.
#[test]
fn comments_lists_saved_comments_by_jelly_and_project() {
    let d = daemon();
    for name in ["t:a", "t:b", "u:c"] {
        new_jelly(&d, name);
    }
    post(&d, "/t:a/comment", json!({"text": "open", "by": "person"}));
    post(&d, "/t:a/comment", json!({"text": "done", "by": "person"}));
    post(&d, "/t:a/resolve", json!({"id": "c2", "by": "person"}));
    post(&d, "/t:b/comment", json!({"text": "done", "by": "person"}));
    post(&d, "/t:b/resolve", json!({"id": "c1", "by": "person"}));
    post(&d, "/u:c/comment", json!({"text": "elsewhere", "by": "person"}));

    // Only open comments wait on the agent; a resolved one is dealt with.
    assert_eq!(comments(&d, &["a"]), pairs(&[("t:a", "c1")]));
    assert_eq!(comments(&d, &[]), pairs(&[("t:a", "c1")]));
    let mut all = comments(&d, &["--all"]);
    all.sort();
    assert_eq!(all, pairs(&[("t:a", "c1"), ("u:c", "c1")]), "--all is every project");
}

/// `--mine` keeps the jellies made, or last shaped, under this agent's `--as` name.
#[test]
fn comments_mine_lists_only_this_agents_jellies() {
    let d = daemon();
    for (name, agent) in [("t:a", "ana"), ("t:b", "bo")] {
        post(&d, "", json!({"name": name, "view": "<p>x</p>", "verbs": "const verbs = {};", "state": {}, "by": agent}));
        post(&d, &format!("/{name}/comment"), json!({"text": "bigger", "by": "person"}));
    }
    assert_eq!(comments(&d, &["--as", "ana", "--mine"]), pairs(&[("t:a", "c1")]));
    assert_eq!(comments(&d, &["--as", "bo", "--mine"]), pairs(&[("t:b", "c1")]));
    assert_eq!(comments(&d, &["--as", "ana"]).len(), 2);

    let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--url", &d.url, "--project", "t", "waiting", "--mine"])
        .env_remove("JELLY_AS")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--mine needs your name"), "{out:?}");
}

/// A comment with no anchor is about the whole jelly: the agent hears it as
/// such in the inbox (the hook too) and `jelly waiting`, and the jelly page
/// gets its thread from the list rather than pinning it in the view.
#[test]
fn a_whole_jelly_comment_reaches_the_agent_and_its_reply_reaches_the_page() {
    let d = daemon();
    let client = tempfile::tempdir().unwrap();
    let jelly = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["--url", &d.url, "--project", "t", "--as", "claude"])
            .args(args)
            .env("JELLY_HOME", client.path())
            .env("HOME", client.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    new_jelly(&d, "t:a");
    post(&d, "/t:a/comment", json!({"anchor": {"selector": "#hero", "quote": "Hero"}, "text": "bigger", "by": "person"}));
    post(&d, "/t:a/comment", json!({"anchor": null, "text": "Why three steps?", "by": "person"}));
    let hook = jelly(&["hear", "--hook"]);
    assert!(hook.contains(r#"t:a: commented on "Hero": bigger"#), "{hook}");
    assert!(hook.contains("t:a: commented on the whole jelly, not one part: Why three steps?"), "{hook}");

    post(&d, "/t:a/comment", json!({"text": "Can it be two?", "by": "person"}));
    let inbox = jelly(&["hear"]);
    assert!(inbox.contains("t:a: commented on the whole jelly, not one part: Can it be two?"), "{inbox}");

    let listed = jelly(&["waiting", "a"]);
    assert!(listed.contains(r#"t:a  c1 on #hero: "bigger""#), "{listed}");
    assert!(listed.contains(r#"t:a  c2 on the whole jelly: "Why three steps?""#), "{listed}");

    jelly(&["reply", "a", "c2", "Each step is one PR."]);
    let list: Value = ureq::get(&format!("{}/api/jellies", d.url)).call().unwrap().into_json().unwrap();
    let whole = &list[0]["whole_jelly_comments"];
    let ids: Vec<&str> = whole.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["c2", "c3"], "element comments stay out: {whole}");
    assert_eq!(whole[0]["replies"][0]["text"], "Each step is one PR.");
    assert_eq!(list[0]["replies_waiting"], 1);
}
