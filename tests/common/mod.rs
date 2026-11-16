// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Child;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long a daemon may take to print its banner on a cold, loaded machine.
const START: Duration = Duration::from_secs(10);

/// Waits for the `jelly on <url>` line `serve` prints once it is listening, then
/// closes the daemon's stderr pipe, so later writes to it fail.
pub fn served_url(child: &mut Child) -> String {
    let stderr = child.stderr.take().unwrap();
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let mut stderr = BufReader::new(stderr);
        let mut line = String::new();
        let _ = stderr.read_line(&mut line);
        if !line.starts_with("jelly on ") {
            let _ = stderr.read_to_string(&mut line);
        }
        drop(stderr);
        let _ = sent.send(line);
    });
    let Ok(line) = received.recv_timeout(START) else {
        let _ = child.kill();
        panic!("daemon did not start");
    };
    match line.trim().strip_prefix("jelly on ") {
        Some(url) => url.to_string(),
        None => panic!("daemon exited ({}): {line}", child.wait().unwrap()),
    }
}

/// `served_url` for a daemon whose stderr is the file at `path`.
pub fn served_url_in(child: &mut Child, path: &Path) -> String {
    let deadline = Instant::now() + START;
    loop {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let complete = &text[..text.rfind('\n').map_or(0, |end| end + 1)];
        if let Some(url) = complete.lines().find_map(|line| line.strip_prefix("jelly on ")) {
            return url.to_string();
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("daemon exited ({status}): {text}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("daemon did not start");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// A port nothing listens on, held so that no `bind` to port 0 and no outgoing connection
/// is handed it: it is the local end of an open loopback connection, so connecting to it is
/// refused at once. (A bound socket that never listens would not do: macOS drops
/// connections to it instead of refusing them.) Keep the streams alive while the port is used.
pub fn refused_port() -> ([TcpStream; 2], u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    let port = client.local_addr().unwrap().port();
    ([client, server], port)
}

/// A free port for a daemon that has to be started on a port chosen beforehand, such as
/// one `jelly` starts on its own. It lies below every ephemeral range (Linux starts at
/// 32768, macOS at 49152), so no `bind` to port 0 and no outgoing connection is handed
/// it after the probe releases it. Each process starts at a different point, so parallel
/// test processes do not try the same ports.
pub fn unassigned_port() -> u16 {
    unassigned_listener().local_addr().unwrap().port()
}

/// The probe behind `unassigned_port`, kept open for a test that needs the port taken.
pub fn unassigned_listener() -> TcpListener {
    const FIRST: u16 = 20_000;
    const COUNT: u16 = 12_000;
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let start = (std::process::id().wrapping_mul(97) % u32::from(COUNT)) as u16;
    loop {
        let port = FIRST + (start + NEXT.fetch_add(1, Ordering::Relaxed) % COUNT) % COUNT;
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return listener;
        }
    }
}
