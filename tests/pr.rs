//! `jelly pr`: the comment goes to the pull request through `gh`. A stub
//! stands in for `gh` here, so nothing reaches GitHub.
#![cfg(unix)]

mod common;

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

const PULL: &str = "https://github.com/o/r/pull/7";

/// A daemon with one jelly the person answered and commented on, and a stub
/// `gh`. The stub writes down how it was called (`gh-ran`) and the comment it
/// was handed (`body`), and answers `gh pr view` with `view.json`.
struct Room {
    dir: tempfile::TempDir,
    daemon: Child,
    url: String,
}

impl Drop for Room {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

impl Room {
    fn new() -> Room {
        let dir = tempfile::tempdir().unwrap();
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_jelly")).args(["serve", "--port", "0", "--home"]).arg(dir.path().join("home"))
            .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let url = common::served_url(&mut daemon);
        let room = Room { dir, daemon, url };
        let post = |path: &str, body: Value| { ureq::post(&format!("{}/api/jellies{path}", room.url)).send_json(body).unwrap(); };
        post("", json!({"name": "t:hero", "view": "<h1 id=title>Checkout</h1>", "verbs": "const verbs = {};", "state": {}, "by": "test", "about": "Which hero ships"}));
        post("/t:hero/asks", json!({"title": "Which hero?", "options": [{"id": "bold", "label": "Bold gradient"}, {"id": "quiet", "label": "Quiet white"}], "by": "agent"}));
        post("/t:hero/asks/a1/answer", json!({"option": "quiet", "text": "keep the button blue", "by": "person:ana@example.com"}));
        post("/t:hero/comment", json!({"text": "Bigger on phones", "anchor": {"selector": "#title", "quote": "Checkout"}, "by": "person"}));
        room.seen(json!([]));
        room.gh("case \"$1 $2\" in\n\"pr view\") cat \"$dir/view.json\";;\n\"pr comment\") cat > \"$dir/body\"; echo https://github.com/o/r/pull/7#issuecomment-1;;\n\"api graphql\") cat > \"$dir/body\"; echo '{}';;\nesac");
        room
    }

    fn at(&self, file: &str) -> PathBuf {
        self.dir.path().join(file)
    }

    /// The comments `gh pr view` finds on the pull request.
    fn seen(&self, comments: Value) {
        std::fs::write(self.at("view.json"), json!({"url": PULL, "comments": comments}).to_string()).unwrap();
    }

    fn gh(&self, does: &str) {
        let dir = self.dir.path().display();
        std::fs::write(self.at("gh"), format!("#!/bin/sh\ndir='{dir}'\necho \"$@\" >> \"$dir/gh-ran\"\n{does}\n")).unwrap();
        std::fs::set_permissions(self.at("gh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn pr(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_jelly")).args(["--url", &self.url, "pr", "t:hero"]).args(args).env("JELLY_GH", self.at("gh")).output().unwrap()
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.at(file)).unwrap_or_default()
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn the_decision_is_posted_on_the_pull_request() {
    let room = Room::new();
    let out = room.pr(&["12"]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "t:hero: posted https://github.com/o/r/pull/7#issuecomment-1\n");
    assert_eq!(room.read("gh-ran"), format!("pr view 12 --json url,comments\npr comment {PULL} --body-file -\n"));
    let body = room.read("body");
    assert!(body.starts_with("<!-- jelly t:hero -->\n\n### Which hero ships\n"), "{body}");
    assert!(body.contains("- **Which hero?** Quiet white · by `ana` · over Bold gradient · `keep the button blue`\n"), "{body}");
    assert!(body.contains("- On `Checkout`: `Bigger on phones`\n"), "{body}");
    // The page as the browser drew it.
    assert!(body.contains("- heading \"Checkout\" [h1] #title\n"), "{body}");
}

#[test]
fn posting_again_replaces_your_earlier_comment() {
    let room = Room::new();
    let comment = |id: &str, body: &str, mine: bool| json!({"id": id, "url": format!("{PULL}#{id}"), "body": body, "viewerDidAuthor": mine});
    // Someone else's comment about the same jelly is not yours to change.
    room.seen(json!([comment("IC_other", "unrelated", true), comment("IC_mine", "<!-- jelly t:hero -->\n\nold", true), comment("IC_theirs", "<!-- jelly t:hero -->\n\ntheirs", false)]));
    let out = room.pr(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("t:hero: updated {PULL}#IC_mine\n"));
    let ran = room.read("gh-ran");
    assert!(ran.starts_with("pr view --json url,comments\napi graphql -f query=mutation("), "{ran}");
    assert!(ran.ends_with(" -f id=IC_mine -F body=@-\n"), "{ran}");
    assert!(room.read("body").contains("- **Which hero?** Quiet white"));
}

#[test]
fn a_dry_run_prints_the_comment_and_posts_nothing() {
    let room = Room::new();
    let out = room.pr(&["--dry-run"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("- **Which hero?** Quiet white"), "{}", text(&out));
    assert!(String::from_utf8_lossy(&out.stderr).contains(&format!("not posted. It would be a new comment on {PULL}")), "{}", text(&out));
    assert_eq!(room.read("gh-ran"), "pr view --json url,comments\n");
}

#[test]
fn a_gh_that_cannot_post_is_said_with_the_fix() {
    let room = Room::new();
    room.gh("echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4");
    let out = room.pr(&[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("gh is not logged in to GitHub. Run `gh auth login`"), "{}", text(&out));

    room.gh("echo 'no pull requests found for branch \"main\"' >&2; exit 1");
    let out = room.pr(&[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("`gh pr view` failed: no pull requests found for branch \"main\""), "{}", text(&out));

    std::fs::remove_file(room.at("gh")).unwrap();
    let out = room.pr(&[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("is not installed. Install it from https://cli.github.com, then run `gh auth login`"), "{}", text(&out));
}
