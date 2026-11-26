//! Feeds: commands the daemon runs on a timer to keep a jelly's data fresh
//! (`jelly feed`). The list is one file in the daemon's home. The CLI writes it
//! (`jelly feed`, and `rm`, `move` and `project --move`, which take a jelly's
//! feed with it) and the daemon only reads it, so nothing that arrives over
//! HTTP can make the daemon run a command.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The shortest interval a feed may have, in seconds.
pub const FASTEST: u64 = 5;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Feed {
    /// The jelly's full name. A jelly has at most one feed.
    pub jelly: String,
    /// The verb that takes what the command prints.
    pub verb: String,
    /// Seconds between runs.
    pub every: u64,
    /// The program and its arguments. No shell: write `sh -c '...'` for a pipe.
    pub command: Vec<String>,
    /// Where the command runs: the folder `jelly feed` was run from.
    pub cwd: PathBuf,
}

impl Feed {
    /// `refresh every 60s from "gh pr list"`, for `jelly look` and `jelly feed`.
    pub fn line(&self) -> String {
        format!("{} every {}s from {:?}", self.verb, self.every, self.command.join(" "))
    }
}

fn path(home: &Path) -> PathBuf {
    home.join("feeds.json")
}

/// Every feed in this home. No file means no feeds.
pub fn read(home: &Path) -> Result<Vec<Feed>, String> {
    match std::fs::read(path(home)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{} is not readable: {e}", path(home).display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path(home).display())),
    }
}

/// After `jelly move` or `jelly project --move`: each feed whose jelly got a new
/// name (`renamed` returns it) follows the jelly. A feed left under a new name
/// by a jelly that is gone is dropped, since a jelly has one feed. Returns how
/// many moved.
pub fn follow(feeds: &mut Vec<Feed>, renamed: impl Fn(&str) -> Option<String>) -> usize {
    let taken: Vec<String> = feeds.iter().filter_map(|f| renamed(&f.jelly)).collect();
    feeds.retain(|f| renamed(&f.jelly).is_some() || !taken.contains(&f.jelly));
    for feed in feeds.iter_mut() {
        if let Some(name) = renamed(&feed.jelly) {
            feed.jelly = name;
        }
    }
    taken.len()
}

pub fn write(home: &Path, feeds: &[Feed]) -> Result<(), String> {
    let (file, tmp) = (path(home), home.join("feeds.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(feeds).expect("feeds serialize"))
        .and_then(|_| std::fs::rename(&tmp, &file))
        .map_err(|e| format!("{}: {e}", file.display()))
}
