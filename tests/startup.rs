#![cfg(unix)]

#[path = "../src/startup.rs"]
#[allow(dead_code)]
mod startup;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output};
use std::time::Duration;

struct Sandbox(tempfile::TempDir);
impl Sandbox {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    fn command(&self, port: u16, slot: usize) -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "driver", "--nocapture"])
            .env("STARTUP_TEST_ROOT", self.0.path())
            .env("STARTUP_TEST_PORT", port.to_string())
            .env("STARTUP_TEST_SLOT", slot.to_string())
            .env("TOKIO_WORKER_THREADS", "2")
            .env_remove("JELLY_URL")
            .env_remove("JELLY_PROJECT")
            .env_remove("JELLY_AS")
            .env("HOME", self.0.path())
            .env("JELLY_HOME", self.0.path().join(format!("home-{slot}")))
            .current_dir(self.0.path());
        c
    }
    fn launch(&self, port: u16, slot: usize, quiet: bool, project: &str) -> Output {
        self.command(port, slot)
            .env("STARTUP_TEST_MODE", "client")
            .env("STARTUP_TEST_QUIET", if quiet { "1" } else { "0" })
            .env("STARTUP_TEST_PROJECT", project)
            .output()
            .unwrap()
    }
    fn pid(&self, slot: usize) -> i32 {
        std::fs::read_to_string(self.0.path().join(format!("pid-{slot}")))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        for file in std::fs::read_dir(self.0.path()).unwrap().flatten() {
            if file.file_name().to_string_lossy().starts_with("pid-") {
                if let Ok(pid) = std::fs::read_to_string(file.path())
                    .unwrap()
                    .trim()
                    .parse::<i32>()
                {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                }
            }
        }
    }
}
fn port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}
fn ready(port: u16) -> bool {
    ureq::get(&format!("{}/api/jellies", url(port)))
        .timeout(Duration::from_millis(250))
        .call()
        .is_ok()
}

fn read_request(stream: &mut std::net::TcpStream) {
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
    }
}

#[test]
fn driver() {
    let Ok(mode) = std::env::var("STARTUP_TEST_MODE") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("STARTUP_TEST_ROOT").unwrap());
    let port = std::env::var("STARTUP_TEST_PORT").unwrap();
    let slot = std::env::var("STARTUP_TEST_SLOT").unwrap();
    if mode == "server" {
        std::fs::write(
            root.join(format!("pid-{slot}")),
            std::process::id().to_string(),
        )
        .unwrap();
        let error = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["serve", "--port", &port])
            .exec();
        panic!("exec: {error}");
    }
    let quiet = std::env::var("STARTUP_TEST_QUIET").unwrap() == "1";
    let mut server = Command::new(std::env::current_exe().unwrap());
    server
        .args(["--exact", "driver", "--nocapture"])
        .env("STARTUP_TEST_MODE", "server");
    let endpoint = format!("http://127.0.0.1:{port}");
    let result = if std::env::var_os("STARTUP_TEST_FAIL").is_some() {
        startup::ensure_with_timeout(&endpoint, quiet, server, Duration::from_millis(600))
    } else {
        startup::ensure_running(&endpoint, quiet, server)
    };
    if let Err(e) = result {
        if quiet && std::env::var_os("STARTUP_TEST_FAIL").is_some() {
            return;
        }
        panic!("{e}");
    }
    let project = std::env::var("STARTUP_TEST_PROJECT").unwrap();
    let mut cli = Command::new(env!("CARGO_BIN_EXE_jelly"));
    cli.args([
        "--url",
        &format!("http://127.0.0.1:{port}"),
        "--project",
        &project,
    ]);
    if quiet {
        cli.args(["inbox", "--hook"]);
    } else {
        cli.args(["ls", "--json"]);
    }
    let out = cli.output().unwrap();
    std::fs::write(root.join(format!("stdout-{slot}")), &out.stdout).unwrap();
    std::fs::write(root.join(format!("stderr-{slot}")), &out.stderr).unwrap();
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn cold_start_notice_existing_daemon_and_detached_lifetime() {
    let s = Sandbox::new();
    let p = port();
    let first = s.launch(p, 0, false, "test");
    assert!(first.status.success(), "{first:?}");
    assert_eq!(
        String::from_utf8(first.stderr).unwrap(),
        format!("jelly: daemon started at {}\n", url(p))
    );
    let stdout = std::fs::read(s.0.path().join("stdout-0")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stdout).unwrap(),
        serde_json::json!([])
    );
    assert!(std::fs::read(s.0.path().join("stderr-0"))
        .unwrap()
        .is_empty());
    let pid = s.pid(0);
    assert_eq!(unsafe { libc::getsid(pid) }, pid);
    assert!(
        ready(p),
        "daemon survives completed launcher and closed pipes"
    );
    let second = s.launch(p, 1, false, "test");
    assert!(second.status.success());
    assert!(second.stderr.is_empty());
    assert!(!s.0.path().join("pid-1").exists());
}

