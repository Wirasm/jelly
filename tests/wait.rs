use serde_json::{json, Value};
use std::net::TcpListener;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Daemon {
    child: Child,
    home: tempfile::TempDir,
    url: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Daemon {
    fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let child = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["serve", "--port", &port.to_string(), "--home"])
            .arg(home.path()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut daemon = Self { child, home, url: format!("http://127.0.0.1:{port}") };
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if ureq::get(&format!("{}/api/jellies", daemon.url)).timeout(Duration::from_millis(100)).call().is_ok() { break; }
            assert!(daemon.child.try_wait().unwrap().is_none(), "daemon exited");
            assert!(Instant::now() < end, "daemon did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        daemon.post("/api/jellies", json!({"name": "plan", "project": "test", "view": "<p>Choose</p>", "verbs": "const verbs = { tick(s) { s.n = (s.n || 0) + 1; } };", "state": {}, "by": "agent"}));
        daemon
    }

    fn post(&self, path: &str, value: Value) -> Value {
        ureq::post(&format!("{}{path}", self.url)).timeout(Duration::from_secs(3)).send_json(value).unwrap().into_json().unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_jelly"));
        command.env_remove("JELLY_AS").args(["--url", &self.url, "--project", "test", "--as", "agent"]).args(args);
        command
    }

    fn cli(&self, args: &[&str]) -> Output { self.command(args).output().unwrap() }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }

    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.home.path().join("events.jsonl")).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    fn wait(&self, token: &str) -> Value {
        serde_json::from_str(&self.ok(&["wait", "plan", "--since", token, "--timeout", "0"])).unwrap()
    }
}

fn token(output: &str) -> &str { output.split("since ").last().unwrap().trim() }

#[test]
fn answers_completed_before_wait_are_replayed_in_all_attention_forms() {
    let daemon = Daemon::start();
    let cases = [
        (vec!["ask", "plan", "Which?", "--option", "a=Quiet", "--recommend", "a"], json!({"option": "a", "text": "larger please", "by": "person"})),
        (vec!["ask", "plan", "Anything missing?"], json!({"text": "more space", "by": "person"})),
        (vec!["look", "plan", "Check links"], json!({"by": "person"})),
    ];
    for (args, answer) in cases {
        let output = daemon.ok(&args);
        assert!(output.contains("it shows at the top of the page"));
        let asked = daemon.log().pop().unwrap();
        assert_eq!(token(&output), format!("seq:{}", asked["seq"]));
        let id = asked["args"]["id"].as_str().unwrap();
        let answered = daemon.post(&format!("/api/jellies/test:plan/attention/{id}/answer"), answer);
        let mut expected = answered.clone();
        expected.as_object_mut().unwrap().remove("state");
        assert_eq!(daemon.wait(token(&output)), expected);
        let timeout = daemon.cli(&["wait", "plan", "--since", &format!("seq:{}", answered["seq"]), "--timeout", "0"]);
        assert_eq!(timeout.status.code(), Some(2));
        assert!(timeout.stdout.is_empty());
    }
    assert!(daemon.log().iter().all(|ev| ev.get("state").is_none()));
}

#[test]
fn mutation_tokens_and_spec_rounds_refer_to_their_own_events() {
    let daemon = Daemon::start();
    for args in [vec!["call", "plan", "tick"], vec!["reshape", "plan", "--about", "updated"]] {
        let output = daemon.ok(&args);
        assert_eq!(token(&output), format!("seq:{}", daemon.log().last().unwrap()["seq"]));
    }
    let spec = daemon.home.path().join("spec.json");
    std::fs::write(&spec, json!({"title": "Choice", "questions": [{"id": "one", "question": "Which?", "options": [{"id": "a", "label": "Quiet"}]}]}).to_string()).unwrap();
    for round in 1..=2 {
        let output: Value = serde_json::from_str(&daemon.ok(&["ask", "sharing", "--spec", spec.to_str().unwrap()])).unwrap();
        let log = daemon.log();
        let event = log.last().unwrap();
        assert_eq!(event["kind"], "attention");
        assert_eq!(output["round"], round);
        assert_eq!(output["jelly"], "test:sharing");
        assert_eq!(output["since"], format!("seq:{}", event["seq"]));
        assert!(output["next"].as_str().unwrap().contains(&format!("--since {}", output["since"].as_str().unwrap())));
        if round == 2 { assert!(log.iter().any(|e| e["kind"] == "withdraw")); }
    }
    let bad_name = daemon.cli(&["wait", "missing", "--since", "seq:0", "--timeout", "10"]);
    assert_eq!(bad_name.status.code(), Some(1));
    for bad in ["v2", "seq:-1", "seq:", "seq:+2", "seq:18446744073709551616"] {
        let result = daemon.cli(&["wait", "plan", "--since", bad]);
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains("use seq:N"));
    }
    let too_long = daemon.cli(&["wait", "plan", "--since", "seq:0", "--timeout", "18446744073709551615"]);
    assert_eq!(too_long.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&too_long.stderr).contains("timeout is too large"));
    for query in ["since=0", "since=0&by=", "since=no&by=agent"] {
        assert!(ureq::get(&format!("{}/api/wait?{query}", daemon.url)).call().is_err());
    }
}

