//! What agents read: commands print one line per item, and --json the full data.

use serde_json::{json, Value};
use std::io::Read;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_jelly");

struct Daemon {
    child: Child,
    url: String,
}

impl Daemon {
    fn start(home: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let child = Command::new(BIN).args(["serve", "--port", &port.to_string(), "--home"])
            .arg(home).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let mut daemon = Self { child, url: format!("http://127.0.0.1:{port}") };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if ureq::get(&format!("{}/api/jellies", daemon.url)).call().is_ok() { break; }
            if let Some(status) = daemon.child.try_wait().unwrap() {
                let mut errors = String::new();
                daemon.child.stderr.take().unwrap().read_to_string(&mut errors).unwrap();
                panic!("daemon exited ({status}): {errors}");
            }
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }

    fn post(&self, path: &str, body: Value) -> Value {
        ureq::post(&format!("{}{path}", self.url)).send_json(body).unwrap().into_json().unwrap()
    }

    fn command(&self, client: &Path, project: &str, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command.args(args)
            .env_remove("JELLY_AS").env_remove("JELLY_SESSION_ID").env_remove("CLAUDE_CODE_SESSION_ID")
            .env("JELLY_URL", &self.url).env("JELLY_PROJECT", project).env("JELLY_HOME", client)
            .env("HOME", client).stdin(Stdio::null());
        command
    }

    /// Runs `jelly ARGS` in project `project` and returns its stdout.
    fn jelly(&self, client: &Path, project: &str, args: &[&str]) -> String {
        stdout(self.command(client, project, args).output().unwrap())
    }

    /// Runs `jelly wait ARGS` while the person clicks on `p:w`, and returns its stdout.
    /// It clicks until `wait` exits, because `wait` subscribes some time after it starts.
    fn wait(&self, client: &Path, args: &[&str]) -> String {
        let mut wait = self.command(client, "p", &[&["wait"], args].concat())
            .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while wait.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "jelly wait did not return");
            self.post("/api/jellies/p:w/call", json!({"verb": "click", "args": {}, "by": "person"}));
            std::thread::sleep(Duration::from_millis(50));
        }
        stdout(wait.wait_with_output().unwrap())
    }
}

fn stdout(output: std::process::Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn agent_commands_print_one_line_per_item_and_json_on_request() {
    let temp = tempfile::tempdir().unwrap();
    let client = temp.path().join("client");
    std::fs::create_dir_all(&client).unwrap();
    let daemon = Daemon::start(&temp.path().join("daemon"));
    let jelly = |args: &[&str]| daemon.jelly(&client, "p", args);
    let parse = |text: String| -> Value { serde_json::from_str(&text).unwrap() };

    daemon.post("/api/jellies", json!({"name": "x", "project": "p", "view": "<ul id=list></ul>", "verbs": "const verbs = {};", "state": {}, "by": "agent"}));
    daemon.post("/api/jellies/p:x/attention", json!({"kind": "ask", "title": "Which order?", "options": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}], "recommend": "a", "by": "agent"}));
    daemon.post("/api/jellies/p:x/comment", json!({"anchor": {"selector": "#list"}, "text": "bigger", "by": "person"}));

    assert_eq!(jelly(&["attention"]), "p:x  a1 ask \"Which order?\" [a★/b]\n");
    let full = parse(jelly(&["attention", "--json"]));
    assert_eq!(full.as_array().unwrap().len(), 1);
    assert_eq!(full[0]["jelly"], "p:x");
    assert_eq!(daemon.jelly(&client, "q", &["attention"]), "Nothing waits on the person in project q (--all for every project).\n");

    assert_eq!(jelly(&["comments"]), "p:x  c1 on #list: \"bigger\"\n");
    assert_eq!(jelly(&["comments", "x"]), "p:x  c1 on #list: \"bigger\"\n");
    let full = parse(jelly(&["comments", "--json"]));
    assert_eq!(full.as_array().unwrap().len(), 1);
    assert_eq!(full[0]["text"], "bigger");

    let spec = temp.path().join("spec.json");
    std::fs::write(&spec, json!({"title": "Sharing", "questions": [
        {"id": "how", "question": "How?", "options": [{"id": "y", "label": "Yes"}]},
        {"id": "when", "question": "When?", "options": [{"id": "n", "label": "Now"}]},
    ]}).to_string()).unwrap();
    let spec = spec.to_str().unwrap();
    let asked = jelly(&["ask", "s", "--spec", spec]);
    let since = asked.split("--since ").nth(1).unwrap().trim_end_matches("`\n");
    assert!(since.strip_prefix("seq:").unwrap().parse::<u64>().is_ok());
    assert_eq!(asked, format!("p:s round 1: asked 2 questions. Answers arrive in `jelly inbox`, or block on `jelly wait p:s --since {since}`\n"));
    let full = parse(jelly(&["ask", "s", "--spec", spec, "--json"]));
    assert_eq!((full["jelly"].as_str(), full["round"].as_u64(), full["questions"].as_u64()), (Some("p:s"), Some(2), Some(2)));

    daemon.post("/api/jellies", json!({"name": "w", "project": "p", "view": "<p>w</p>", "verbs": "const verbs = {click(s){s.clicks=(s.clicks||0)+1;}};", "state": {}, "by": "agent"}));
    let line = daemon.wait(&client, &["w"]);
    assert!(line.starts_with("p:w v") && line.ends_with(": person click {}\n") && line.lines().count() == 1, "{line}");
    let full = parse(daemon.wait(&client, &["w", "--json"]));
    assert_eq!((full["jelly"].as_str(), full["verb"].as_str()), (Some("p:w"), Some("click")));
    assert!(full["state"]["clicks"].as_u64().unwrap() > 0, "{full}");
}

#[test]
fn item_commands_finish_quietly_with_closed_stdout() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(temp.path());
    daemon.post("/api/jellies", json!({"name":"p:x", "view":"<p>x</p>", "verbs":"const verbs = {};", "by":"agent"}));
    let spec = temp.path().join("spec.json");
    std::fs::write(&spec, json!({"question":"Which?", "options":[{"id":"a", "label":"A"}]}).to_string()).unwrap();
    for populated in [false, true] {
        if populated {
            daemon.post("/api/jellies/p:x/attention", json!({"kind":"ask", "title":"Which?", "options":[{"id":"a", "label":"A"}], "by":"agent"}));
            daemon.post("/api/jellies/p:x/comment", json!({"text":"bigger", "by":"person"}));
        }
        for args in [vec!["attention"], vec!["comments", "p:x"], vec!["ask", "p:s", "--spec", spec.to_str().unwrap()]] {
            let (reader, writer) = std::io::pipe().unwrap();
            drop(reader);
            let out = daemon.command(temp.path(), "p", &args).stdout(Stdio::from(writer)).output().unwrap();
            assert!(out.status.success() && out.stderr.is_empty(), "{args:?}: {out:?}");
        }
    }
    let created: Value = ureq::get(&format!("{}/api/jellies/p:s", daemon.url)).call().unwrap().into_json().unwrap();
    assert_eq!(created["state"]["round"], 2);
}
