#![cfg(any(target_os = "macos", target_os = "linux"))]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn open_always_prints_the_address_and_reports_browser_failures() {
    let dir = tempfile::tempdir().unwrap();
    // `open NAME` asks the daemon whether the jelly exists, so it needs one.
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(dir.path().join("home"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let base = common::served_url(&mut daemon);
    let _daemon = Daemon(daemon);
    ureq::post(&format!("{base}/api/jellies"))
        .send_json(serde_json::json!({"name": "other:plan", "view": "<p>plan</p>", "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let opener = dir.path().join(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    let arguments = dir.path().join("arguments");

    for exit in [None, Some(7), Some(0)] {
        if let Some(code) = exit {
            std::fs::write(
                &opener,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$JELLY_TEST_OPEN_ARGS\"\nexit {code}\n"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        for (project, name, path) in [
            ("repro", None, "/p/repro"),
            ("", None, "/"),
            ("repro", Some("other:plan"), "/j/other:plan/"),
        ] {
            let url = format!("{base}{path}");
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_jelly"));
            cmd.args([
                "--url",
                &base,
                "--project",
                project,
                "open",
            ])
            .env("PATH", dir.path())
            .env("JELLY_TEST_OPEN_ARGS", &arguments);
            if let Some(name) = name {
                cmd.arg(name);
            }
            let output = cmd.output().unwrap();
            let (reader, writer) = std::io::pipe().unwrap();
            drop(reader);
            if exit.is_some() {
                std::fs::remove_file(&arguments).unwrap();
            }
            let closed = cmd
                .stdout(std::process::Stdio::from(writer))
                .output()
                .unwrap();
            assert!(closed.status.success(), "{closed:?}");
            if exit == Some(0) {
                assert!(closed.stderr.is_empty(), "{closed:?}");
            } else {
                let stderr = String::from_utf8(closed.stderr).unwrap();
                assert!(stderr.contains("could not open a browser"), "{stderr}");
                assert!(stderr.contains(&url), "{stderr}");
            }
            if exit.is_some() {
                assert_eq!(
                    std::fs::read_to_string(&arguments).unwrap(),
                    format!("{url}\n")
                );
            }
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                format!("{url}\n")
            );
            let stderr = String::from_utf8(output.stderr).unwrap();
            if exit == Some(0) {
                assert!(stderr.is_empty(), "{stderr}");
            } else {
                assert!(stderr.contains("could not open a browser"), "{stderr}");
                assert!(stderr.contains(&url), "{stderr}");
            }
            if exit.is_some() {
                assert_eq!(
                    std::fs::read_to_string(&arguments).unwrap(),
                    format!("{url}\n")
                );
            }
        }
    }
}
