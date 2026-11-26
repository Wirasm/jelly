mod common;

use serde_json::{json, Value};
use std::process::{Child, Command, Output, Stdio};

struct Daemon(Child, String);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn jelly(url: &str, project: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jelly")).env_remove("JELLY_AS")
        .args(["--url", url, "--project", project, "--as", "agent"]).args(args).output().unwrap()
}

/// A short name means this project's jelly, or the jelly with no project when
/// there is no project, never a jelly of the same name in another project.
#[test]
fn a_short_name_never_reaches_another_project() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["serve", "--port", "0", "--home"]).arg(home.path())
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let url = common::served_url(&mut child);
    let daemon = Daemon(child, url);
    let url = daemon.1.as_str();
    let make = |project: &str| {
        ureq::post(&format!("{url}/api/jellies")).send_json(json!({"name": "counter", "project": project, "view": "",
            "verbs": "const verbs = { add(s) { s.n = (s.n || 0) + 1; } };", "state": {}, "by": "agent"})).unwrap();
    };
    make("audit");

    let refused = jelly(url, "other", &["do", "counter", "add"]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success() && said.contains("no jelly named other:counter; did you mean audit:counter?"), "{said}");
    let outside = jelly(url, "", &["do", "counter", "add"]);
    assert!(!outside.status.success(), "{outside:?}");

    make("");
    assert!(jelly(url, "", &["do", "counter", "add"]).status.success());
    let state = |name: &str| -> Value { ureq::get(&format!("{url}/api/jellies/{name}/state")).call().unwrap().into_json().unwrap() };
    assert_eq!(state("counter"), json!({"n": 1}));
    assert_eq!(state("audit:counter"), json!({}), "the other project's jelly was not touched");
}
