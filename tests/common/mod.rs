use std::io::{BufRead, BufReader, Read};
use std::process::Child;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Waits up to 10 s for the `jelly on <url>` line `serve` prints once it is listening.
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
        let _ = sent.send(line);
    });
    let Ok(line) = received.recv_timeout(Duration::from_secs(10)) else {
        let _ = child.kill();
        panic!("daemon did not start");
    };
    match line.trim().strip_prefix("jelly on ") {
        Some(url) => url.to_string(),
        None => panic!("daemon exited ({}): {line}", child.wait().unwrap()),
    }
}
