#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// Sends one raw request and returns the status line and the whole response.
fn raw(addr: &str, request: &str) -> (String, String) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // A WebSocket upgrade keeps the connection open, so read until headers end.
    while let Ok(n) = s.read(&mut chunk) {
        if n == 0 { break; }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") && !String::from_utf8_lossy(&buf).contains("Content-Length") { break; }
        if String::from_utf8_lossy(&buf).contains("\r\n\r\n") && buf.len() > 200 { break; }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    (text.lines().next().unwrap_or("").to_string(), text)
}

/// jelly has no login, so only this machine may use it: another website cannot
/// read events over the WebSocket or call verbs, a rebound DNS name cannot reach
/// the API, a crafted link cannot run script on the daemon's address, and the
/// design routes cannot leave the home (#8, #66). The CLI and jelly's own pages
/// keep working.
#[test]
fn only_this_machine_can_use_the_daemon() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"]).arg(home.path())
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let url = common::served_url(&mut daemon);
    let addr = url.trim_start_matches("http://").to_string();
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:c", "view": "<p>c</p>", "verbs": "const verbs = { inc(s) { s.n++; } };", "state": {"n": 0}, "by": "test"}))
        .unwrap();
    let call = |origin: Option<&str>, host: &str| {
        let body = r#"{"verb":"inc","args":{},"by":"person"}"#;
        let origin = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
        raw(&addr, &format!("POST /api/jellies/t:c/call HTTP/1.1\r\nHost: {host}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).0
    };
    let ws = |origin: &str| raw(&addr, &format!("GET /api/ws HTTP/1.1\r\nHost: {addr}\r\nOrigin: {origin}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")).0;

    // Another site: no verb calls, no events.
    assert!(call(Some("https://evil.example"), &addr).contains("403"));
    assert!(call(Some("null"), &addr).contains("403"));
    assert!(ws("https://evil.example").contains("403"));
    // A rebound DNS name reaches 127.0.0.1 with a foreign Host.
    assert!(call(None, "evil.example").contains("403"));
    // A crafted name is refused, not written into the page.
    let (status, page) = raw(&addr, &format!("GET /look-frame/%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E?w=1&h=1 HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"));
    // Refused, and the error is JSON, which a browser never runs as a page.
    assert!(status.contains("400") && page.to_ascii_lowercase().contains("content-type: application/json"), "{page}");
    // The design routes stay inside the home.
    assert!(ureq::put(&format!("{url}/api/projects/..%2F..%2Fescape/design")).send_string("x").is_err());
    assert!(!home.path().join("escape").exists() && !home.path().parent().unwrap().join("escape").exists());

    // Nothing above changed the jelly.
    let state: Value = ureq::get(&format!("{url}/api/jellies/t:c/state")).call().unwrap().into_json().unwrap();
    assert_eq!(state["n"], 0);
    // The CLI (no Origin) and jelly's own pages (same Origin) still work, over HTTP and WebSocket.
    assert!(call(None, &addr).contains("200"));
    assert!(call(Some(&format!("http://{addr}")), &addr).contains("200"));
    assert!(ws(&format!("http://{addr}")).contains("101"));
    let localhost = addr.replace("127.0.0.1", "localhost");
    assert!(call(Some(&format!("http://{localhost}")), &localhost).contains("200"));
    // Pages may only be framed by the daemon's own pages.
    let view = ureq::get(&format!("{url}/j/t:c/")).call().unwrap();
    assert_eq!(view.header("content-security-policy"), Some("frame-ancestors 'self'"));
    let _ = daemon.kill();
}
