//! `jelly pr`: what people decided in a jelly, as a comment on a pull request.
//! The comment is built from the jelly's asks, answers and comments, and
//! posted through the GitHub CLI, so jelly holds no GitHub login of its own.

use crate::store::{self, Ask, Jelly};
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

/// The most of the page outline a comment carries. GitHub refuses a comment
/// over 65536 characters.
const OUTLINE_LIMIT: usize = 20_000;

/// The hidden first line of a jelly's comment. `post` finds the earlier
/// comment by it, so posting again replaces that one.
pub fn marker(jelly: &str) -> String {
    format!("<!-- jelly {jelly} -->")
}

/// The comment, as GitHub markdown. `link` is the jelly's view-only share
/// link and `outline` the page as `jelly look` prints it, when there is one.
pub fn comment(j: &Jelly, link: Option<&str>, outline: Option<&str>) -> String {
    let mut out = vec![marker(&j.name), format!("### {}", if j.about.is_empty() { "Decided in a jelly".into() } else { plain(&j.about) })];
    let mut section = |title: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            out.push(format!("**{title}**\n{}", lines.join("\n")));
        }
    };
    section("Decided", j.asks.iter().filter_map(decided).collect());
    section("Not decided yet", j.asks.iter().filter(|a| a.is_open()).map(|a| {
        let options: Vec<String> = a.options.iter().map(|o| plain(&o.label)).collect();
        format!("- **{}** {}", plain(&a.title), options.join(" / ")).trim_end().to_string()
    }).collect());
    section("Comments", j.comments.iter().map(|c| {
        let place = match c.anchor["quote"].as_str().or(c.anchor["selector"].as_str()) {
            Some(part) => code(part),
            None => "the whole jelly".into(),
        };
        let by = person(&c.by).map(|p| format!(", by {p}")).unwrap_or_default();
        let mut lines = vec![format!("- On {place}{by}: {}{}", code(&c.text), if c.resolved { " (resolved)" } else { "" })];
        lines.extend(c.replies.iter().map(|r| format!("  - {}: {}", code(name(&r.by)), code(&r.text))));
        lines.join("\n")
    }).collect());
    if let Some(link) = link {
        out.push(format!("Watch it live (view only): {link}"));
    }
    if let Some(outline) = outline.filter(|o| !o.is_empty()) {
        let fence = "`".repeat(ticks(outline).max(2) + 1);
        out.push(format!("<details><summary>The page as text</summary>\n\n{fence}text\n{}\n{fence}\n\n</details>", clipped(outline)));
    }
    out.push(format!("<sub>From the jelly {} at version {}.</sub>", code(&j.name), j.version));
    out.join("\n\n") + "\n"
}

/// One answered ask: the option picked, who picked it, what it was picked
/// over, and their words.
fn decided(ask: &Ask) -> Option<String> {
    let answer = ask.answer.as_ref()?;
    let picked = answer.option.as_deref().filter(|o| !o.is_empty());
    let label = |id: &str| ask.options.iter().find(|o| o.id == id).map_or_else(|| plain(id), |o| plain(&o.label));
    let others: Vec<String> = ask.options.iter().filter(|o| picked.is_some_and(|p| p != o.id)).map(|o| plain(&o.label)).collect();
    let words = code(&answer.text);
    let mut parts: Vec<String> = Vec::new();
    parts.extend(picked.map(label));
    parts.extend(person(&answer.by).map(|p| format!("by {p}")));
    if !others.is_empty() {
        parts.push(format!("over {}", others.join(", ")));
    }
    if !words.is_empty() {
        parts.push(words);
    }
    if parts.is_empty() {
        parts.push("seen".into());
    }
    Some(format!("- **{}** {}", plain(&ask.title), parts.join(" · ")))
}

/// Who did something, for a public page: the name a person gave, or the part
/// of their email before the `@`. A person without a name is nobody to name.
fn name(by: &str) -> &str {
    let name = store::viewer(by).unwrap_or(by);
    name.split_once('@').map(|(local, _)| local).filter(|local| !local.is_empty()).unwrap_or(name)
}

fn person(by: &str) -> Option<String> {
    (by != store::PERSON).then(|| code(name(by)))
}

