#![cfg(unix)]

mod common;

use serde_json::json;
use std::process::{Command, Stdio};

/// `jelly see` turns the page into an outline. Words around inline elements
/// must stay with them, and controls inside text must still read as controls.
#[test]
fn see_keeps_labels_and_controls_inside_text() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    let view = r#"<div class="stats"><span>p50 <b>6.5</b> ms</span></div>
        <p>When ready, <button id="go">Go</button></p>
        <section id="s">Intro <h2>Part</h2><p>Body</p></section>"#;
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:o", "view": view, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "see", "t:o"]).output().unwrap();
    let _ = daemon.kill();
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [r#"- text "p50 6.5 ms""#, r#"- text "When ready,""#, r#"  - button "Go" #go"#, r#"- text "Intro" #s"#, r#"  - heading "Part" [h2]"#] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}
