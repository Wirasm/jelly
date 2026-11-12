use std::net::TcpListener;
use std::process::Command;

fn hook(port: u16, home: &std::path::Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["--as", "hook", "--url", &format!("http://127.0.0.1:{port}"), "--project", "p", "inbox", "--hook"])
        .env("JELLY_HOME", home)
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn the_inbox_hook_says_so_when_the_daemon_does_not_answer() {
    let home = tempfile::tempdir().unwrap();
    // Connections complete in the backlog, but nothing ever answers.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let (ok, out) = hook(silent.local_addr().unwrap().port(), home.path());
    assert!(ok);
    assert!(out.starts_with("jelly inbox failed: "), "{out:?}");

    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (ok, out) = hook(port, home.path());
    assert!(ok);
    assert_eq!(out, "", "no daemon running is not worth a word");
}
