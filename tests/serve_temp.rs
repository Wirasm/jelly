#![cfg(unix)]

mod common;

use serde_json::json;
use std::process::{Command, Stdio};

/// `jelly serve --temp` is a throwaway daemon for agents and tests: a free port,
/// a fresh home in the temp dir, and nothing left behind after a clean stop.
#[test]
fn temp_daemons_are_separate_and_clean_up() {
    let tmp = tempfile::tempdir().unwrap();
    let start = || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["serve", "--temp"])
            .env("TMPDIR", tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let url = common::served_url(&mut child);
        (child, url)
    };
    let (mut a, url_a) = start();
    let (mut b, url_b) = start();
    assert_ne!(url_a, url_b);
    for url in [&url_a, &url_b] {
        ureq::post(&format!("{url}/api/jellies"))
            .send_json(json!({"name": "t:x", "view": "<p>x</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
            .unwrap();
    }
    let homes = || std::fs::read_dir(tmp.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("jelly-temp-")).count();
    assert_eq!(homes(), 2, "each daemon has its own home");
    for child in [&mut a, &mut b] {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(homes(), 0, "a clean stop removes the temporary home");
}
