#![cfg(unix)]

mod common;

use serde_json::json;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// A view that keeps a connection open (here an EventSource to an outside
/// server) never lets the network go quiet. `look` and `try` still show it,
/// with what the connection delivered, instead of timing out (#198).
#[test]
fn look_and_try_show_a_page_with_an_open_connection() {
    // An event stream that sends one message and then stays open.
    let stream = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let stream_url = format!("http://{}/events", stream.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut open = Vec::new();
        for mut conn in stream.incoming().flatten() {
            let _ = conn.read(&mut [0; 4096]);
            let _ = conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\naccess-control-allow-origin: *\r\n\r\ndata: tick\n\n");
            open.push(conn);
        }
    });
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    let view = format!(r#"<p id="t">waiting</p><button id="b" onclick="jelly.do('bump')">Bump</button><p id="n"></p><script>
        new EventSource("{stream_url}").onmessage = (e) => {{ t.textContent = "got " + e.data; }};
        jelly.onState((s) => {{ n.textContent = "n=" + s.n; }});
    </script>"#);
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:x", "view": view, "verbs": "const verbs = { bump(s) { s.n++; } };", "state": {"n": 0}, "by": "test"}))
        .unwrap();
    let jelly = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url]).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let seen = jelly(&["look", "t:x", "--text"]);
    let tried = jelly(&["try", "t:x", "--click", "#b"]);
    let _ = daemon.kill();
    assert!(seen.contains(r#"text "got tick" #t"#), "{seen}");
    assert!(tried.contains(r#"text "n=1" #n"#), "{tried}");
}
