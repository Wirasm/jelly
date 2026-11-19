#![cfg(unix)]

mod common;

use serde_json::json;
use std::process::{Command, Stdio};

/// `jelly see` renders in headless Chrome. A 3D view must get a WebGL context
/// there, or the agent cannot check what it made.
#[test]
fn see_renders_webgl_views() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    let view = r#"<p id="gl"></p><script>
        const gl = document.createElement("canvas").getContext("webgl");
        document.getElementById("gl").textContent = gl ? "webgl yes" : "webgl no";
    </script>"#;
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:gl", "view": view, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "see", "t:gl"]).output().unwrap();
    let _ = daemon.kill();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(r#"text "webgl yes" #gl"#), "{text}{}", String::from_utf8_lossy(&out.stderr));
}