/// Text an agent wrote (a question, a label, the about line) on one line,
/// with nothing in it read as HTML.
fn plain(text: &str) -> String {
    one_line(text).replace('&', "&amp;").replace('<', "&lt;")
}

/// What someone wrote in a jelly, as a code span. GitHub reads nothing inside
/// one: no markdown, no HTML, and an `@name` in it notifies nobody.
fn code(text: &str) -> String {
    let text = one_line(text);
    if text.is_empty() {
        return text;
    }
    let fence = "`".repeat(ticks(&text) + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') { " " } else { "" };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The longest run of backticks in `text`: a code fence has to be longer.
fn ticks(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

/// The outline, cut at a line end once it passes `OUTLINE_LIMIT`.
fn clipped(outline: &str) -> String {
    if outline.len() <= OUTLINE_LIMIT {
        return outline.into();
    }
    let mut kept = 0;
    let lines: Vec<&str> = outline.lines().take_while(|l| { kept += l.len() + 1; kept <= OUTLINE_LIMIT }).collect();
    format!("{}\n… {} more lines", lines.join("\n"), outline.lines().count() - lines.len())
}

/// A pull request as `gh pr view --json url,comments` prints it.
#[derive(Deserialize)]
struct Pull {
    url: String,
    comments: Vec<Posted>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Posted {
    id: String,
    url: String,
    body: String,
    viewer_did_author: bool,
}

const UPDATE: &str = "mutation($id: ID!, $body: String!) { updateIssueComment(input: {id: $id, body: $body}) { issueComment { url } } }";

/// Posts `body` on the pull request `pull` (a number, a URL or a branch; the
/// current branch's when None), or replaces the comment this GitHub user
/// posted for the same jelly before. With `dry_run` it only finds out which.
/// Returns what it did, in a few words.
pub fn post(body: &str, pull: Option<&str>, marker: &str, dry_run: bool) -> Result<String, String> {
    let mut view = vec!["pr", "view"];
    view.extend(pull);
    view.extend(["--json", "url,comments"]);
    let found = gh(&view, None)?;
    let pull: Pull = serde_json::from_str(&found).map_err(|e| format!("`gh pr view` printed something else than a pull request: {e}"))?;
    let earlier = pull.comments.iter().rev().find(|c| c.viewer_did_author && c.body.contains(marker));
    match (earlier, dry_run) {
        (Some(c), true) => Ok(format!("not posted. It would replace your earlier comment {}", c.url)),
        (None, true) => Ok(format!("not posted. It would be a new comment on {}", pull.url)),
        (Some(c), false) => {
            gh(&["api", "graphql", "-f", &format!("query={UPDATE}"), "-f", &format!("id={}", c.id), "-F", "body=@-"], Some(body))?;
            Ok(format!("updated {}", c.url))
        }
        (None, false) => Ok(format!("posted {}", gh(&["pr", "comment", &pull.url, "--body-file", "-"], Some(body))?.trim())),
    }
}

/// Runs the GitHub CLI and returns what it printed. `JELLY_GH` names another
/// binary, as `JELLY_NPM` does for npm.
fn gh(args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let gh = std::env::var("JELLY_GH").unwrap_or_else(|_| "gh".into());
    let mut child = Command::new(&gh).args(args)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("`jelly pr` posts through the GitHub CLI, and `{gh}` is not installed. Install it from https://cli.github.com, then run `gh auth login`"),
            _ => format!("could not run {gh}: {e}"),
        })?;
    let pipe = child.stdin.take();
    // The body is written beside the wait, so a full pipe on either side stalls nothing.
    let out = std::thread::scope(|scope| {
        scope.spawn(move || {
            if let (Some(mut pipe), Some(text)) = (pipe, stdin) {
                let _ = pipe.write_all(text.as_bytes());
            }
        });
        child.wait_with_output()
    }).map_err(|e| format!("could not run {gh}: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    // gh's own exit code for "this needs a login".
    if out.status.code() == Some(4) {
        return Err("gh is not logged in to GitHub. Run `gh auth login`, then this again".into());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(format!("`gh {}` failed: {}", args[..2].join(" "), if said.trim().is_empty() { "it said nothing" } else { said.trim() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jelly(extra: serde_json::Value) -> Jelly {
        let mut j = json!({"name": "shop:checkout", "view": "", "verbs": "", "state": {}, "version": 4, "view_rev": 1,
            "kept": false, "made_by": "agent", "updated_ms": 1, "about": "Which checkout button ships"});
        j.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(j).unwrap()
    }

    #[test]
    fn the_comment_says_who_picked_what_and_what_is_still_open() {
        let ask = |id: &str, title: &str, options: serde_json::Value, answer: serde_json::Value| json!({"id": id, "title": title, "by": "agent", "at_ms": 1,
            "seen": options.as_array().unwrap().is_empty(), "options": options, "answer": answer});
        let hero = json!([{"id": "bold", "label": "Bold gradient"}, {"id": "quiet", "label": "Quiet white"}]);
        let j = jelly(json!({
            "asks": [
                ask("a1", "Which hero?", hero.clone(), json!({"option": "quiet", "text": "keep the button blue", "by": "person:ana@example.com", "at_ms": 2})),
                ask("a2", "Check the footer <a> links", json!([]), json!({"option": null, "text": "", "by": "person", "at_ms": 2})),
                ask("a3", "What should it say?", json!([]), json!({"option": null, "text": "Pay now", "by": "person:ben", "at_ms": 2})),
                ask("a4", "Ship dark mode too?", json!([{"id": "yes", "label": "Yes"}, {"id": "no", "label": "No"}]), json!(null)),
                {"id": "a5", "title": "Taken back", "by": "agent", "at_ms": 1, "unasked": {"by": "agent", "at_ms": 2}},
            ],
            "comments": [
                {"id": "c1", "anchor": {"selector": "#pay", "quote": "Pay now"}, "text": "Make it bigger\non phones", "by": "person", "at_ms": 1, "resolved": true,
                    "replies": [{"text": "Now 48px tall.", "by": "agent", "at_ms": 2}]},
                {"id": "c2", "anchor": null, "text": "Is this behind a flag?", "by": "person:ben@example.com", "at_ms": 1, "resolved": false},
            ],
        }));
        assert_eq!(comment(&j, Some("https://relay.example/s/abc/def"), Some("- heading \"Checkout\" [h1]")), "\
<!-- jelly shop:checkout -->

### Which checkout button ships

**Decided**
- **Which hero?** Quiet white · by `ana` · over Bold gradient · `keep the button blue`
- **Check the footer &lt;a> links** seen
- **What should it say?** by `ben` · `Pay now`

**Not decided yet**
- **Ship dark mode too?** Yes / No

**Comments**
- On `Pay now`: `Make it bigger on phones` (resolved)
  - `agent`: `Now 48px tall.`
- On the whole jelly, by `ben`: `Is this behind a flag?`

Watch it live (view only): https://relay.example/s/abc/def

<details><summary>The page as text</summary>

```text
- heading \"Checkout\" [h1]
```

</details>

<sub>From the jelly `shop:checkout` at version 4.</sub>
");
    }

    /// GitHub notifies the owner of every `@name` outside a code span, and a
    /// person typing in a jelly did not ask for that.
    #[test]
    fn what_people_wrote_stays_inside_code() {
        let j = jelly(json!({"comments": [
            {"id": "c1", "anchor": null, "text": "ask @octocat, see `main` </details>", "by": "person:@octocat", "at_ms": 1, "resolved": false},
        ]}));
        let body = comment(&j, None, Some("- text \"```\""));
        assert!(body.contains("- On the whole jelly, by `@octocat`: ``ask @octocat, see `main` </details>``\n"), "{body}");
        assert!(body.contains("\n````text\n- text \"```\"\n````\n"), "{body}");
    }

    #[test]
    fn a_long_outline_is_cut_at_a_line() {
        let outline = "- text \"a line of the page\"\n".repeat(2000);
        let kept = clipped(&outline);
        assert!(kept.len() < OUTLINE_LIMIT + 100, "{}", kept.len());
        let said = kept.lines().last().unwrap();
        let more: usize = said.split(' ').nth(1).unwrap().parse().unwrap();
        assert_eq!(kept.lines().count() - 1 + more, 2000, "{said}");
    }
}
