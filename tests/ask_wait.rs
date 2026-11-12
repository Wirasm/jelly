#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Output};
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

/// Starts a daemon whose headless browser is a stub that sends no snapshot,
/// so an ask with `--at` returns only after the anchor check gives up.
fn daemon() -> Daemon {
    let home = tempfile::tempdir().unwrap();
    let browser = home.path().join("browser");
    std::fs::write(&browser, "#!/bin/sh\nsleep 1\n").unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", &port.to_string(), "--home"])
        .arg(home.path().join("jelly"))
        .env("JELLY_CHROME", &browser)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let d = Daemon { url: format!("http://127.0.0.1:{port}"), child, _home: home };
    for _ in 0..100 {
        if ureq::get(&format!("{}/api/jellies", d.url)).call().is_ok() {
            ureq::post(&format!("{}/api/jellies", d.url))
                .send_json(json!({"name": "t:q", "view": "<p id=x>x</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
                .unwrap();
            return d;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the daemon did not start");
}

/// On the first attention event, waits `after`, then posts `action` to that
/// item as the person would.
fn react(url: &str, after: Duration, action: &'static str, body: Value) -> std::thread::JoinHandle<()> {
    let url = url.to_string();
    let resp = ureq::get(&format!("{url}/api/events?jelly=t:q")).call().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(resp.into_reader()).lines() {
            let line = line.unwrap();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let ev: Value = serde_json::from_str(data.trim()).unwrap();
            if ev["kind"] == "attention" {
                let item = ev["args"]["id"].as_str().unwrap();
                std::thread::sleep(after);
                ureq::post(&format!("{url}/api/jellies/t:q/attention/{item}/{action}")).send_json(body).unwrap();
                return;
            }
        }
    })
}

fn ask(url: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--url", url, "ask", "t:q", "Q", "--option", "a=A", "--option", "b=B", "--wait"])
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn an_answer_given_before_the_asker_listens_is_returned() {
    let d = daemon();
    let person = react(&d.url, Duration::ZERO, "answer", json!({"option": "b", "text": "keep it short", "by": "person"}));
    let out = ask(&d.url, &["--at", "#x", "--timeout", "20"]);
    person.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "a1 answered: b · \"keep it short\"\n");
}

#[test]
fn an_answer_given_while_the_asker_listens_is_returned() {
    let d = daemon();
    // Ample time for the asker to subscribe and find the item still open.
    let person = react(&d.url, Duration::from_secs(2), "answer", json!({"option": "a", "by": "person"}));
    let out = ask(&d.url, &["--timeout", "20"]);
    person.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "a1 answered: a\n");
}

#[test]
fn a_withdrawn_ask_returns_and_json_prints_the_item() {
    let d = daemon();
    let other = react(&d.url, Duration::ZERO, "withdraw", json!({"by": "other"}));
    let out = ask(&d.url, &["--json", "--timeout", "20"]);
    other.join().unwrap();
    assert!(out.status.success(), "{out:?}");
    let item: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(item["id"], "a1");
    assert_eq!(item["jelly"], "t:q");
    assert_eq!(item["withdrawn"]["by"], "other");
}

#[test]
fn an_unanswered_ask_times_out_with_exit_2() {
    let d = daemon();
    let out = ask(&d.url, &["--timeout", "1"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");
}
