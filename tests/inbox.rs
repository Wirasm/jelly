use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
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

    fn person_event(&self) -> Value {
        self.post("/api/jellies", json!({"name": "sample", "project": "p", "view": "<p>sample</p>", "verbs": "const verbs = {click(s,a){s.clicked=true;}};", "state": {}, "by": "agent"}));
        self.post("/api/jellies/p:sample/call", json!({"verb": "click", "args": {}, "by": "person"}))
    }

    fn command(&self, client: &Path) -> Command {
        let mut cmd = Command::new(BIN);
        configure(&mut cmd, &self.url, client);
        cmd
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn configure(cmd: &mut Command, url: &str, client: &Path) {
    cmd.env_remove("JELLY_AS").env_remove("JELLY_SESSION_ID").env_remove("CLAUDE_CODE_SESSION_ID")
        .env("JELLY_URL", url).env("JELLY_PROJECT", "p").env("JELLY_HOME", client)
        .env("HOME", client).stdin(Stdio::null());
}

fn stdout(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

fn has_event(text: &str) { assert!(text.contains("p:sample: click {}"), "{text}"); }
fn empty(text: &str) { assert_eq!(text.trim(), "Nothing new from the person."); }

#[test]
fn inbox_readers_are_independent_and_daemon_owned() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("daemon");
    let client = temp.path().join("client");
    std::fs::create_dir_all(client.join("inbox")).unwrap();
    std::fs::write(client.join("inbox/p"), "9999999999999").unwrap();
    let daemon = Daemon::start(&home);
    let event = daemon.person_event();
    let legacy = || -> Vec<Value> { ureq::get(&format!("{}/api/inbox?project=p&since=0", daemon.url)).call().unwrap().into_json().unwrap() };
    assert_eq!(legacy()[0]["seq"], event["seq"]);
    assert!(!home.join("inbox-readers.json").exists());
    let read = |name: &str| stdout(daemon.command(&client).env("JELLY_AS", name).arg("inbox").output().unwrap());
    for name in ["reader-a", "reader-b"] { has_event(&read(name)); empty(&read(name)); }
    assert_eq!(legacy().len(), 1);
    let filtered: Vec<Value> = ureq::get(&format!("{}/api/inbox?project=p&since={}", daemon.url, event["at_ms"])).call().unwrap().into_json().unwrap();
    assert!(filtered.is_empty());
    daemon.post("/api/jellies/p:sample/call", json!({"verb": "click", "args": {}, "by": "person"}));
    for name in ["reader-a", "reader-b"] { has_event(&read(name)); empty(&read(name)); }
    assert_eq!(std::fs::read_to_string(client.join("inbox/p")).unwrap(), "9999999999999");
    assert!(!client.join("inbox-readers.json").exists());
    assert!(home.join("inbox-readers.json").exists());
    stdout(daemon.command(&client).args(["call", "p:sample", "click"]).output().unwrap());
    let last_event = || -> Value {
        serde_json::from_str(std::fs::read_to_string(home.join("events.jsonl")).unwrap().lines().last().unwrap()).unwrap()
    };
    assert_eq!(last_event()["by"], "agent");
    stdout(daemon.command(&client).args(["--as", "writer", "call", "p:sample", "click"]).output().unwrap());
    assert_eq!(last_event()["by"], "writer");
    drop(daemon);
    let daemon = Daemon::start(&home);
    for name in ["reader-a", "reader-b"] {
        empty(&stdout(daemon.command(&client).env("JELLY_AS", name).arg("inbox").output().unwrap()));
    }
}

#[test]
fn inbox_conversations_resume_across_commands_and_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(&temp.path().join("daemon"));
    daemon.person_event();
    let client = temp.path().join("client");
    has_event(&stdout(daemon.command(&client).env("CLAUDE_CODE_SESSION_ID", "conversation-a").arg("inbox").output().unwrap()));
    let hook = |id: &str| {
        let mut child = daemon.command(&client).args(["inbox", "--hook"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        write!(child.stdin.take().unwrap(), "{}", json!({"session_id": id, "prompt": "ignored"})).unwrap();
        stdout(child.wait_with_output().unwrap())
    };
    assert_eq!(hook("conversation-a"), "");
    has_event(&hook("conversation-b"));
    empty(&stdout(daemon.command(&client).env("JELLY_SESSION_ID", "conversation-a").arg("inbox").output().unwrap()));
    has_event(&stdout(daemon.command(&client).env("JELLY_AS", "agent").env("JELLY_SESSION_ID", "conversation-a").arg("inbox").output().unwrap()));
    has_event(&stdout(daemon.command(&client).env("JELLY_AS", "agent").args(["--as", "override", "inbox"]).output().unwrap()));
    has_event(&stdout(daemon.command(&client).env("JELLY_SESSION_ID", "conversation-a").args(["inbox", "--session-id", "conversation-c"]).output().unwrap()));
    assert_eq!(stdout(daemon.command(&client).env("CLAUDE_CODE_SESSION_ID", "conversation-a").args(["inbox", "--hook"]).output().unwrap()), "");
    assert_eq!(stdout(daemon.command(&client).env("JELLY_PROJECT", "").args(["inbox", "--hook"]).output().unwrap()), "");

    let mut ordinary = daemon.command(&client).arg("inbox").stdin(Stdio::piped())
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while ordinary.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = ordinary.kill();
            let _ = ordinary.wait();
            panic!("ordinary inbox blocked on open stdin");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    has_event(&stdout(ordinary.wait_with_output().unwrap()));

    let request = json!({"project": "p", "reader": {"kind": "conversation", "id": "concurrent"}});
    let url = format!("{}/api/inbox", daemon.url);
    let responses = std::thread::scope(|scope| {
        let reads: Vec<_> = (0..2).map(|_| scope.spawn(|| -> Value {
            ureq::post(&url).send_json(request.clone()).unwrap().into_json().unwrap()
        })).collect();
        reads.into_iter().map(|t| t.join().unwrap()["events"].as_array().unwrap().len()).sum::<usize>()
    });
    assert_eq!(responses, 2);
}

#[cfg(unix)]
#[test]
fn inbox_parent_lifetime_and_fresh_shell_conversations() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("daemon");
    let daemon = Daemon::start(&home);
    daemon.person_event();
    let client = temp.path().join("client");
    let shell = |session: Option<&str>| {
        let mut cmd = Command::new("sh");
        configure(&mut cmd, &daemon.url, &client);
        cmd.env("JELLY_TEST_BIN", BIN).args(["-c", "\"$JELLY_TEST_BIN\" inbox; \"$JELLY_TEST_BIN\" inbox; :"]);
        if let Some(session) = session { cmd.env("JELLY_SESSION_ID", session); }
        stdout(cmd.output().unwrap())
    };
    for _ in 0..2 {
        let result = shell(None);
        has_event(&result);
        assert_eq!(result.matches("p:sample: click {}").count(), 1);
        assert!(result.contains("Nothing new from the person."));
    }
    has_event(&shell(Some("resumed")));
    assert_eq!(shell(Some("resumed")).matches("Nothing new from the person.").count(), 2);
    let records: Vec<Value> = serde_json::from_slice(&std::fs::read(home.join("inbox-readers.json")).unwrap()).unwrap();
    let parents: Vec<_> = records.iter().filter(|c| c["reader"]["kind"] == "parent").collect();
    assert_eq!(parents.len(), 2);
    assert!(parents.iter().all(|c| c["reader"]["start_time"].as_u64().unwrap() > 0));
}

#[test]
fn inbox_daemon_failures_are_visible_in_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("daemon");
    let daemon = Daemon::start(&home);
    daemon.person_event();
    std::fs::write(home.join("inbox-readers.json"), "broken").unwrap();
    let client = temp.path().join("client");
    let result = daemon.command(&client).args(["--as", "a", "inbox"]).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("could not read inbox"));
    assert!(stdout(daemon.command(&client).args(["--as", "a", "inbox", "--hook"]).output().unwrap()).contains("jelly inbox failed: could not read inbox"));
    std::fs::remove_file(home.join("inbox-readers.json")).unwrap();
    let log = std::fs::read_to_string(home.join("events.jsonl")).unwrap();
    let event: Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    let mut missing_field = event.clone();
    missing_field.as_object_mut().unwrap().remove("seq");
    let mut unknown_kind = event.clone();
    unknown_kind["kind"] = json!("unknown");
    let mut old = missing_field.clone();
    old["at_ms"] = json!(0);
    let mut excluded = missing_field.clone();
    excluded["by"] = json!("agent");
    let mut other_project = missing_field.clone();
    other_project["jelly"] = json!("other:sample");
    for malformed in ["malformed".to_string(), missing_field.to_string(), unknown_kind.to_string(),
        String::new(), old.to_string(), excluded.to_string(), other_project.to_string()]
    {
        std::fs::write(home.join("events.jsonl"), format!("{malformed}\n{log}")).unwrap();
        let result = daemon.command(&client).args(["--as", "a", "inbox"]).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("could not read inbox"));
        for _ in 0..2 {
            let warning = stdout(daemon.command(&client).args(["--as", "a", "inbox", "--hook"]).output().unwrap());
            assert!(warning.contains("jelly inbox failed: could not read inbox"), "{warning}");
        }
        assert!(!home.join("inbox-readers.json").exists());
    }
    std::fs::write(home.join("events.jsonl"), log).unwrap();
    has_event(&stdout(daemon.command(&client).args(["--as", "a", "inbox"]).output().unwrap()));
    assert_eq!(stdout(daemon.command(&client).args(["--as", "a", "inbox", "--hook"]).output().unwrap()), "");
    let url = daemon.url.clone();
    drop(daemon);
    let mut cmd = Command::new(BIN);
    configure(&mut cmd, &url, &client);
    assert!(!cmd.args(["--as", "a", "inbox"]).output().unwrap().status.success());
    let mut cmd = Command::new(BIN);
    configure(&mut cmd, &url, &client);
    assert_eq!(stdout(cmd.args(["--as", "a", "inbox", "--hook"]).output().unwrap()), "");
}

