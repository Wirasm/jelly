#![cfg(target_os = "windows")]

use std::process::Command;
use std::time::{Duration, Instant};

struct Protocol(String);

impl Drop for Protocol {
    fn drop(&mut self) {
        let output = Command::new("reg.exe")
            .args(["delete", &self.0, "/f"])
            .output()
            .expect("remove temporary URL protocol");
        assert!(output.status.success(), "{output:?}");
    }
}

fn register_value(key: &str, value: &str, data: &str) {
    let mut command = Command::new("reg.exe");
    command.args(["add", key, "/f", "/t", "REG_SZ", "/d", data]);
    if value.is_empty() {
        command.arg("/ve");
    } else {
        command.args(["/v", value]);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn open_delivers_the_printed_address_to_the_windows_handler() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("handler.ps1");
    let received = dir.path().join("received");
    std::fs::write(
        &script,
        "param([string]$Url)\n\
         $path = Join-Path $PSScriptRoot 'received'\n\
         [IO.File]::WriteAllText(\"$path.tmp\", $Url)\n\
         Move-Item -LiteralPath \"$path.tmp\" -Destination $path\n",
    )
    .unwrap();

    let scheme = format!(
        "jelly-test-{}",
        dir.path().file_name().unwrap().to_str().unwrap()
    )
    .to_ascii_lowercase();
    let key = format!("HKCU\\Software\\Classes\\{scheme}");
    register_value(&key, "", "URL:jelly opener test");
    let protocol = Protocol(key);
    register_value(&protocol.0, "URL Protocol", "");
    register_value(
        &format!("{}\\shell\\open\\command", protocol.0),
        "",
        &format!(
            "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\" \"%1\"",
            script.display()
        ),
    );

    // A private scheme exercises the real system handler without changing the browser default.
    let base = format!("{scheme}://localhost:7879");
    for (project, name, path) in [
        ("repro", None, "/p/repro"),
        ("", None, "/"),
        ("repro", Some("other:plan"), "/j/other:plan/"),
        ("a&b!c%d^e(f)", None, "/p/a&b!c%d^e(f)"),
    ] {
        let url = format!("{base}{path}");
        let mut command = Command::new(env!("CARGO_BIN_EXE_jelly"));
        command.args(["--url", &base, "--project", project, "open"]);
        if let Some(name) = name {
            command.arg(name);
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), format!("{url}\n"));
        assert!(output.stderr.is_empty(), "{output:?}");

        // START returns before its handler; the handler renames a complete capture into place.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !received.exists() {
            assert!(Instant::now() < deadline, "handler did not receive {url}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&received).unwrap(), url);
        std::fs::remove_file(&received).unwrap();
    }
}
