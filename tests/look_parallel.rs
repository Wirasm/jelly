#![cfg(unix)]

mod common;

use serde_json::json;
use std::process::{Command, Stdio};

/// Several agents can each run `jelly look` on their own jelly at the same time.
/// Each render's headless Chrome must not fight the others over a profile.
#[test]
fn parallel_looks_all_render() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    for i in 0..4 {
        ureq::post(&format!("{url}/api/jellies"))
            .send_json(json!({"name": format!("t:p{i}"), "view": "<p id=\"x\">here</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
            .unwrap();
    }
    let looks: Vec<_> = (0..4)
        .map(|i| {
            let url = url.clone();
            std::thread::spawn(move || {
                let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "look", &format!("t:p{i}"), "--width", "1280"]).output().unwrap();
                String::from_utf8_lossy(&out.stdout).into_owned()
            })
        })
        .collect();
    let outs: Vec<String> = looks.into_iter().map(|t| t.join().unwrap()).collect();
    let _ = daemon.kill();
    for out in outs {
        assert!(out.contains(r#"text "here" #x"#) && out.contains("png      "), "{out}");
    }
}