#[test]
fn inbox_uncommitted_reads_replay_and_commits_preserve_new_events() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(temp.path());
    let event = daemon.person_event();
    let request = json!({"project": "p", "reader": {"kind": "named", "name": "retry"}});
    let first = daemon.post("/api/inbox", request.clone());
    assert_eq!(first["events"][0]["seq"], event["seq"]);
    assert_eq!(daemon.post("/api/inbox", request.clone()), first);
    let cursors: Value = serde_json::from_slice(&std::fs::read(temp.path().join("inbox-readers.json")).unwrap()).unwrap();
    assert!(cursors[0].get("seq").is_none());
    let later = daemon.post("/api/jellies/p:sample/call", json!({"verb": "click", "args": {}, "by": "person"}));
    let mut commit = request.clone();
    commit["through"] = first["through"].clone();
    daemon.post("/api/inbox/commit", commit.clone());
    let next = daemon.post("/api/inbox", request.clone());
    assert_eq!(next["events"].as_array().unwrap().len(), 1);
    assert_eq!(next["events"][0]["seq"], later["seq"]);
    let mut newer = commit.clone();
    newer["through"] = next["through"].clone();
    daemon.post("/api/inbox/commit", newer);
    daemon.post("/api/inbox/commit", commit);
    assert!(daemon.post("/api/inbox", request)["events"].as_array().unwrap().is_empty());
}

#[test]
fn inbox_failed_progress_commit_reports_failure_and_replays() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(temp.path());
    daemon.person_event();
    let read = |hook: bool| {
        let mut cmd = daemon.command(temp.path());
        cmd.args(["--as", "retry", "inbox"]);
        if hook { cmd.arg("--hook"); }
        cmd.output().unwrap()
    };
    has_event(&stdout(read(false)));
    daemon.post("/api/jellies/p:sample/call", json!({"verb": "click", "args": {}, "by": "person"}));
    let saved = std::fs::read(temp.path().join("inbox-readers.json")).unwrap();
    let tmp = temp.path().join("inbox-readers.json.tmp");
    std::fs::create_dir(&tmp).unwrap();
    let failed = read(false);
    assert!(!failed.status.success());
    has_event(std::str::from_utf8(&failed.stdout).unwrap());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("could not commit inbox progress"));
    let hook = stdout(read(true));
    has_event(&hook);
    assert!(hook.contains("jelly inbox failed: could not commit inbox progress"));
    assert_eq!(std::fs::read(temp.path().join("inbox-readers.json")).unwrap(), saved);
    std::fs::remove_dir(tmp).unwrap();
    has_event(&stdout(read(false)));
    empty(&stdout(read(false)));
}
