//! `jelly doctor`: what on this machine is out of date or missing, one line per
//! check, each failing line with the command that fixes it.

use crate::setup::{self, State};
use crate::update::{self, VERSION};
use crate::{server, startup};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

#[derive(Serialize)]
pub struct Check {
    /// False needs fixing. None could not be told, which is not a failure.
    pub ok: Option<bool>,
    pub text: String,
    /// What fixes a check that is not ok.
    pub fix: Option<String>,
}

impl Check {
    fn ok(text: String) -> Check { Check { ok: Some(true), text, fix: None } }
    fn fix(text: String, fix: &str) -> Check { Check { ok: Some(false), text, fix: Some(fix.into()) } }
    fn unknown(text: String) -> Check { Check { ok: None, text, fix: None } }

    pub fn line(&self) -> String {
        match (self.ok, &self.fix) {
            (Some(true), _) => format!("ok    {}", self.text),
            (Some(false), Some(fix)) => format!("fix   {}: {fix}", self.text),
            (Some(false), None) => format!("fix   {}", self.text),
            (None, _) => format!("--    {}", self.text),
        }
    }
}

/// Checks this jelly, the daemon at `url`, the browser, and what `setup` keeps
/// for each agent in `home`.
pub fn run(home: &Path, url: &str) -> Vec<Check> {
    let mut checks = vec![release(), daemon(url)];
    checks.push(match server::browser() {
        Some(browser) => Check::ok(format!("browser for `look` and `try`: {browser}")),
        None => Check::fix("no browser for `look` and `try`".into(), "install Chrome, Chromium, Edge or Brave, or set JELLY_CHROME to a browser binary"),
    });
    let items = setup::items(home);
    if items.is_empty() {
        checks.push(Check::unknown(format!("no agent found: {}", setup::NO_AGENT)));
    }
    checks.extend(items.iter().map(|item| {
        let name = format!("{} {} {}", item.agent, item.what, item.path.display());
        match item.state() {
            State::Current => Check::ok(name),
            State::Link(target) => Check::ok(format!("{name} (a link to {})", target.display())),
            State::Missing => Check::fix(format!("{name} is missing"), "jelly setup"),
            State::Stale => Check::fix(format!("{name} is not the one this jelly comes with"), "jelly setup"),
        }
    }));
    checks
}

fn release() -> Check {
    match update::latest() {
        Ok(latest) if update::newer(&latest, VERSION) => Check::fix(format!("jelly {VERSION}, and {latest} is out"), "jelly update"),
        Ok(latest) => Check::ok(format!("jelly {VERSION} (latest release: {latest})")),
        Err(e) => Check::unknown(format!("jelly {VERSION}; {e}")),
    }
}

fn daemon(url: &str) -> Check {
    match ureq::get(&format!("{url}/api/version")).timeout(Duration::from_secs(2)).call() {
        Ok(response) => match response.into_json::<Value>() {
            Ok(daemon) if update::daemon_differs(&daemon) => Check::fix(
                format!("daemon at {url} runs another build ({}) than this jelly ({VERSION})", daemon["version"].as_str().unwrap_or("?")), "jelly update"),
            Ok(_) => Check::ok(format!("daemon at {url}, the same build as this jelly")),
            Err(e) => Check::fix(format!("daemon at {url} did not say its version ({e})"), "run `jelly serve` for details"),
        },
        Err(ureq::Error::Transport(t)) if t.kind() == ureq::ErrorKind::ConnectionFailed => {
            if startup::is_default_endpoint(url) {
                Check::ok(format!("no daemon at {url}: the next jelly command starts it"))
            } else {
                Check::unknown(format!("no daemon at {url}"))
            }
        }
        Err(e) => Check::fix(format!("daemon at {url} does not answer ({e})"), "run `jelly serve` for details"),
    }
}
