mod common;

use serde_json::{json, Value};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fed(home: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(home.join("feeds.json")) else { return Vec::new() };
    serde_json::from_str::<Vec<Value>>(&text).unwrap().iter().map(|f| f["jelly"].as_str().unwrap().to_string()).collect()
}

/// `jelly feed` writes the feeds of the daemon it talks to, not of the CLI's
/// own home, and a feed follows its jelly when the jelly or its project moves.
#[test]
fn a_feed_lives_in_the_daemons_home_and_follows_its_jelly() {
    let (daemon_home, cli_home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["serve", "--port", "0", "--home"]).arg(daemon_home.path())
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let url = common::served_url(&mut child);
    let _daemon = Daemon(child);
    ureq::post(&format!("{url}/api/jellies")).send_json(json!({"name": "prs", "project": "p", "view": "",
        "verbs": "const verbs = { refresh(s, a) { s.got = a; } };", "state": {}, "by": "agent"})).unwrap();
    let jelly = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).env("JELLY_HOME", cli_home.path()).env_remove("JELLY_AS")
            .args(["--url", &url, "--as", "agent"]).args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };

    jelly(&["--project", "p", "feed", "prs", "refresh", "--every", "5", "--", "echo", r#"{"x": 1}"#]);
    assert_eq!(fed(daemon_home.path()), ["p:prs"]);
    assert!(!cli_home.path().join("feeds.json").exists());
    let deadline = Instant::now() + Duration::from_secs(10);
    while jelly(&["state", "p:prs"]).trim() == "{}" {
        assert!(Instant::now() < deadline, "the daemon never ran the feed");
        std::thread::sleep(Duration::from_millis(100));
    }

    assert!(jelly(&["--project", "p", "move", "prs", "q"]).contains("its feed moved with it"));
    assert_eq!(fed(daemon_home.path()), ["q:prs"]);
    jelly(&["project", "q", "--move", "r"]);
    assert_eq!(fed(daemon_home.path()), ["r:prs"]);
    jelly(&["--project", "r", "feed", "prs", "--rm"]);
    assert!(fed(daemon_home.path()).is_empty());

    // A removed jelly's feed goes with it, so a jelly moved in under that name
    // keeps its own feed and gets no other.
    for project in ["p", "q"] {
        ureq::post(&format!("{url}/api/jellies")).send_json(json!({"name": "prs", "project": project, "view": "",
            "verbs": "const verbs = { refresh(s, a) { s.got = a; } };", "state": {}, "by": "agent"})).unwrap();
        jelly(&["--project", project, "feed", "prs", "refresh", "--every", "5", "--", "echo", project]);
    }
    jelly(&["rm", "p:prs"]);
    assert_eq!(fed(daemon_home.path()), ["q:prs"]);
    // A feed left under the new name (its jelly removed on the jelly page) is dropped.
    let mut feeds: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(daemon_home.path().join("feeds.json")).unwrap()).unwrap();
    let mut left = feeds[0].clone();
    left["jelly"] = json!("p:prs");
    left["command"] = json!(["echo", "stale"]);
    feeds.push(left);
    std::fs::write(daemon_home.path().join("feeds.json"), serde_json::to_vec(&feeds).unwrap()).unwrap();
    jelly(&["move", "q:prs", "p"]);
    let text = std::fs::read_to_string(daemon_home.path().join("feeds.json")).unwrap();
    let feeds: Vec<Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(feeds.len(), 1, "{text}");
    assert_eq!((feeds[0]["jelly"].as_str(), feeds[0]["command"][1].as_str()), (Some("p:prs"), Some("q")));

    // A feeds file it cannot read stops a move before the jelly moves.
    std::fs::write(daemon_home.path().join("feeds.json"), "not json").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "--as", "agent", "move", "p:prs", "q"]).output().unwrap();
    assert!(!out.status.success());
    assert!(ureq::get(&format!("{url}/api/jellies/p:prs/state")).call().is_ok(), "the jelly did not move");
}
