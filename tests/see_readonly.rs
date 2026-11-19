#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::process::{Command, Stdio};

/// `jelly see` renders pages in headless Chrome where nobody is looking. A view
/// that calls a verb on load, and the jelly page marking tiles as seen, must not
/// record anything as the person there.
#[test]
fn see_never_changes_anything_as_the_person() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    let view = r#"<p id="n">loading</p><script>
        jelly.onState((s) => { document.getElementById("n").textContent = "opens " + s.opens; });
        jelly.call("open").catch(() => {});
    </script>"#;
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:x", "view": view, "verbs": "const verbs = { open(s) { s.opens++; } };", "state": {"opens": 0}, "by": "test"}))
        .unwrap();
    let see = |name: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "see", name]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let x = see("t:x");
    // Waits long enough for the jelly page's 1.5 s seen timer.
    let surface = see("surface");
    let state: Value = ureq::get(&format!("{url}/api/jellies/t:x/state")).call().unwrap().into_json().unwrap();
    let page: Value = ureq::get(&format!("{url}/api/jellies/surface/state")).call().unwrap().into_json().unwrap();
    let _ = daemon.kill();
    assert!(x.contains(r#"text "opens 0" #n"#), "{x}");
    assert_eq!(state["opens"], 0, "{x}");
    assert!(page["seen"].as_object().is_none_or(|seen| seen.is_empty()), "{page}\n{surface}");
}