#[test]
fn racing_starts_initialize_only_one_home() {
    let s = Sandbox::new();
    let p = port();
    let clients: Vec<_> = (0..8)
        .map(|slot| {
            s.command(p, slot)
                .env("STARTUP_TEST_MODE", "client")
                .env("STARTUP_TEST_QUIET", "0")
                .env("STARTUP_TEST_PROJECT", "test")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut client in clients {
        assert!(client.wait().unwrap().success());
    }
    let homes: Vec<_> = (0..8)
        .map(|i| s.0.path().join(format!("home-{i}")))
        .filter(|p| p.exists())
        .collect();
    assert_eq!(homes.len(), 1);
    let events = std::fs::read_to_string(homes[0].join("events.jsonl")).unwrap();
    let events: Vec<serde_json::Value> = events
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], "new");
    assert_eq!(events[1]["kind"], "pin");
    assert!(ready(p));
}

#[cfg(target_os = "linux")]
#[test]
fn daemon_listens_only_on_127_0_0_1() {
    let s = Sandbox::new();
    let p = port();
    let out = s.launch(p, 0, false, "test");
    assert!(out.status.success(), "{out:?}");
    let other_address = TcpListener::bind(("127.0.0.2", p))
        .expect("daemon must leave addresses other than 127.0.0.1 unbound");
    assert!(ready(p));
    drop(other_address);
}

#[test]
fn readiness_allows_slow_response_body() {
    for quiet in [false, true] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let p = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n")
                .unwrap();
            std::thread::sleep(Duration::from_millis(350));
            let _ = stream.write_all(b"[]");
        });
        let result = startup::ensure_with_timeout(
            &url(p),
            quiet,
            Command::new("/must/not/spawn"),
            Duration::from_secs(3),
        );
        server.join().unwrap();
        assert!(result.is_ok(), "{result:?}");
    }
}

#[test]
fn quiet_hooks_start_inside_and_outside_projects_and_keep_new_items() {
    for project in ["", "test"] {
        let s = Sandbox::new();
        let p = port();
        let out = s.launch(p, 0, true, project);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty());
        assert!(std::fs::read(s.0.path().join("stdout-0"))
            .unwrap()
            .is_empty());
        assert!(std::fs::read(s.0.path().join("stderr-0"))
            .unwrap()
            .is_empty());
        assert!(ready(p));
        if project.is_empty() {
            continue;
        }
        ureq::post(&format!("{}/api/jellies", url(p))).send_json(serde_json::json!({
            "name": "demo", "project": "test", "view": "", "verbs": "const verbs = {};", "state": {}, "by": "agent"
        })).unwrap();
        ureq::post(&format!("{}/api/jellies/test:demo/comment", url(p)))
            .send_json(serde_json::json!({
                "anchor": {}, "text": "Please change this", "by": "person"
            }))
            .unwrap();
        let out = s.launch(p, 0, true, project);
        assert!(out.status.success(), "{out:?}");
        assert!(out.stderr.is_empty());
        let text = std::fs::read_to_string(s.0.path().join("stdout-0")).unwrap();
        assert!(text.contains("Please change this"));
    }
}

