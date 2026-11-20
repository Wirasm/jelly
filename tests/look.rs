//! `jelly look`: desktop and phone in one call.
#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};

struct Daemon(Child, String);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A sidebar on desktop becomes a menu button on a phone, and two tiny buttons
/// are fine with a mouse but too small to tap.
const VIEW: &str = r##"<style>
    .row { display: flex; gap: 16px; } .menu { display: none; }
    @media (max-width: 600px) { .side { display: none; } .menu { display: block; } }
    .tiny { width: 16px; height: 16px; padding: 0; }
    </style>
    <h1 id="title">Plan</h1><button class="menu" id="menu">Menu</button>
    <div class="row"><aside class="side" id="side"><a href="#a">Overview</a></aside>
    <main><p id="count">3 items</p><div style="display: flex"><button class="tiny" id="a">a</button><button class="tiny" id="b">b</button></div></main></div>"##;

fn daemon(home: &std::path::Path) -> Daemon {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut child);
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:l", "view": VIEW, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    Daemon(child, url)
}

fn look(d: &Daemon, json: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jelly"));
    cmd.args(["--url", &d.1, "look", "t:l"]);
    if json { cmd.arg("--json"); }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The width of a PNG, from its header.
fn png_width(path: &str) -> u32 {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[1..4], b"PNG", "{path} is not a PNG");
    u32::from_be_bytes(bytes[16..20].try_into().unwrap())
}

#[test]
fn look_shows_desktop_and_only_what_differs_on_a_phone() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    let out = look(&d, false);
    let (top, phone) = out.split_once("\nat phone width 390×844, compared with desktop:\n").unwrap_or_else(|| panic!("no phone part:\n{out}"));
    // The desktop outline, as `look --text` prints it.
    assert!(top.contains("on screen at 1280×900 (v1):\n- heading \"Plan\" [h1] #title\n- aside: #side\n  - link \"Overview\""), "{out}");
    // The phone part is the difference, not a second outline.
    assert_eq!(phone, "removed  aside: #side\nremoved  link \"Overview\"\nadded    button \"Menu\" #menu\n", "{out}");
    // A problem only a phone has is said once, as a phone problem.
    assert!(top.contains("!        at phone width: too small to tap (under 24px): button \"a\" #a 16×16"), "{out}");
    assert!(!top.contains("!        too small"), "{out}");
    // One image: desktop, then phone, side by side with a 16px margin around each.
    let png = top.lines().find_map(|l| l.strip_prefix("png      ")).and_then(|l| l.split(" · ").next()).unwrap_or_else(|| panic!("no png:\n{out}"));
    assert_eq!(png_width(png), 16 + 1280 + 16 + 390 + 16);
}

#[test]
fn look_json_has_the_phone_difference() {
    let home = tempfile::tempdir().unwrap();
    let d = daemon(home.path());
    let v: Value = serde_json::from_str(&look(&d, true)).unwrap();
    assert_eq!(v["jelly"], "t:l");
    assert!(v["outline"].as_str().unwrap().contains("link \"Overview\""), "{v}");
    assert_eq!(v["phone"], json!({"width": 390, "height": 844, "added": ["button \"Menu\" #menu"],
        "removed": ["aside: #side", "link \"Overview\""], "changed": []}));
    assert!(v["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("at phone width: too small to tap")), "{v}");
    assert!(std::path::Path::new(v["png"].as_str().unwrap()).exists(), "{v}");
}
