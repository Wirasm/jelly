//! A render that fails says which part of the page was not checked, so a page is
//! never reported clean, fixed or newly broken on the strength of a render that
//! did not happen. The browser here is a script that runs the real one but fails
//! the dark-mode renders.
#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

struct Daemon(Child, String, #[allow(dead_code)] tempfile::TempDir);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Text that is white on white only in dark mode, so only the dark render sees it.
const VIEW: &str = r#"<style>
    :root { color-scheme: light dark; }
    .card { background: #fff; color: #222; padding: 12px; }
    @media (prefers-color-scheme: dark) { body { background: #111; } .card { color: #fff; } }
    </style><div class="card"><p id="total">Total due: 42 kr</p></div>"#;

/// A daemon whose browser fails dark renders: all of them, or with `once` only the first.
fn daemon(once: bool) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let chrome = std::env::var("JELLY_CHROME").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| {
        ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"]
            .into_iter().find(|c| std::path::Path::new(c).exists()).expect("these tests need Chrome; set JELLY_CHROME").into()
    });
    let fail = if once { format!("mkdir '{}/failed' 2>/dev/null && exit 1", dir.path().display()) } else { "exit 1".into() };
    let browser = dir.path().join("browser");
    std::fs::write(&browser, format!("#!/bin/sh\ncase \"$*\" in *preferredColorScheme=0*) {fail};; esac\nexec '{chrome}' \"$@\"\n")).unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(dir.path().join("home"))
        .env("JELLY_CHROME", &browser)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut child);
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:f", "view": VIEW, "verbs": "const verbs = { add(s) { s.n += 1; } };", "state": {"n": 0}, "by": "test"}))
        .unwrap();
    Daemon(child, url, dir)
}

fn jelly(d: &Daemon, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &d.1]).args(args).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_failed_dark_render_is_never_read_as_a_clean_page() {
    let d = daemon(false);
    let out = jelly(&d, &["look", "t:f", "--text", "--problems"]);
    assert!(out.starts_with("failed   dark mode at 1280px was not checked: the browser failed"), "{out}");
    assert!(out.contains("Run `jelly look` again; if it keeps failing, report it"), "{out}");
    let v: Value = serde_json::from_str(&jelly(&d, &["look", "t:f", "--json"])).unwrap();
    let problems: Vec<&str> = v["problems"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(problems.iter().any(|p| p.starts_with("failed   dark mode at desktop width was not checked")), "{v}");
    assert!(problems.iter().any(|p| p.starts_with("failed   dark mode at phone width was not checked")), "{v}");
}

/// With one side's dark render missing, a problem both sides have would look
/// different on each side, and so show as fixed or new.
#[test]
fn a_comparison_with_an_unchecked_side_calls_nothing_fixed_or_new() {
    for json in [false, true] {
        let d = daemon(true);
        ureq::post(&format!("{}/api/jellies/t:f/do", d.1)).send_json(json!({"verb": "add", "args": {}, "by": "test"})).unwrap();
        let mut args = vec!["look", "t:f", "--compare", "v1"];
        if json { args.push("--json"); }
        let out = jelly(&d, &args);
        assert!(out.contains("was not checked: the browser failed"), "{out}");
        if json {
            let v: Value = serde_json::from_str(&out).unwrap();
            assert_eq!((&v["new_problems"], &v["fixed_problems"]), (&Value::Null, &Value::Null), "{v}");
        } else {
            assert!(!out.contains("fixed ") && !out.contains(" new: "), "{out}");
        }
    }
}
