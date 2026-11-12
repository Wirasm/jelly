#![cfg(unix)]

use serde_json::{json, Value};
use std::process::{Child, Command};
use std::time::Duration;

/// A daemon on a free port and a throwaway home, killed on drop.
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
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", &port.to_string(), "--home"])
        .arg(home.path().join("jelly"))
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let d = Daemon { url: format!("http://127.0.0.1:{port}"), child, _home: home };
    for _ in 0..100 {
        if ureq::get(&format!("{}/api/jellies", d.url)).call().is_ok() {
            return d;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the daemon did not start");
}

fn post(d: &Daemon, path: &str, body: Value) {
    ureq::post(&format!("{}/api/jellies{path}", d.url)).send_json(body).unwrap();
}

fn new_jelly(d: &Daemon, name: &str) {
    post(d, "", json!({"name": name, "view": "<p>x</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}));
}

/// Runs `jelly comments` in project `t` and returns `(jelly, id)` pairs.
fn comments(d: &Daemon, extra: &[&str]) -> Vec<(String, String)> {
    let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--url", &d.url, "--project", "t", "comments"])
        .args(extra)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let list: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    list.iter().map(|c| (c["jelly"].as_str().unwrap().into(), c["id"].as_str().unwrap().into())).collect()
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

    assert_eq!(comments(&d, &["a"]), pairs(&[("t:a", "c1")]));
    assert_eq!(comments(&d, &["a", "--all"]), pairs(&[("t:a", "c1"), ("t:a", "c2")]));

    let mut all = comments(&d, &["--all"]);
    all.sort();
    assert_eq!(all, pairs(&[("t:a", "c1"), ("t:a", "c2"), ("t:b", "c1")]));
    assert_eq!(comments(&d, &[]), pairs(&[("t:a", "c1")]));
}
