#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::process::{Command, Stdio};

/// `jelly try` does the steps in the page as a person would and shows the result,
/// but the jelly itself is left exactly as it was: same state, same version, no
/// new events, and `jelly look` still shows the real page.
#[test]
fn try_runs_the_steps_and_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    let view = r#"<ul id="items"></ul>
        <form id="f" hidden><input id="title" aria-label="Title"><button id="save">Save</button></form>
        <button id="add">Add</button>
        <button id="sign" onclick="jelly.person.name = 'Ana'; jelly.do('sign').catch(() => {})">Sign</button>
        <script>
        add.onclick = () => { f.hidden = false; };
        f.onsubmit = (e) => { e.preventDefault(); jelly.do("add", { title: title.value }).catch(() => {}); };
        jelly.onState((s) => { items.innerHTML = s.items.map((i) => `<li>${i}</li>`).join(""); });
        </script>"#;
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:x", "view": view, "verbs": "const verbs = { add(s, { title }) { s.items.push(title); }, sign(s, a, { by }) { s.signed = by; } };", "state": {"items": ["Eggs"]}, "by": "test"}))
        .unwrap();
    let jelly = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url]).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    let before: Value = ureq::get(&format!("{url}/api/jellies/t:x")).call().unwrap().into_json().unwrap();
    let log = || std::fs::read(home.path().join("events.jsonl")).unwrap();
    let log_before = log();

    let tried = jelly(&["try", "t:x", "--click", "#add", "--type", "#title=Buy milk", "--click", "#save"]);
    // No verb runs here, so the page stays at the real version: if it reported its
    // screen like a normal render, `look` would show the typed draft.
    let drafted = jelly(&["try", "t:x", "--click", "#add", "--type", "#title=Draft"]);
    // A page that names its viewer calls verbs as that person, as it would for real.
    let signed = jelly(&["try", "t:x", "--click", "#sign"]);
    let seen = jelly(&["look", "t:x", "--text"]);
    let after: Value = ureq::get(&format!("{url}/api/jellies/t:x")).call().unwrap().into_json().unwrap();
    let log_after = log();
    let _ = daemon.kill();

    assert!(tried.contains(r#"call     add {"title":"Buy milk"}"#), "{tried}");
    assert!(tried.contains(r#"text "Buy milk""#), "the page shows the added item: {tried}");
    assert_eq!((&after["state"], &after["version"]), (&before["state"], &before["version"]), "{tried}");
    assert_eq!(log_after, log_before, "no events recorded");
    assert!(signed.contains(r#"/signed = "person:Ana""#), "{signed}");
    assert!(drafted.contains("call     none: no verb ran") && drafted.contains(r#"= "Draft""#), "{drafted}");
    assert!(!seen.contains("Buy milk") && !seen.contains("Draft"), "look shows the real jelly, not a trial: {seen}");
}

/// Nobody is at a `jelly try` render to close a dialog, so the page must not wait
/// on one: the dialog is reported and the steps after it still run.
#[test]
fn try_reports_dialogs_and_goes_on() {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    let view = r#"<p id="said"></p><button id="add">Add</button><button id="clear">Clear</button>
        <script>
        add.onclick = () => jelly.do("add", {}).catch((e) => alert(e.message));
        clear.onclick = () => { if (confirm("Clear all?")) said.textContent = "cleared"; };
        </script>"#;
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:x", "view": view, "verbs": "const verbs = { add() { throw new Error('a tea needs a name'); } };", "state": {}, "by": "test"}))
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &url, "try", "t:x", "--click", "#add", "--click", "#clear"]).output().unwrap();
    let _ = daemon.kill();
    let tried = String::from_utf8_lossy(&out.stdout);

    assert!(tried.contains(r#"dialog   alert "a tea needs a name""#), "{tried}");
    assert!(tried.contains(r#"dialog   confirm "Clear all?", answered OK"#), "{tried}");
    assert!(tried.contains(r#"text "cleared""#), "the step after the alert ran: {tried}");
}
