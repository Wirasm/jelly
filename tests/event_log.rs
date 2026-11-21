#[path = "../src/disk.rs"]
#[allow(dead_code)]
mod disk;
mod common;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(10);

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

enum Failure {
    Append,
    Trim,
}

fn event_log_failure(failure: Failure, closed_stderr: bool) {
    let home = tempfile::tempdir().unwrap();
    let warnings = home.path().join("stderr.txt");
    let stderr = if closed_stderr {
        Stdio::piped()
    } else {
        std::fs::File::create(&warnings).unwrap().into()
    };
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["serve", "--port", "0", "--home"])
            .arg(home.path())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    // A closed stderr is closed only after the banner: event-log failures must not
    // depend on the startup diagnostic.
    let url = if closed_stderr {
        common::served_url(&mut daemon.0)
    } else {
        common::served_url_in(&mut daemon.0, &warnings)
    };
    let agent = ureq::AgentBuilder::new().timeout(LIMIT).build();
    let post = |path: &str, body: Value| -> Value {
        agent
            .post(&format!("{url}{path}"))
            .send_json(body)
            .unwrap()
            .into_json()
            .unwrap()
    };
    let created = post(
        "/api/jellies",
        json!({
            "name": "counter", "project": "test", "view": "<p>counter</p>",
            "verbs": "const verbs = {increment(s) { s.count += 1; }};",
            "state": {"count": 0}, "by": "test"
        }),
    );
    let log = home.path().join("events.jsonl");
    let history = std::fs::read(&log).unwrap();
    match failure {
        Failure::Append => {
            std::fs::remove_file(&log).unwrap();
            std::fs::create_dir(&log).unwrap();
        }
        Failure::Trim => {
            let line = std::fs::read(&log).unwrap();
            let count = (8 << 20) / line.len() + 1;
            std::fs::write(&log, line.repeat(count)).unwrap();
            std::fs::create_dir(log.with_extension("json.tmp")).unwrap();
        }
    };
    let stream = agent
        .get(&format!("{url}/api/events?jelly=test:counter"))
        .call()
        .unwrap();
    let mut stream = BufReader::new(stream.into_reader());

    if matches!(failure, Failure::Append) {
        let error = agent
            .post(&format!("{url}/api/jellies/test:counter/do"))
            .send_json(json!({"verb": "increment", "args": {}, "by": "test"}))
            .unwrap_err();
        let ureq::Error::Status(status, response) = error else {
            panic!("expected a store error, got {error}");
        };
        assert_eq!(status, 400);
        let error: Value = response.into_json().unwrap();
        assert!(error["error"]
            .as_str()
            .unwrap()
            .contains("could not record event"));
        let saved: Value = serde_json::from_slice(
            &std::fs::read(home.path().join("jellies").join(format!("{}.json", disk::name("test:counter")))).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["jelly"]["state"], created["state"]);
        assert_eq!(saved["jelly"]["version"], created["version"]);
        let current: Value = agent
            .get(&format!("{url}/api/jellies/test:counter"))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(current["state"], created["state"]);
        assert_eq!(current["version"], created["version"]);

        std::fs::remove_dir(home.path().join("events.jsonl")).unwrap();
        std::fs::write(home.path().join("events.jsonl"), history).unwrap();
    }
    let event = post(
        "/api/jellies/test:counter/do",
        json!({"verb": "increment", "args": {}, "by": "test"}),
    );
    assert_eq!(event["state"], json!({"count": 1}));
    assert_eq!(event["verb"], "increment");
    loop {
        let mut line = String::new();
        assert!(
            stream.read_line(&mut line).unwrap() > 0,
            "event stream closed"
        );
        if let Some(data) = line.strip_prefix("data: ") {
            assert_eq!(serde_json::from_str::<Value>(data).unwrap(), event);
            break;
        }
    }
    let saved: Value = serde_json::from_slice(
        &std::fs::read(home.path().join("jellies").join(format!("{}.json", disk::name("test:counter")))).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["jelly"]["state"], event["state"]);
    let listed: Value = agent
        .get(&format!("{url}/api/jellies"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(listed[0]["name"], "test:counter");
    assert_eq!(listed[0]["version"], event["version"]);
    assert!(daemon.0.try_wait().unwrap().is_none());
    if !closed_stderr && matches!(failure, Failure::Trim) {
        assert!(std::fs::read_to_string(warnings)
            .unwrap()
            .contains("could not trim the event log: "));
    }
}

#[test]
fn event_log_failure_with_closed_stderr_rejects_mutation_and_preserves_store_access() {
    event_log_failure(Failure::Append, true);
}

#[test]
fn event_log_failure_with_writable_stderr_rejects_mutation_and_preserves_store_access() {
    event_log_failure(Failure::Append, false);
}

#[test]
fn event_log_trim_failure_with_closed_stderr_preserves_mutation_and_store_access() {
    event_log_failure(Failure::Trim, true);
}

#[test]
fn event_log_trim_failure_with_writable_stderr_still_warns() {
    event_log_failure(Failure::Trim, false);
}
