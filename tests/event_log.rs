use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(10);

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn event_log_failure(closed_stderr: bool) {
    let home = tempfile::tempdir().unwrap();
    let warnings = home.path().join("stderr.txt");
    let stderr = if closed_stderr {
        Stdio::piped()
    } else {
        std::fs::File::create(&warnings).unwrap().into()
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["serve", "--port", &port.to_string(), "--home"])
            .arg(home.path())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    let url = format!("http://127.0.0.1:{port}");
    let agent = ureq::AgentBuilder::new().timeout(LIMIT).build();
    let deadline = Instant::now() + LIMIT;
    loop {
        if agent.get(&format!("{url}/api/jellies")).call().is_ok() {
            break;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon exited during startup"
        );
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let post = |path: &str, body: Value| -> Value {
        agent
            .post(&format!("{url}{path}"))
            .send_json(body)
            .unwrap()
            .into_json()
            .unwrap()
    };
    post(
        "/api/jellies",
        json!({
            "name": "counter", "project": "test", "view": "<p>counter</p>",
            "verbs": "const verbs = {increment(s) { s.count += 1; }};",
            "state": {"count": 0}, "by": "test"
        }),
    );
    // Close only after startup: this exercises the event warning, not the startup diagnostic.
    if closed_stderr {
        drop(daemon.0.stderr.take().unwrap());
    }
    std::fs::remove_file(home.path().join("events.jsonl")).unwrap();
    std::fs::create_dir(home.path().join("events.jsonl")).unwrap();
    let stream = agent
        .get(&format!("{url}/api/events?jelly=test:counter"))
        .call()
        .unwrap();
    let mut stream = BufReader::new(stream.into_reader());

    let event = post(
        "/api/jellies/test:counter/call",
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
        &std::fs::read(home.path().join("jellies/test:counter.json")).unwrap(),
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
    if !closed_stderr {
        assert!(std::fs::read_to_string(warnings)
            .unwrap()
            .contains("could not log event: "));
    }
}

#[test]
fn event_log_failure_with_closed_stderr_preserves_mutation_and_store_access() {
    event_log_failure(true);
}

#[test]
fn event_log_failure_with_writable_stderr_still_warns() {
    event_log_failure(false);
}
