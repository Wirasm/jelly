#![cfg(unix)]

mod common;

use serde_json::json;
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

/// `see --problems` prints only what the agent must fix.
#[test]
fn see_problems_lists_only_errors() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    let view = r#"<p id="x">fine</p><script>throw new Error("boom in view")</script>"#;
    ureq::post(&format!("{}/api/jellies", d.1))
        .send_json(json!({"name": "t:p", "view": view, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1, "see", "t:p", "--problems"]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().count() == 1 && text.starts_with("error ") && text.contains("boom in view"), "{text}");
}

/// `jelly share` without --out writes to the temp dir, never into the
/// directory the agent runs in, which is usually the person's repo.
#[test]
fn share_defaults_outside_the_working_directory() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    ureq::post(&format!("{}/api/jellies", d.1))
        .send_json(json!({"name": "t:s", "view": "<p>hi</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--url", &d.1, "share", "t:s"])
        .current_dir(cwd.path())
        .env("TMPDIR", tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(std::fs::read_dir(cwd.path()).unwrap().count(), 0);
    assert!(tmp.path().join("jelly-share/s.html").exists(), "{}", String::from_utf8_lossy(&out.stdout));
}

/// An asset added by mistake can be removed, and `see --poster` turns the
/// screenshot into the jelly page's thumbnail in one step.
#[test]
fn assets_can_be_removed_and_posters_come_from_see() {
    let home = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    ureq::post(&format!("{}/api/jellies", d.1))
        .send_json(json!({"name": "t:a", "view": "<p id=\"x\">hi</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let jelly = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1]).args(args).output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let assets = || -> Vec<String> { ureq::get(&format!("{}/api/jellies/t:a/assets", d.1)).call().unwrap().into_json().unwrap() };
    let wrong = files.path().join("wrong.png");
    std::fs::write(&wrong, b"not the right file").unwrap();
    jelly(&["asset", "t:a", wrong.to_str().unwrap()]);
    assert_eq!(assets(), ["wrong.png"]);
    assert_eq!(jelly(&["asset", "t:a", "--rm", "wrong.png"]), "removed assets/wrong.png\n");
    assert!(assets().is_empty());
    let missing = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1, "asset", "t:a", "--rm", "wrong.png"]).output().unwrap();
    assert!(!missing.status.success());
    jelly(&["see", "t:a", "--poster"]);
    assert_eq!(assets(), ["poster.png"]);
    let poster = ureq::get(&format!("{}/j/t:a/assets/poster.png", d.1)).call().unwrap();
    let mut png = Vec::new();
    std::io::Read::read_to_end(&mut poster.into_reader(), &mut png).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
}