#[test]
fn manual_bind_failure_does_not_touch_home_and_custom_url_stays_selected() {
    let s = Sandbox::new();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let p = listener.local_addr().unwrap().port();
    let home = s.0.path().join("rejected-home");
    let result = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", &p.to_string(), "--home"])
        .arg(&home)
        .env("HOME", s.0.path())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!home.exists());
    drop(listener);
    for env in [false, true] {
        let mut c = Command::new(env!("CARGO_BIN_EXE_jelly"));
        c.env_remove("JELLY_URL")
            .env("HOME", s.0.path())
            .env("JELLY_HOME", &home)
            .current_dir(s.0.path());
        if env {
            c.env("JELLY_URL", url(p));
        } else {
            c.args(["--url", &url(p)]);
        }
        let result = c.args(["ls", "--json"]).output().unwrap();
        assert!(!result.status.success());
        assert!(!home.exists());
        assert!(!String::from_utf8(result.stderr)
            .unwrap()
            .contains("daemon started"));
    }
    let mut server = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", &p.to_string(), "--home"])
        .arg(&home)
        .env("HOME", s.0.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = s.0.path().join("pid-0");
    std::fs::write(&pid_file, server.id().to_string()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready(p) {
        assert!(std::time::Instant::now() < deadline);
        assert!(server.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(home.join("jellies/surface.json").exists());
    let result = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--url", &url(p), "ls", "--json"])
        .env("HOME", s.0.path())
        .current_dir(s.0.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(result.stderr.is_empty());
    server.kill().unwrap();
    server.wait().unwrap();
    std::fs::remove_file(pid_file).unwrap();
}

#[test]
fn startup_failures_and_readiness_are_bounded() {
    let missing = || Command::new("/no/such/jelly-executable");
    let p = port();
    let error = startup::ensure_running(&url(p), true, missing()).unwrap_err();
    assert!(error.contains("cannot start daemon") && error.contains("jelly serve"));

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let p = listener.local_addr().unwrap().port();
    let reply = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });
    let e = startup::ensure_running(&url(p), false, missing()).unwrap_err();
    assert!(e.contains("expected a JSON array"));
    reply.join().unwrap();

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let p = listener.local_addr().unwrap().port();
    let started = std::time::Instant::now();
    let e = startup::ensure_with_timeout(&url(p), true, missing(), Duration::from_millis(350))
        .unwrap_err();
    assert!(e.contains("did not become ready"));
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn daemon_survives_hangup_of_originating_shell() {
    let s = Sandbox::new();
    let p = port();
    let base = s.command(p, 0);
    let mut shell = Command::new("/bin/sh");
    shell
        .args(["-c", "\"$@\" && kill -HUP -$$", "startup-shell"])
        .arg(base.get_program())
        .args(base.get_args())
        .current_dir(s.0.path())
        .process_group(0);
    for (key, value) in base.get_envs() {
        if let Some(value) = value {
            shell.env(key, value);
        } else {
            shell.env_remove(key);
        }
    }
    let out = shell
        .env("STARTUP_TEST_MODE", "client")
        .env("STARTUP_TEST_QUIET", "0")
        .env("STARTUP_TEST_PROJECT", "test")
        .output()
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(out.status.signal(), Some(libc::SIGHUP), "{out:?}");
    assert!(ready(p));
    let pid = s.pid(0);
    assert_eq!(unsafe { libc::getsid(pid) }, pid);
}

#[test]
fn invalid_home_and_early_exit_do_not_report_success_and_hooks_suppress_errors() {
    let s = Sandbox::new();
    let p = port();
    std::fs::write(s.0.path().join("home-0"), "not a directory").unwrap();
    for quiet in [true, false] {
        let out = s
            .command(p, 0)
            .env("STARTUP_TEST_MODE", "client")
            .env("STARTUP_TEST_QUIET", if quiet { "1" } else { "0" })
            .env("STARTUP_TEST_FAIL", "1")
            .env("STARTUP_TEST_PROJECT", "test")
            .output()
            .unwrap();
        assert_eq!(out.status.success(), quiet, "{out:?}");
        let stderr = String::from_utf8(out.stderr).unwrap();
        if quiet {
            assert!(stderr.is_empty());
        } else {
            assert!(stderr.contains("startup process") && stderr.contains("jelly serve"), "{stderr}");
        }
        assert!(!stderr.contains("daemon started"));
        assert!(!ready(p));
    }
}

#[test]
fn waits_for_bound_server_and_leaves_timed_out_daemon_running() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let p = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]")
            .unwrap();
    });
    startup::ensure_running(&url(p), false, Command::new("/must/not/spawn")).unwrap();
    server.join().unwrap();

    let s = Sandbox::new();
    let p = port();
    let pid_file = s.0.path().join("pid-0");
    let mut child = Command::new("/bin/sh");
    child
        .args(["-c", "echo $$ > \"$1\"; exec sleep 30", "never-ready"])
        .arg(&pid_file);
    let error = startup::ensure_with_timeout(&url(p), false, child, Duration::from_millis(350))
        .unwrap_err();
    assert!(error.contains("did not become ready"));
    let pid = s.pid(0);
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        0,
        "a timed-out starter must not kill a potentially shared daemon"
    );
    unsafe {
        libc::kill(pid, libc::SIGTERM);
        libc::waitpid(pid, std::ptr::null_mut(), 0);
    }
    std::fs::remove_file(pid_file).unwrap();
}
