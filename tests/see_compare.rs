//! `jelly see --compare`: what a change did.
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

impl Daemon {
    fn start(home: &std::path::Path) -> Daemon {
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

    fn jelly(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &self.1]).args(args).output().unwrap();
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn post(&self, path: &str, body: Value) {
        ureq::post(&format!("{}{path}", self.1)).send_json(body).unwrap();
    }
}

const VERBS: &str = "const verbs = { add(s) { s.n += 1; } };";

/// v1 renders the count from state; a verb call (v2) and a reshape (v3) change
/// it. Comparing with v1 rebuilds that version from history.
#[test]
fn compare_names_what_a_change_did() {
    let home = tempfile::tempdir().unwrap();
    let d = Daemon::start(home.path());
    let view = |extra: &str| format!(r#"<h1 id="title">Plan</h1>{extra}<p id="count"></p>
        <script>jelly.onState((s) => document.getElementById("count").textContent = s.n + " items");</script>"#);
    d.post("/api/jellies", json!({"name": "t:c", "view": view(""), "verbs": VERBS, "state": {"n": 3}, "by": "test"}));
    let (ok, out) = d.jelly(&["see", "t:c", "--save", "first"]);
    assert!(ok, "{out}");
    d.post("/api/jellies/t:c/call", json!({"verb": "add", "args": {}, "by": "test"}));
    d.post("/api/jellies/t:c/reshape", json!({"view": view(r#"<p id="faint" style="color: #ddd">Faint</p>"#), "by": "test"}));

    let (ok, out) = d.jelly(&["see", "t:c", "--compare", "v1"]);
    assert!(ok, "{out}");
    let (top, diff) = out.split_once("\ncompared with v1:\n").unwrap_or_else(|| panic!("no comparison:\n{out}"));
    assert_eq!(diff, "changed  text \"3 items\" #count → text \"4 items\" #count\nadded    text \"Faint\" #faint\n", "{out}");
    assert!(top.contains("!        new: unreadable text: #dddddd on #ffffff"), "{out}");
    let png = top.lines().find_map(|l| l.strip_prefix("png      ")).and_then(|l| l.split(" · ").next()).unwrap_or_else(|| panic!("no png:\n{out}"));
    let bytes = std::fs::read(png).unwrap();
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 16 + 1280 + 16 + 1280 + 16, "before and after side by side");

    // The other way round, from a saved version: the faint text is fixed.
    let (ok, out) = d.jelly(&["see", "t:c", "--save", "faint"]);
    assert!(ok, "{out}");
    d.post("/api/jellies/t:c/reshape", json!({"view": view(""), "by": "test"}));
    let (ok, out) = d.jelly(&["see", "t:c", "--compare", "faint", "--json"]);
    assert!(ok, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!((v["against"].as_str(), v["against_version"].as_u64()), (Some("faint (v3)"), Some(3)), "{v}");
    assert_eq!((&v["added"], &v["removed"], &v["changed"]), (&json!([]), &json!(["text \"Faint\" #faint"]), &json!([])), "{v}");
    assert!(v["fixed_problems"][0].as_str().unwrap().contains("unreadable text: #dddddd"), "{v}");
    assert_eq!(v["new_problems"], json!([]));

    let (ok, out) = d.jelly(&["see", "t:c", "--compare", "first"]);
    assert!(ok && out.contains("\ncompared with first (v1):\nchanged  text \"3 items\" #count → text \"4 items\" #count\n"), "{out}");
}

/// After an undo, versions from before it are ambiguous, so they are refused.
#[test]
fn compare_refuses_versions_it_cannot_rebuild() {
    let home = tempfile::tempdir().unwrap();
    let d = Daemon::start(home.path());
    d.post("/api/jellies", json!({"name": "t:u", "view": "<p>x</p>", "verbs": VERBS, "state": {"n": 0}, "by": "test"}));
    d.post("/api/jellies/t:u/call", json!({"verb": "add", "args": {}, "by": "test"}));
    let (ok, out) = d.jelly(&["see", "t:u", "--compare", "v9"]);
    assert!(!ok && out.contains("t:u is at v2; there is no v9"), "{out}");
    let (ok, _) = d.jelly(&["undo", "t:u"]);
    assert!(ok);
    let (ok, out) = d.jelly(&["see", "t:u", "--compare", "v1"]);
    assert!(!ok && out.contains("older than its kept history, which starts at v3"), "{out}");
    let (ok, out) = d.jelly(&["see", "t:u", "--compare", "never"]);
    assert!(!ok && out.contains("nothing is saved as never"), "{out}");
}
