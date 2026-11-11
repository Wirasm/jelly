#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn open_always_prints_the_address_and_reports_browser_failures() {
    let dir = tempfile::tempdir().unwrap();
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
            let url = format!("http://127.0.0.1:7879{path}");
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_jelly"));
            cmd.args([
                "--url",
                "http://127.0.0.1:7879",
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
