#![cfg(unix)]

mod common;

use serde_json::json;
use std::process::{Command, Stdio};

/// `jelly look --text --problems` on a jelly with this view, at this width.
fn problems(view: &str, width: Option<u32>) -> String {
    let home = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly"))
        .args(["serve", "--port", "0", "--home"])
        .arg(home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = common::served_url(&mut daemon);
    ureq::post(&format!("{url}/api/jellies"))
        .send_json(json!({"name": "t:p", "view": view, "verbs": "const verbs = {};", "state": {}, "by": "test"}))
        .unwrap();
    let mut args = vec!["--url".to_string(), url, "look".into(), "t:p".into(), "--text".into(), "--problems".into()];
    if let Some(w) = width { args.extend(["--width".into(), w.to_string()]); }
    let out = Command::new(env!("CARGO_BIN_EXE_jelly")).args(&args).output().unwrap();
    let _ = daemon.kill();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn assert_clean(view: &str, width: Option<u32>) {
    let out = problems(view, width);
    assert_eq!(out, "no problems\n", "a page without problems got:\n{out}");
}

/// The bug from a real session: cards stay white in dark mode while their text
/// turns white. Only a render with the dark color scheme can see it.
#[test]
fn white_on_white_in_dark_mode_is_caught() {
    let page = |dark_card: &str| format!(r#"<style>
        :root {{ color-scheme: light dark; }}
        body {{ background: #f4f4f4; color: #222; }}
        .card {{ background: #fff; padding: 12px; }}
        @media (prefers-color-scheme: dark) {{ body {{ background: #111; color: #eee; }} .card {{ color: #fff; {dark_card} }} }}
        </style><div class="card"><p id="total">Total due: 42 kr</p></div>"#);
    let out = problems(&page(""), None);
    assert!(out.contains(r##"!        unreadable text in dark mode: #ffffff on #ffffff is 1.0:1 at #total "Total due: 42 kr""##), "{out}");
    assert!(!out.contains("light mode"), "{out}");
    assert_clean(&page("background: #222;"), None);
}

/// Low contrast in both schemes is one line. Large text needs only 3:1, and text
/// apps hide on purpose (screen-reader only, aria-hidden) is not checked.
#[test]
fn faint_text_is_caught_in_both_schemes() {
    let out = problems(r#"<style>.note { color: #ccc; }</style><p class="note">Faint note</p>"#, None);
    assert!(out.contains(r##"!        unreadable text: #cccccc on #ffffff is 1.6:1 at p.note "Faint note""##), "{out}");
    assert_clean(r#"<style>
        .big { color: #888; font-size: 28px; }
        .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); color: #fff; }
        </style>
        <p>Plain text</p><p class="big">Big grey heading text</p>
        <span class="sr-only">For screen readers</span>
        <div aria-hidden="true" style="color: #fafafa">decoration</div>
        <span title="3 of 5">●●●<span style="opacity: .28">●●</span></span>"#, None);
    // A word among the symbols is still text.
    let out = problems(r#"<span style="color: #ddd">●● Low</span>"#, None);
    assert!(out.contains("unreadable text"), "{out}");
}

#[test]
fn content_wider_than_the_screen_is_caught() {
    let out = problems(r#"<div class="wide" style="width: 1600px; height: 20px"></div>"#, None);
    assert!(out.contains("!        wider than the screen (1608px at 1280px): div.wide reaches 1608px"), "{out}");
    // Inside a box that scrolls sideways, wide content is fine.
    assert_clean(r#"<div style="overflow-x: auto"><div style="width: 1600px; height: 20px"></div></div>"#, None);
}

#[test]
fn controls_off_the_screen_are_caught() {
    let out = problems(r#"<button id="away" style="position: absolute; left: -300px">Away</button>"#, None);
    assert!(out.contains(r#"!        off the screen: button "Away" #away"#), "{out}");
    assert_clean(r#"<div style="overflow-x: auto; white-space: nowrap"><span style="display: inline-block; width: 1400px"></span><button id="later">Later</button></div>"#, None);
}

/// A control under a fixed bar is fine when scrolling brings it out; a honeypot
/// under a cover is left out of the tab order on purpose.
#[test]
fn covered_controls_are_caught() {
    let out = problems(r#"<div style="position: relative"><button id="hid">Hidden</button>
        <div id="cover" style="position: absolute; inset: 0; background: #fff"></div></div>"#, None);
    assert!(out.contains(r#"!        covered by another element: button "Hidden" #hid under #cover"#), "{out}");
    // The bar is tall so the button is under it whatever the window's inner height.
    assert_clean(r#"<div style="height: 760px"></div><button id="low">Low</button><div style="height: 600px"></div>
        <div style="position: fixed; left: 0; right: 0; bottom: 0; height: 200px; background: #eee"></div>
        <div aria-hidden="true" style="position: relative"><input id="hp" tabindex="-1">
        <div style="position: absolute; inset: 0; background: #fff"></div></div>"#, None);
}

/// At phone width, a tap target under 24px is a problem unless it is spaced out
/// (WCAG 2.5.8) or a labelled checkbox, which is tapped through its label.
#[test]
fn small_tap_targets_are_caught_at_phone_width() {
    let tiny = "width: 16px; height: 16px; padding: 0; margin: 0";
    let out = problems(&format!(r#"<div style="display: flex"><button id="a" style="{tiny}">a</button><button id="b" style="{tiny}">b</button></div>"#), Some(390));
    assert!(out.contains(r#"!        too small to tap (under 24px): button "a" #a 16×16"#), "{out}");
    assert_clean(&format!(r#"<label><input type="checkbox" style="{tiny}"> Remember me</label><br>
        <label><input type="checkbox" style="{tiny}"> Send me news</label>
        <div style="margin-top: 40px"><button id="lone" style="{tiny}">x</button></div>"#), Some(390));
}

#[test]
fn failed_requests_are_caught() {
    let out = problems(r#"<p>Data</p><script>fetch("assets/missing.json").catch(() => {});</script>"#, None);
    assert!(out.contains("!        failed request: 404 /j/t:p/assets/missing.json"), "{out}");
    assert_clean(r#"<p>Data</p><script>fetch("/runtime.js");</script>"#, None);
}

const PIXEL: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='40' height='40'/%3E";

#[test]
fn images_without_alt_are_caught() {
    let out = problems(&format!(r#"<img id="pic" src="{PIXEL}" width="40" height="40">"#), None);
    assert!(out.contains("!        image without alt: #pic"), "{out}");
    assert_clean(&format!(r#"<img src="{PIXEL}" alt="A dot" width="40" height="40"><img src="{PIXEL}" alt="" width="40" height="40">
        <img src="{PIXEL}" role="presentation" width="40" height="40">"#), None);
}

/// A honeypot field (aria-hidden, out of the tab order) has no label on purpose.
#[test]
fn fields_without_labels_are_caught() {
    let out = problems(r#"<input id="q">"#, None);
    assert!(out.contains(r#"!        field without a label: input "" #q"#), "{out}");
    assert_clean(r#"<label for="n">Name</label> <input id="n"> <input aria-label="Search"> <input placeholder="Add a task">
        <div aria-hidden="true"><input name="website" tabindex="-1"></div>"#, None);
}

/// Text cut off by its box is a problem; an ellipsis is truncation by design.
#[test]
fn clipped_text_is_caught() {
    let clip = "width: 60px; overflow: hidden; white-space: nowrap";
    let out = problems(&format!(r#"<div class="name" style="{clip}">This text is much too long for its box</div>"#), None);
    assert!(out.contains(r#"!        text cut off: div.name "This text is much too long for""#), "{out}");
    assert_clean(&format!(r#"<div style="{clip}; text-overflow: ellipsis">This text is much too long for its box</div>"#), None);
}

/// A link scrolled out of view inside a sideways tab bar is reachable by
/// scrolling the bar, so it is not "covered" by the button beside the bar.
/// A control really under another element still is.
#[test]
fn links_scrolled_away_in_a_tab_bar_are_not_covered() {
    let tabs: String = (0..20).map(|i| format!(r#"<a href="/p/{i}" style="padding: 4px 10px">project-{i}</a>"#)).collect();
    assert_clean(&format!(r#"<header style="display: flex; gap: 8px; align-items: center">
        <nav style="display: flex; overflow-x: auto; min-width: 0">{tabs}</nav>
        <button style="margin-left: auto; white-space: nowrap">Set your name</button></header>"#), None);
    let out = problems(r#"<button id="under" style="position: absolute; top: 40px; left: 40px; width: 80px; height: 40px">Hidden</button>
        <div style="position: absolute; top: 30px; left: 30px; width: 120px; height: 60px; background: #eee">On top</div>"#, None);
    assert!(out.contains("covered by another element"), "{out}");
}

/// The bug from a real session: an agent with an old copy of the skill wrote
/// `jelly.call`, which loads fine and throws on every click.
#[test]
fn a_view_using_a_jelly_function_that_does_not_exist_is_caught() {
    let out = problems(r#"<button id="add" onclick="jelly.person.sent()">Add</button>
        <script>document.getElementById("add").onclick = () => jelly.call("add", {});</script>"#, None);
    assert!(out.contains("fails on a click: jelly.call (it has name, about, css, state, onState, do,"), "{out}");
    assert!(out.contains("fails on a click: jelly.person.sent (jelly.person has"), "{out}");
    assert_clean(r#"<button id="add">Add</button>
        <script>const logo = "jelly.png", doc = "see jelly.app/docs";
        document.getElementById("add").onclick = () => jelly.do("add", { n: jelly.state.items.length });
        jelly.onState(() => {}); jelly.person.onAsk(() => {});</script>"#, None);
}
