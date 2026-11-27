#![cfg(unix)]

mod common;

use std::process::{Child, Command, Stdio};

struct Daemon(Child, String);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The status the daemon gives a request that a proxy passed on with these headers.
fn status(url: &str, headers: &[(&str, &str)]) -> u16 {
    let request = headers.iter().fold(ureq::get(&format!("{url}/api/version")), |r, (name, value)| r.set(name, value));
    match request.call() {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r.status(),
        Err(e) => panic!("{e}"),
    }
}

/// `jelly trust` lets requests for one named address in, at once and with no
/// restart, and nothing else: not another host, not another site's page on that
/// address, and not the address itself once it is removed.
#[test]
fn the_daemon_answers_at_the_trusted_address_and_no_other() {
    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut child);
    let d = Daemon(child, url);
    let trust = |address: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1, "trust", address]).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let mini = ("Host", "mini.example.ts.net");

    assert_eq!(status(&d.1, &[mini]), 403);

    assert!(trust("https://mini.example.ts.net/").contains("jelly trusts https://mini.example.ts.net\n"));
    assert_eq!(status(&d.1, &[mini]), 200);
    assert_eq!(status(&d.1, &[mini, ("Origin", "https://mini.example.ts.net")]), 200);
    assert_eq!(status(&d.1, &[mini, ("Origin", "https://evil.example")]), 403);
    assert_eq!(status(&d.1, &[mini, ("Sec-Fetch-Site", "cross-site")]), 403);
    assert_eq!(status(&d.1, &[("Host", "evil.example")]), 403);
    // This machine's own address still works, and a page at the trusted address
    // cannot use it to pass as local.
    assert_eq!(status(&d.1, &[]), 200);
    assert_eq!(status(&d.1, &[("Origin", "https://mini.example.ts.net")]), 403);

    assert!(trust("").contains("this machine only"));
    assert_eq!(status(&d.1, &[mini]), 403);
}
