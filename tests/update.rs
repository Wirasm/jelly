#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

// What the binary reports (build.rs).
const VERSION: &str = env!("JELLY_VERSION");

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn version(url: &str) -> Option<Value> {
    ureq::get(&format!("{url}/api/version")).call().ok()?.into_json().ok()
}

/// Polls the daemon until `done` says yes to what `/api/version` answers.
fn until(url: &str, done: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..150 {
        if let Some(v) = version(url).filter(&done) {
            return v;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("the daemon never got there: {:?}", version(url));
}

/// jelly as npm installs it, with a release newer than it on the registry and
/// a stub for `npm`. The stub writes down how it was called and puts a "new
/// release" at the binary's path: a script that says it is 99.0.0 and is
/// otherwise this build.
struct NpmInstall {
    dir: tempfile::TempDir,
    exe: PathBuf,
    npm: PathBuf,
    registry: String,
}

impl NpmInstall {
    fn new() -> NpmInstall {
        let dir = tempfile::tempdir().unwrap();
        let at = |p: &str| dir.path().join(p);
        let bin = at("node_modules/@getjellyware/jelly-test/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(at("home/.claude")).unwrap();
        let exe = bin.join("jelly");
        std::fs::copy(env!("CARGO_BIN_EXE_jelly"), &exe).unwrap();
        let install = NpmInstall { exe, npm: at("npm"), registry: common::registry("99.0.0"), dir };
        install.npm_works();
        install
    }

    fn at(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }

    fn npm_works(&self) {
        script(&self.at("release"), &format!("#!/bin/sh\ncase \"$1\" in --version) echo 'jelly 99.0.0';; *) exec {} \"$@\";; esac\n", env!("CARGO_BIN_EXE_jelly")));
        script(&self.npm, &format!("#!/bin/sh\necho \"$@\" >> {}\nmv -f {} {}\n", self.at("npm-ran").display(), self.at("release").display(), self.exe.display()));
    }

    fn jelly(&self) -> Command {
        let mut command = Command::new(&self.exe);
        command.env("HOME", self.at("home")).env("JELLY_HOME", self.at("jellies"))
            .env("JELLY_UPDATE_URL", &self.registry).env("JELLY_NPM", &self.npm);
        command
    }

    fn daemon(&self) -> (Child, String) {
        let port = common::unassigned_port().to_string();
        let mut daemon = self.jelly().args(["serve", "--port", &port, "--home"]).arg(self.at("jellies"))
            .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let url = common::served_url(&mut daemon);
        (daemon, url)
    }
}

/// `jelly update` on an npm install: npm installs the latest release, the new
/// binary writes the agents' skill, and the daemon starts again on it.
#[test]
fn update_installs_with_npm_writes_the_skill_and_restarts_the_daemon() {
    let install = NpmInstall::new();
    let (mut daemon, url) = install.daemon();
    let before = until(&url, |v| v["latest"] == "99.0.0");
    assert_eq!((before["install"].as_str(), before["newer"].as_bool()), (Some("npm"), Some(true)), "{before}");

    let out = install.jelly().args(["--url", &url, "update"]).output().unwrap();
    let said = text(&out);
    let after = version(&url);
    let _ = daemon.kill();
    assert!(out.status.success(), "{said}");
    assert_eq!(std::fs::read_to_string(install.at("npm-ran")).unwrap(), "install -g jellyware@latest\n");
    assert!(install.at("home/.claude/skills/jelly/SKILL.md").exists(), "{said}");
    assert!(said.contains("Claude Code  skill") && said.contains("Restarted the daemon") && said.contains("jelly is now 99.0.0"), "{said}");
    assert_ne!(after.expect("the daemon is back")["started_ms"], before["started_ms"]);
}

/// The jelly page's "Update now": the daemon says why an update failed, and
/// after one that worked it starts again, which the page sees as a new start time.
#[test]
fn the_daemon_updates_itself_and_says_when_it_could_not() {
    let install = NpmInstall::new();
    script(&install.npm, "#!/bin/sh\necho 'npm error code EACCES' >&2\nexit 243\n");
    let (mut daemon, url) = install.daemon();
    let before = until(&url, |v| v["latest"] == "99.0.0");
    let update = || ureq::post(&format!("{url}/api/update"));

    // Another website cannot start an update.
    assert!(matches!(update().set("Origin", "https://evil.example").call(), Err(ureq::Error::Status(403, _))));
    assert_eq!(version(&url).unwrap()["updating"], false);

    update().call().unwrap();
    let failed = until(&url, |v| !v["failed"].is_null());
    assert!(failed["failed"].as_str().unwrap().contains("EACCES") && failed["updating"] == false, "{failed}");
    assert_eq!(failed["started_ms"], before["started_ms"]);

    install.npm_works();
    update().call().unwrap();
    let after = until(&url, |v| v["started_ms"] != before["started_ms"]);
    let _ = daemon.kill();
    assert!(after["failed"].is_null(), "{after}");
    assert!(install.at("home/.claude/skills/jelly/SKILL.md").exists());
}

/// A jelly built from source is not updated for the person: the page and
/// `jelly update` get the commands, and the daemon refuses to run anything.
#[test]
fn a_source_build_is_told_the_commands_to_run() {
    let home = tempfile::tempdir().unwrap();
    let registry = common::registry("99.0.0");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["serve", "--port", "0", "--home"]).arg(home.path())
        .env("JELLY_UPDATE_URL", &registry).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let url = common::served_url(&mut daemon);
    let v = until(&url, |v| v["newer"] == true);
    let refused = ureq::post(&format!("{url}/api/update")).call();
    let jelly = |registry: &str| Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "update"]).env("JELLY_UPDATE_URL", registry).output().unwrap();
    let out = jelly(&registry);
    let current = jelly(&common::registry(VERSION));
    let _ = daemon.kill();

    assert_eq!(v["install"], "source");
    assert!(v["commands"][1].as_str().unwrap().starts_with("cargo install --path "), "{v}");
    assert!(matches!(refused, Err(ureq::Error::Status(409, _))), "{refused:?}");
    let said = text(&out);
    assert!(out.status.success() && said.contains("jelly 99.0.0 is out") && said.contains("  cargo install --path "), "{said}");
    assert_eq!(text(&current), format!("jelly {VERSION} is up to date (latest release: {VERSION}).\n"));
}

