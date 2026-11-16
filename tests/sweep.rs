mod common;

use serde_json::{json, Value};
use std::path::Path;
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

fn closed_pipe() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer.into()
}

enum Stderr<'a> {
    /// Read only for the banner, then closed.
    Banner,
    File(&'a Path),
    /// Closed from the start, so the startup sweep's warning cannot be written.
    Closed,
}

fn start(home: &Path, stderr: Stderr) -> (Daemon, String, ureq::Agent) {
    let serve = |port: &str, stderr: Stdio| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jelly"));
        command.args(["serve", "--port", port, "--home"]).arg(home);
        Daemon(command.stdout(Stdio::null()).stderr(stderr).spawn().unwrap())
    };
    let agent = ureq::AgentBuilder::new().timeout(LIMIT).build();
    let (daemon, url) = match stderr {
        Stderr::Banner => {
            let mut daemon = serve("0", Stdio::piped());
            let url = common::served_url(&mut daemon.0);
            (daemon, url)
        }
        Stderr::File(path) => {
            let mut daemon = serve("0", std::fs::File::create(path).unwrap().into());
            let url = common::served_url_in(&mut daemon.0, path);
            (daemon, url)
        }
        // The daemon cannot print its banner, so it gets a port chosen beforehand.
        Stderr::Closed => {
            let port = common::unassigned_port();
            let mut daemon = serve(&port.to_string(), closed_pipe());
            let url = format!("http://127.0.0.1:{port}");
            let deadline = Instant::now() + LIMIT;
            while agent.get(&format!("{url}/api/jellies")).call().is_err() {
                assert!(
                    daemon.0.try_wait().unwrap().is_none(),
                    "daemon exited during startup"
                );
                assert!(Instant::now() < deadline, "daemon did not start");
                std::thread::sleep(Duration::from_millis(20));
            }
            (daemon, url)
        }
    };
    (daemon, url, agent)
}

/// Two week-idle jellies: archiving `test:a` fails, `test:b` archives after it.
fn idle_jellies(home: &Path) {
    let (daemon, url, agent) = start(home, Stderr::Banner);
    for name in ["a", "b"] {
        agent
            .post(&format!("{url}/api/jellies"))
            .send_json(json!({
                "name": name, "project": "test", "view": "<p></p>",
                "verbs": "const verbs = {};", "state": {}, "by": "test"
            }))
            .unwrap();
    }
    drop(daemon);
    for name in ["a", "b"] {
        let path = home.join(format!("jellies/test:{name}.json"));
        let mut entry: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        entry["jelly"]["updated_ms"] = json!(0);
        std::fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
    }
    std::fs::create_dir(home.join("jellies/test:a.json.tmp")).unwrap();
}

fn archive_failure(closed_stderr: bool) {
    let home = tempfile::tempdir().unwrap();
    idle_jellies(home.path());
    let warnings = home.path().join("stderr.txt");
    let stderr = if closed_stderr { Stderr::Closed } else { Stderr::File(&warnings) };
    let (mut daemon, url, agent) = start(home.path(), stderr);
    let deadline = Instant::now() + LIMIT;
    loop {
        let listed: Value = agent
            .get(&format!("{url}/api/jellies"))
            .call()
            .expect("the store stays usable after a failed archive")
            .into_json()
            .unwrap();
        let archived = |name: &str| {
            listed
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["name"] == name)
                .unwrap()["archived"]
                .as_bool()
                .unwrap()
        };
        assert!(!archived("test:a"));
        if archived("test:b") {
            break;
        }
        assert!(Instant::now() < deadline, "the sweep did not run");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::remove_dir(home.path().join("jellies/test:a.json.tmp")).unwrap();
    agent
        .post(&format!("{url}/api/jellies/test:a/pin"))
        .send_json(json!({"pinned": true, "by": "test"}))
        .expect("the store accepts changes after a failed archive");
    assert!(daemon.0.try_wait().unwrap().is_none());
    if !closed_stderr {
        assert!(std::fs::read_to_string(warnings)
            .unwrap()
            .contains("could not archive test:a: "));
    }
}

#[test]
fn archive_failure_with_closed_stderr_preserves_store_access() {
    archive_failure(true);
}

#[test]
fn archive_failure_with_writable_stderr_still_warns() {
    archive_failure(false);
}
