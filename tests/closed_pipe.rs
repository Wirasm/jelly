use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(10);

fn closed_pipe() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    writer.into()
}

struct Process(Child);
impl Process {
    fn finish(&mut self) -> Output {
        let until = Instant::now() + LIMIT;
        while self.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < until, "jelly did not exit");
            thread::sleep(Duration::from_millis(10));
        }
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.0.stdout.take() {
            pipe.read_to_end(&mut stdout).unwrap();
        }
        if let Some(mut pipe) = self.0.stderr.take() {
            pipe.read_to_end(&mut stderr).unwrap();
        }
        Output {
            status: self.0.wait().unwrap(),
            stdout,
            stderr,
        }
    }
    fn quiet(&mut self) {
        let output = self.finish();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn command(url: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jelly"));
    cmd.args(["--url", url, "--project", ""])
        .args(args)
        .env("RUST_BACKTRACE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

struct Request {
    stream: TcpStream,
    method: String,
    path: String,
    body: Vec<u8>,
}
impl Request {
    fn read(stream: TcpStream) -> Self {
        stream.set_read_timeout(Some(LIMIT)).unwrap();
        stream.set_write_timeout(Some(LIMIT)).unwrap();
        let mut input = BufReader::new(stream);
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap().to_string();
        let path = parts.next().unwrap().to_string();
        let mut length = 0;
        loop {
            let mut header = String::new();
            assert!(
                input.read_line(&mut header).unwrap() > 0,
                "incomplete HTTP headers"
            );
            if header == "\r\n" {
                break;
            }
            if let Some((key, value)) = header.split_once(':') {
                if key.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap();
                }
            }
        }
        let mut body = vec![0; length];
        input.read_exact(&mut body).unwrap();
        Self {
            stream: input.into_inner(),
            method,
            path,
            body,
        }
    }
    fn reply(&mut self, body: &str) {
        write!(self.stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    }
}

struct Fixture {
    listener: TcpListener,
    url: String,
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        Self { listener, url }
    }
    fn request(&self) -> Request {
        let until = Instant::now() + LIMIT;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => return Request::read(stream),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < until, "missing HTTP request");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        }
    }
    fn no_request(&self) {
        assert!(
            matches!(self.listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}

#[test]
fn list_and_json_and_help_exit_quietly_with_a_closed_reader() {
    let fixture = Fixture::new();
    for args in [vec!["ls", "--all"], vec!["ls", "--all", "--json"]] {
        for closed in [false, true] {
            let mut cmd = command(&fixture.url, &args);
            if closed {
                cmd.stdout(closed_pipe());
            }
            let mut child = Process(cmd.spawn().unwrap());
            let mut req = fixture.request();
            assert_eq!(req.path, "/api/jellies");
            req.reply(
                r#"[{"name":"probe:first","version":1},{"name":"probe:second","version":2}]"#,
            );
            let out = child.finish();
            assert!(out.status.success(), "{out:?}");
            assert!(out.stderr.is_empty(), "{out:?}");
            if !closed {
                let text = String::from_utf8(out.stdout).unwrap();
                if args.contains(&"--json") {
                    assert_eq!(
                        serde_json::from_str::<serde_json::Value>(&text)
                            .unwrap()
                            .as_array()
                            .unwrap()
                            .len(),
                        2
                    );
                } else {
                    assert_eq!(text, "probe:first    (v1)\nprobe:second    (v2)\n");
                }
            }
        }
    }
    Process(
        command(&fixture.url, &["--help"])
            .stdout(closed_pipe())
            .spawn()
            .unwrap(),
    )
    .quiet();
}

#[test]
fn creation_and_asset_uploads_finish_before_output() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("view.html"), "<p>hello</p>").unwrap();
    std::fs::write(dir.path().join("verbs.js"), "const verbs = {};").unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    let mut files: Vec<_> = ["c.txt", "b.txt", "a.txt"]
        .iter()
        .map(|name| {
            let path = dir.path().join("assets").join(name);
            std::fs::write(&path, name).unwrap();
            path
        })
        .collect();
    files.sort();
    for new in [true, false] {
        for closed in [false, true] {
            let mut cmd = command(&fixture.url, &[]);
            if new {
                cmd.args(["new", "probe:x", "--from"]).arg(dir.path());
            } else {
                cmd.args(["asset", "probe:x"]).args(&files);
            }
            if closed {
                cmd.stdout(closed_pipe());
            }
            let mut child = Process(cmd.spawn().unwrap());
            if new {
                let mut req = fixture.request();
                assert_eq!((&*req.method, &*req.path), ("POST", "/api/jellies"));
                let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
                assert_eq!(body["name"], "probe:x");
                req.reply(r#"{"jelly":"probe:x","version":1,"kind":"new"}"#);
            }
            for file in &files {
                let name = file.file_name().unwrap().to_str().unwrap();
                let mut req = fixture.request();
                assert_eq!(req.method, "POST");
                assert_eq!(
                    req.path,
                    format!("/api/jellies/probe:x/assets/{name}?by=agent")
                );
                assert_eq!(req.body, name.as_bytes());
                req.reply("{}");
            }
            let out = child.finish();
            assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
            if !closed {
                let expected = format!(
                    "{}assets/a.txt  (1 KB)\nassets/b.txt  (1 KB)\nassets/c.txt  (1 KB)\n",
                    if new { "probe:x v1: made\n" } else { "" }
                );
                assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
            }
            fixture.no_request();
        }
    }
}

#[test]
fn streams_exit_at_the_failed_write_without_waiting_for_eof() {
    for cmd in ["events", "watch", "wait"] {
        let fixture = Fixture::new();
        let mut child = Process(
            command(&fixture.url, &[cmd])
                .stdout(closed_pipe())
                .spawn()
                .unwrap(),
        );
        let mut req = fixture.request();
        assert_eq!(req.path, "/api/events");
        write!(req.stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {{\"by\":\"person\",\"jelly\":\"probe:x\",\"state\":{{}}}}\n\n").unwrap();
        req.stream.flush().unwrap();
        child.quiet(); // req keeps the stream open until after the child exits
    }
}

#[test]
fn dev_stops_before_watching_or_after_a_complete_reshape() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let view = dir.path().join("view.html");
    std::fs::write(&view, "before").unwrap();
    let mut cmd = command(&fixture.url, &["dev", "probe:x"]);
    cmd.arg(dir.path()).stdout(closed_pipe());
    Process(cmd.spawn().unwrap()).quiet();
    fixture.no_request();

    let mut child = Process(
        command(&fixture.url, &["dev", "probe:x"])
            .arg(dir.path())
            .spawn()
            .unwrap(),
    );
    let pipe = child.0.stdout.take().unwrap();
    let (sent, received) = mpsc::channel();
    let reading = thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        drop(reader);
        sent.send(line).unwrap();
    });
    assert!(received
        .recv_timeout(LIMIT)
        .unwrap()
        .starts_with("watching "));
    reading.join().unwrap();
    // Set an explicit distinct timestamp rather than relying on filesystem clock resolution.
    let file = std::fs::File::options().write(true).open(&view).unwrap();
    std::fs::write(&view, "after").unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(2))
        .unwrap();
    let mut req = fixture.request();
    assert_eq!(
        (&*req.method, &*req.path),
        ("POST", "/api/jellies/probe:x/reshape")
    );
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["view"], "after");
    req.reply(r#"{"version":2}"#);
    child.quiet();
    fixture.no_request();
}