#[test]
fn ordinary_wait_remains_live_only_and_since_timeout_keeps_its_diagnostic() {
    let daemon = Daemon::start();
    let asked = daemon.ok(&["ask", "plan", "Which?"]);
    daemon.post("/api/jellies/test:plan/attention/a1/answer", json!({"text": "yes", "by": "person"}));
    let timeout = daemon.cli(&["wait", "plan", "--timeout", "1"]);
    assert_eq!(timeout.status.code(), Some(2));
    assert!(timeout.stdout.is_empty());
    assert!(daemon.wait(token(&asked)).get("state").is_none());

    for since_mode in [false, true] {
        let head = format!("seq:{}", daemon.log().last().unwrap()["seq"]);
        let mut args = vec!["wait", "plan", "--timeout", "3"];
        if since_mode { args.extend(["--since", &head]); }
        let mut child = daemon.command(&args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && Instant::now() < end {
            daemon.post("/api/jellies/test:plan/call", json!({"verb": "tick", "by": "person"}));
            std::thread::sleep(Duration::from_millis(20));
        }
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("live wait did not finish");
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let event: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(event["by"], "person");
        assert_eq!(event.get("state").is_none(), since_mode);
        if !since_mode { assert!(event["state"]["n"].as_u64().unwrap() > 0); }
    }

    let head = daemon.log().last().unwrap()["seq"].clone();
    let mut child = daemon.command(&["wait", "plan", "--since", &format!("seq:{head}"), "--timeout", "1"])
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let end = Instant::now() + Duration::from_secs(4);
    while child.try_wait().unwrap().is_none() && Instant::now() < end {
        daemon.post("/api/jellies/test:plan/call", json!({"verb": "tick", "by": "agent"}));
        std::thread::sleep(Duration::from_millis(20));
    }
    if child.try_wait().unwrap().is_none() { child.kill().unwrap(); child.wait().unwrap(); panic!("ignored events extended timeout"); }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8(output.stderr).unwrap().trim(), "jelly: nobody changed test:plan in time");
    assert!(daemon.log().iter().all(|ev| ev.get("state").is_none()));
}

#[test]
fn historical_short_names_keep_the_current_project_after_removal() {
    for other_exists in [false, true] {
        let daemon = Daemon::start();
        let asked = daemon.ok(&["ask", "plan", "Which?"]);
        let answer = daemon.post("/api/jellies/test:plan/attention/a1/answer", json!({"text": "yes", "by": "person"}));
        ureq::delete(&format!("{}/api/jellies/test:plan?by=agent", daemon.url)).call().unwrap();
        if other_exists {
            daemon.post("/api/jellies", json!({"name": "plan", "project": "other", "view": "", "verbs": "const verbs = {};", "state": {}, "by": "person"}));
        }
        assert_eq!(daemon.wait(token(&asked))["seq"], answer["seq"]);
    }
}

#[test]
fn historical_short_names_still_fall_back_to_a_unique_existing_jelly() {
    for removed_before_ask in [false, true] {
        let daemon = Daemon::start();
        daemon.post("/api/jellies", json!({"name": "remote", "project": "other", "view": "", "verbs": "const verbs = {};", "state": {}, "by": "agent"}));
        if removed_before_ask {
            daemon.post("/api/jellies", json!({"name": "remote", "project": "test", "view": "", "verbs": "const verbs = {};", "state": {}, "by": "agent"}));
            ureq::delete(&format!("{}/api/jellies/test:remote?by=agent", daemon.url)).call().unwrap();
        }
        let asked = daemon.ok(&["ask", "remote", "Which?"]);
        let answer = daemon.post("/api/jellies/other:remote/attention/a1/answer", json!({"text": "yes", "by": "person"}));
        let result: Value = serde_json::from_str(&daemon.ok(&["wait", "remote", "--since", token(&asked), "--timeout", "0"])).unwrap();
        assert_eq!(result["seq"], answer["seq"]);
    }
}

#[test]
fn reading_through_an_intervening_change_reaches_the_answer() {
    let daemon = Daemon::start();
    let asked = daemon.ok(&["ask", "plan", "Which?"]);
    let change = daemon.post("/api/jellies/test:plan/call", json!({"verb": "tick", "by": "person"}));
    let answer = daemon.post("/api/jellies/test:plan/attention/a1/answer", json!({"text": "yes", "by": "person"}));
    let first = daemon.wait(token(&asked));
    assert_eq!(first["seq"], change["seq"]);
    assert_ne!(first["kind"], "answer");
    let next = daemon.wait(&format!("seq:{}", first["seq"]));
    assert_eq!(next["seq"], answer["seq"]);
    assert_eq!(next["kind"], "answer");
    assert_eq!(next["args"]["id"], "a1");
}