/// `jelly doctor` names what is behind with the command that fixes it, and
/// exits 0 only when nothing is.
#[test]
fn doctor_says_what_to_fix_and_how() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let (_refused, port) = common::refused_port();
    let jelly = |registry: &str, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &format!("http://127.0.0.1:{port}")]).args(args)
            .env("HOME", home.path()).env("JELLY_UPDATE_URL", registry).env("JELLY_CHROME", env!("CARGO_BIN_EXE_jelly")).output().unwrap()
    };
    let behind = jelly(&common::registry("99.0.0"), &["doctor"]);
    let said = text(&behind);
    assert!(!behind.status.success(), "{said}");
    assert!(said.contains(&format!("fix   jelly {VERSION}, and 99.0.0 is out: jelly update\n")), "{said}");
    assert!(said.contains("skills/jelly is missing: jelly setup\n") && said.contains("settings.json is missing: jelly setup\n"), "{said}");
    assert!(said.contains("ok    browser for `look` and `try`: "), "{said}");

    let current = common::registry(VERSION);
    assert!(jelly(&current, &["setup"]).status.success());
    let fine = jelly(&current, &["doctor"]);
    assert!(fine.status.success() && !text(&fine).contains("fix "), "{}", text(&fine));

    // A skill an older jelly wrote.
    std::fs::write(home.path().join(".claude/skills/jelly/SKILL.md"), "old").unwrap();
    let stale = jelly(&current, &["--json", "doctor"]);
    let checks: Value = serde_json::from_slice(&stale.stdout).unwrap();
    let to_fix: Vec<&Value> = checks.as_array().unwrap().iter().filter(|c| c["ok"] == false).collect();
    assert!(!stale.status.success() && to_fix.len() == 1 && to_fix[0]["fix"] == "jelly setup", "{checks}");
}

/// The Claude Code hook tells a session its skill is behind: once when jelly
/// was updated under it, and otherwise at most once a day.
#[test]
fn the_hook_says_once_that_the_skill_is_behind() {
    let home = tempfile::tempdir().unwrap();
    let at = |p: &str| home.path().join(p);
    std::fs::create_dir_all(at(".claude/skills/jelly")).unwrap();
    std::fs::write(at(".claude/skills/jelly/SKILL.md"), "old").unwrap();
    let (_refused, port) = common::refused_port();
    let hook = |session: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly"))
            .args(["--url", &format!("http://127.0.0.1:{port}"), "--project", "p", "hear", "--hook", "--session-id", session])
            .env("HOME", home.path()).env("JELLY_HOME", at("jellies")).output().unwrap();
        assert!(out.status.success());
        text(&out)
    };
    std::fs::create_dir_all(at("jellies")).unwrap();

    let first = hook("a");
    assert!(first.contains("Run `jelly setup`") && first.lines().count() == 1, "{first}");
    assert_eq!(hook("a"), "", "not again today");
    assert!(hook("b").contains("Run `jelly setup`"), "another session has not heard it");

    // The skill is current, and jelly was updated since session c read it.
    assert!(Command::new(env!("CARGO_BIN_EXE_jelly")).arg("setup").env("HOME", home.path()).output().unwrap().status.success());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    std::fs::write(at("jellies/hook-told.json"), json!({"c": {"version": "0.0.1", "at_ms": now}}).to_string()).unwrap();
    assert!(hook("c").contains(&format!("jelly was updated to {VERSION} during this session")));
    assert_eq!(hook("c"), "");
}
