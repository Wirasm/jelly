#![cfg(unix)]

mod common;

use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

/// `jelly hear --wake` returns at once, and the person's Send makes the daemon
/// queue one message to the Codex session that asked. A stub stands in for codex.
#[test]
fn a_send_queues_a_message_to_the_codex_session_that_asked() {
    let home = tempfile::tempdir().unwrap();
    let said = home.path().join("said");
    let codex = home.path().join("codex-stub");
    std::fs::write(&codex, format!("#!/bin/sh\necho \"$@\" >> {}\n", said.display())).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"]).arg(home.path())
        .env("JELLY_CODEX", &codex)
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let url = common::served_url(&mut daemon);
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:plan", "view": "<p>plan</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"})).unwrap();
    let jelly = |args: &[&str], thread: Option<&str>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_jelly"));
        cmd.args(["--url", &url, "--project", "t"]).args(args).env_remove("CODEX_THREAD_ID");
        if let Some(t) = thread { cmd.env("CODEX_THREAD_ID", t); }
        cmd.output().unwrap()
    };

    let refused = jelly(&["hear", "--wake", "plan"], None);
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("jelly hear --wait"), "{refused:?}");

    let asked = jelly(&["hear", "--wake", "plan"], Some("0199-abc"));
    assert!(asked.status.success() && String::from_utf8_lossy(&asked.stdout).contains("presses Send in t:plan"), "{asked:?}");

    let send = |by: &str| { ureq::post(&format!("{url}/api/jellies/t:plan/send")).send_json(json!({"by": by})).unwrap(); };
    send("agent"); // an agent's own send wakes nobody
    send("person");
    send("person"); // the wake was used up by the first
    let mut lines = String::new();
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        lines = std::fs::read_to_string(&said).unwrap_or_default();
        if !lines.is_empty() { break; }
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    lines = std::fs::read_to_string(&said).unwrap_or(lines);
    let _ = daemon.kill();
    assert_eq!(lines.lines().count(), 1, "{lines}");
    assert!(lines.starts_with("queue --thread 0199-abc --message The person pressed Send on t:plan"), "{lines}");
}
