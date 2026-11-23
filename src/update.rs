//! Knowing when a newer jelly is out, and installing it. The daemon, `jelly
//! update`, `jelly doctor` and `jelly hear --hook` all work from here.

use crate::setup;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REGISTRY: &str = "https://registry.npmjs.org/jellyware/latest";

/// The newest released version, from npm. `JELLY_UPDATE_URL` names another
/// address to ask, or `off` to never ask.
pub fn latest() -> Result<String, String> {
    #[derive(Deserialize)]
    struct Release { version: String }
    let url = std::env::var("JELLY_UPDATE_URL").unwrap_or_else(|_| REGISTRY.into());
    if url == "off" {
        return Err("the check for a newer jelly is off (JELLY_UPDATE_URL=off)".into());
    }
    let failed = |e: String| format!("could not ask {url} for the latest jelly: {e}");
    let release: Release = ureq::get(&url).set("User-Agent", concat!("jelly/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10)).call().map_err(|e| failed(e.to_string()))?
        .into_json().map_err(|e| failed(e.to_string()))?;
    Ok(release.version)
}

/// Whether `latest` is a later release than `current`. A version that is not
/// three numbers (a pre-release) is never an update.
pub fn newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| -> Option<Vec<u64>> {
        let parts: Vec<u64> = v.split('.').map(|n| n.parse().ok()).collect::<Option<_>>()?;
        (parts.len() == 3).then_some(parts)
    };
    matches!((parts(latest), parts(current)), (Some(latest), Some(current)) if latest > current)
}

/// How this jelly got onto the machine, which decides how it is updated.
#[derive(PartialEq, Debug)]
pub enum Install {
    /// `npm install -g jellyware`: the binary sits in its platform package.
    Npm,
    /// Built with cargo from a checkout.
    Source,
}

impl Install {
    /// npm puts the binary in `node_modules/@getjellyware/jelly-<os>-<cpu>/bin`,
    /// and nothing else does.
    pub fn of(exe: &Path) -> Install {
        let parts: Vec<_> = exe.components().map(|c| c.as_os_str()).collect();
        if parts.windows(2).any(|w| w[0] == "node_modules" && w[1] == "@getjellyware") { Install::Npm } else { Install::Source }
    }
}

/// The commands that update a jelly built from source. They name the checkout
/// it was built in while that is still there.
pub fn source_commands() -> [String; 2] {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"));
    if checkout.join(".git").exists() {
        [format!("git -C {} pull", checkout.display()), format!("cargo install --path {}", checkout.display())]
    } else {
        ["git pull    (in your jelly checkout)".into(), "cargo install --path .".into()]
    }
}

/// Installs the latest release over the npm install `exe` belongs to, then has
/// the new binary write every agent's skill again. Returns what `setup` said.
pub fn apply(exe: &Path, latest: &str) -> Result<String, String> {
    // JELLY_NPM names another binary, as JELLY_CHROME does for the browser.
    let npm = std::env::var("JELLY_NPM").unwrap_or_else(|_| "npm".into());
    let run = |command: &mut Command| command.stdin(Stdio::null()).output();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    let installed = run(Command::new(&npm).args(["install", "-g", "jellyware@latest"])).map_err(|e| format!("could not run {npm}: {e}"))?;
    if !installed.status.success() {
        let said = text(&installed.stderr);
        return Err(format!("`{npm} install -g jellyware@latest` failed: {}", said.lines().last().unwrap_or("it said nothing")));
    }
    // npm can install into another place than this jelly came from (a second
    // Node version), and then nothing here changed.
    let now = run(Command::new(exe).arg("--version")).map(|out| text(&out.stdout)).unwrap_or_default();
    if now != format!("jelly {latest}") {
        return Err(format!("`{npm} install -g jellyware@latest` ran, but {} is still {}: that npm installs somewhere else. Run the npm that installed this jelly",
            exe.display(), if now.is_empty() { "not running" } else { &now }));
    }
    let setup = run(Command::new(exe).arg("setup")).map_err(|e| format!("could not run {} setup: {e}", exe.display()))?;
    // No agent to set up is said, and is no reason to fail the update.
    Ok([text(&setup.stdout), text(&setup.stderr)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n"))
}

/// Whether the daemon that answered `/api/version` with `daemon` runs another
/// build than this binary: another version, or one started before this binary
/// was installed.
pub fn daemon_differs(daemon: &Value) -> bool {
    let installed_ms = std::env::current_exe().and_then(std::fs::metadata).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);
    daemon["version"] != VERSION || daemon["started_ms"].as_u64().zip(installed_ms).is_some_and(|(started, installed)| started < installed)
}

const DAY_MS: u64 = 24 * 3600 * 1000;

/// What `jelly hear --hook` last told a session, kept in `hook-told.json` in the home.
#[derive(Serialize, Deserialize)]
struct Told {
    /// The jelly whose skill the session is taken to have read.
    version: String,
    /// When this session was last checked.
    at_ms: u64,
}

/// One line for a Claude Code session whose jelly skill is behind, or None.
/// It speaks once when jelly was updated under a running session, and
/// otherwise checks at most once a day per session: the skill folder against
/// this binary, then the daemon at `url` for a newer release.
pub fn hook_notice(url: &str, session: &str, user_home: &Path, jelly_home: &Path, now_ms: u64) -> Option<String> {
    let path = jelly_home.join("hook-told.json");
    let mut told: std::collections::HashMap<String, Told> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let seen = told.get(session);
    let updated = seen.is_some_and(|t| t.version != VERSION);
    if !updated && seen.is_some_and(|t| now_ms.saturating_sub(t.at_ms) < DAY_MS) {
        return None;
    }
    let skill: Option<PathBuf> = setup::items(user_home).into_iter().find(|i| i.agent == "Claude Code" && i.what == "skill")
        .map(|i| (i.path.clone(), i.state())).and_then(|(path, state)| match state {
            setup::State::Stale | setup::State::Missing => Some(path),
            setup::State::Current | setup::State::Link(_) => None,
        });
    let line = if let Some(skill) = skill {
        Some(format!("jelly: the jelly skill at {} is not the one this jelly ({VERSION}) comes with. Run `jelly setup`, then read the skill again before you use jelly.", skill.display()))
    } else if updated {
        Some(format!("jelly was updated to {VERSION} during this session. Read the jelly skill again before you use jelly: commands and `window.jelly` may have changed."))
    } else {
        let daemon: Option<Value> = ureq::get(&format!("{url}/api/version")).timeout(Duration::from_secs(1)).call().ok().and_then(|r| r.into_json().ok());
        daemon.filter(|d| d["newer"] == true).map(|d| format!("jelly {} is out; this is {VERSION}. `jelly update` installs it and updates your skill. Tell the person.", d["latest"].as_str().unwrap_or("?")))
    };
    // Sessions that went quiet a month ago are not coming back.
    told.retain(|_, t| now_ms.saturating_sub(t.at_ms) < 30 * DAY_MS);
    told.insert(session.into(), Told { version: VERSION.into(), at_ms: now_ms });
    if let Ok(text) = serde_json::to_vec(&told) {
        let _ = std::fs::write(&path, text);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_later_release_is_an_update() {
        assert!(newer("0.1.10", "0.1.9"));
        assert!(newer("1.0.0", "0.9.9"));
        for (latest, current) in [("0.1.1", "0.1.1"), ("0.1.0", "0.1.1"), ("0.2.0-beta.1", "0.1.1"), ("", "0.1.1")] {
            assert!(!newer(latest, current), "{latest} over {current}");
        }
    }

    #[test]
    fn an_npm_install_is_known_by_its_platform_package() {
        assert_eq!(Install::of(Path::new("/opt/node/lib/node_modules/jellyware/node_modules/@getjellyware/jelly-darwin-arm64/bin/jelly")), Install::Npm);
        assert_eq!(Install::of(Path::new("/home/me/.cargo/bin/jelly")), Install::Source);
        assert_eq!(Install::of(Path::new("/src/node_modules/jelly/target/release/jelly")), Install::Source);
    }
}
