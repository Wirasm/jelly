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

/// `look --problems` prints only what the agent must fix.
#[test]
fn look_problems_lists_only_errors() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    let view = r#"<p id="x">fine</p><script>throw new Error("boom in view")</script>"#;
    ureq::post(&format!("{}/api/jellies", d.1))
        .send_json(json!({"name": "t:p", "view": view, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1, "look", "t:p", "--text", "--problems"]).output().unwrap();
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

/// An asset added by mistake can be removed, and `look --poster` turns the
/// screenshot into the jelly page's thumbnail in one step.
#[test]
fn assets_can_be_removed_and_posters_come_from_look() {
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
    jelly(&["look", "t:a", "--poster"]);
    assert_eq!(assets(), ["poster.png"]);
    let poster = ureq::get(&format!("{}/j/t:a/assets/poster.png", d.1)).call().unwrap();
    let mut png = Vec::new();
    std::io::Read::read_to_end(&mut poster.into_reader(), &mut png).unwrap();
    assert!(png.starts_with(b"\x89PNG"));
}

/// When the person names a jelly loosely ("the one about the three issues"),
/// `jelly find` resolves it across every project, archived ones included, and
/// prints the same short reference the person can copy from the jelly page.
#[test]
fn find_resolves_a_loosely_named_jelly() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    for (name, about) in [("a:three", "Shaping the three issues that need you"), ("b:other", "Something else"), ("b:old", "Three old issues")] {
        ureq::post(&format!("{}/api/jellies", d.1))
            .send_json(json!({"name": name, "about": about, "view": "<p>x</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
            .unwrap();
    }
    ureq::post(&format!("{}/api/jellies/b:old/archive", d.1)).send_json(json!({"by": "test"})).unwrap();
    let find = |words: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1, "find"]).args(words).output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let (ok, text) = find(&["three", "issues"]);
    assert!(ok, "{text}");
    let first = text.lines().next().unwrap_or("");
    assert_eq!(first, "a:three · Shaping the three issues that need you", "live jellies come first: {text}");
    assert!(text.contains("made by test") && text.contains("b:old · Three old issues") && text.contains("archived"), "{text}");
    assert!(!text.contains("b:other"), "{text}");
    assert!(!find(&["nothing", "like", "this"]).0);
}

/// A jelly made outside a git repo lands in no project, where `jelly hear` run
/// from the person's repo never looks. `new` says so, unless that was asked for.
#[test]
fn new_outside_a_repo_says_the_jelly_is_in_no_project() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    std::fs::write(cwd.path().join("view.html"), "<p>hi</p>").unwrap();
    std::fs::write(cwd.path().join("verbs.js"), "const verbs = {};").unwrap();
    let new = |name: &str, more: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["--url", &d.1]).args(more).args(["make", name, "--from", "."])
            .current_dir(cwd.path())
            // A temp dir inside a checkout must not count as that repo.
            .env("GIT_CEILING_DIRECTORIES", cwd.path().parent().unwrap())
            .env_remove("JELLY_PROJECT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    assert!(new("tea", &[]).contains("tea is in no project"));
    assert_eq!(new("chai", &["--project", ""]), "");
    assert_eq!(new("mate", &["--project", "kitchen"]), "");
}