#[test]
fn domain_errors_stay_errors_with_closed_stdout() {
    let fixture = Fixture::new();
    let mut child = Process(
        command(&fixture.url, &["ls", "--all"])
            .stdout(closed_pipe())
            .spawn()
            .unwrap(),
    );
    let mut req = fixture.request();
    write!(req.stream, "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 16\r\nConnection: close\r\n\r\n{{\"error\":\"oops\"}}").unwrap();
    let out = child.finish();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(out.stderr, b"jelly: oops\n");
}

#[test]
fn serve_keeps_serving_with_closed_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Process(
        command(
            "http://unused",
            &["serve", "--port", &port.to_string(), "--home"],
        )
        .arg(dir.path())
        .stdout(closed_pipe())
        .spawn()
        .unwrap(),
    );
    let until = Instant::now() + LIMIT;
    loop {
        if let Ok(resp) = ureq::get(&format!("http://127.0.0.1:{port}/api/jellies"))
            .timeout(Duration::from_secs(1))
            .call()
        {
            assert_eq!(resp.status(), 200);
            break;
        }
        assert!(child.0.try_wait().unwrap().is_none(), "daemon exited");
        assert!(Instant::now() < until, "daemon never started");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn wait_retains_its_timeout_exit_with_closed_stdout() {
    let fixture = Fixture::new();
    let mut child = Process(
        command(&fixture.url, &["wait", "--timeout", "1"])
            .stdout(closed_pipe())
            .spawn()
            .unwrap(),
    );
    let mut req = fixture.request();
    write!(
        req.stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    req.stream.flush().unwrap();
    let out = child.finish();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(out.stderr, b"jelly: nobody changed any jelly in time\n");
}

#[cfg(unix)]
#[test]
fn non_broken_pipe_stdout_failure_exits_with_a_diagnostic() {
    let fixture = Fixture::new();
    let socket = std::os::unix::net::UnixDatagram::unbound().unwrap();
    let fd: std::os::fd::OwnedFd = socket.into();
    let mut stdout = std::fs::File::from(fd);
    let error = stdout.write_all(b"probe").unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::BrokenPipe);
    let mut child = Process(
        command(&fixture.url, &["ls", "--all"])
            .stdout(stdout)
            .spawn()
            .unwrap(),
    );
    let mut req = fixture.request();
    assert_eq!(req.path, "/api/jellies");
    req.reply(r#"[{"name":"probe:first","version":1}]"#);
    let out = child.finish();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(out.stderr, format!("jelly: {error}\n").as_bytes());
}
