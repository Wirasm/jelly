//! `jelly setup`: gives the coding agents on this machine the jelly skill and,
//! where an agent has one, the part that tells it the person pressed Send.

use crate::recipes::RECIPES;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const SKILL: &str = include_str!("../skill/SKILL.md");
const PI_EXTENSION: &str = include_str!("../assets/pi-extension.ts");
/// Runs at the start of every Claude Code turn; prints nothing when nothing is new.
const HOOK: &str = "jelly hear --hook";
/// Runs when the person approves a plan in Claude Code's plan mode.
const PLAN_HOOK: &str = "jelly plan-approved";

/// What `jelly plan-approved` prints: Claude Code hands `additionalContext` to
/// the agent with the approval. Whether the plan is worth a jelly is the agent's call.
pub fn plan_approved() -> Value {
    json!({"hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext":
        "The person approved your plan. Before you change anything, show it to them as a plan jelly, so they can \
         follow the work and steer it: run `jelly recipes plan` and do what it says. \
         Skip this only for a plan of three steps or fewer."}})
}

const CLAUDE_NOTE: &str = "Claude Code  note   Claude Code asks the person before each `jelly` command. To stop that, add \
    \"Bash(jelly:*)\" to permissions.allow in ~/.claude/settings.json. It is not added for you: \
    `jelly feed` runs any command, so the rule lets the agent run commands without asking";
const CODEX_NOTE: &str = "Codex        note   Codex's sandbox blocks jelly from reaching its daemon. Let it through by adding to ~/.codex/config.toml: \
    `[sandbox_workspace_write]` and under it `network_access = true`; files stay sandboxed. \
    Or approve each `jelly` command as Codex asks. To hand the turn to the person, Codex runs `jelly hear --wake NAME` and ends its turn";

/// One thing jelly keeps in an agent's folder.
pub struct Item {
    pub agent: &'static str,
    /// "skill", "hook", "plan" or "wake".
    pub what: &'static str,
    pub path: PathBuf,
    kind: Kind,
}

enum Kind {
    /// The skill folder: SKILL.md and the recipes it points at.
    Skill,
    /// A command among Claude Code's hooks for `event`, for the tool `matcher` names.
    Hook { event: &'static str, matcher: Option<&'static str>, command: &'static str },
    /// One file with this text.
    File(&'static str),
}

/// How an item on disk compares with what this jelly would write.
pub enum State {
    Current,
    /// The person's own arrangement (a checkout of jelly), which `setup` leaves alone.
    Link(PathBuf),
    Missing,
    Stale,
}

/// What belongs where, for every agent that has a folder in `home`. `setup`
/// writes from this list and `doctor` checks against it.
pub fn items(home: &Path) -> Vec<Item> {
    let item = |agent, what, path, kind| Item { agent, what, path, kind };
    let mut items = Vec::new();
    let claude = home.join(".claude");
    if claude.is_dir() {
        items.push(item("Claude Code", "skill", claude.join("skills/jelly"), Kind::Skill));
        let settings = claude.join("settings.json");
        items.push(item("Claude Code", "hook", settings.clone(), Kind::Hook { event: "UserPromptSubmit", matcher: None, command: HOOK }));
        items.push(item("Claude Code", "plan", settings, Kind::Hook { event: "PostToolUse", matcher: Some("ExitPlanMode"), command: PLAN_HOOK }));
    }
    let codex = home.join(".codex");
    if codex.is_dir() {
        items.push(item("Codex", "skill", codex.join("skills/jelly"), Kind::Skill));
    }
    let pi = home.join(".pi/agent");
    if pi.is_dir() {
        items.push(item("Pi", "skill", pi.join("skills/jelly"), Kind::Skill));
        items.push(item("Pi", "wake", pi.join("extensions/jelly.ts"), Kind::File(PI_EXTENSION)));
    }
    items
}

pub const NO_AGENT: &str = "there is no ~/.claude, ~/.codex or ~/.pi/agent";

/// Sets up every agent that has a folder in `home`, and says what it did, one
/// line per thing. With `dry_run` nothing is written.
pub fn run(home: &Path, dry_run: bool) -> Result<Vec<String>, String> {
    let items = items(home);
    if items.is_empty() {
        return Err(format!("found no agent to set up: {NO_AGENT}"));
    }
    let mut out = Vec::new();
    for item in &items {
        out.push(item.write(dry_run)?);
        match (item.agent, item.what) {
            ("Claude Code", "plan") => out.push(CLAUDE_NOTE.into()),
            ("Codex", _) => out.push(CODEX_NOTE.into()),
            _ => {}
        }
    }
    Ok(out)
}

/// Every file of the skill folder, by its path inside it.
fn skill_files() -> impl Iterator<Item = (PathBuf, &'static [u8])> {
    let recipes = RECIPES.iter().flat_map(|r| r.files.iter().map(|(path, bytes)| (Path::new("recipes").join(r.name).join(path), *bytes)));
    std::iter::once((PathBuf::from("SKILL.md"), SKILL.as_bytes())).chain(recipes)
}

impl Item {
    pub fn state(&self) -> State {
        if let Ok(target) = fs::read_link(&self.path) {
            return State::Link(target);
        }
        let same = |path: &Path, bytes: &[u8]| fs::read(path).is_ok_and(|on_disk| on_disk == bytes);
        match self.kind {
            _ if !self.path.exists() => State::Missing,
            Kind::Skill if skill_files().all(|(file, bytes)| same(&self.path.join(file), bytes)) => State::Current,
            Kind::File(text) if same(&self.path, text.as_bytes()) => State::Current,
            Kind::Skill | Kind::File(_) => State::Stale,
            Kind::Hook { event, command, .. } => {
                let settings = fs::read_to_string(&self.path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok());
                let Some(mut settings) = settings else { return State::Missing };
                let found = ours(&mut settings, event, command);
                if found.iter().any(|c| *c == command) {
                    State::Current
                } else if found.is_empty() {
                    State::Missing
                } else {
                    State::Stale
                }
            }
        }
    }

    fn line(&self, did: &str) -> String {
        format!("{:<12} {:<6} {} ({did})", self.agent, self.what, self.path.display())
    }

    fn write(&self, dry_run: bool) -> Result<String, String> {
        if let Kind::Hook { event, matcher, command } = self.kind {
            return self.write_hook(event, matcher, command, dry_run);
        }
        // A link is the person's own arrangement (a checkout of jelly), so it stays.
        if let Ok(target) = fs::read_link(&self.path) {
            return Ok(self.line(&format!("left alone: it is a link to {}", target.display())));
        }
        let existed = self.path.exists();
        if !dry_run {
            let failed = |e: std::io::Error| format!("could not write {}: {e}", self.path.display());
            let write = |to: &Path, bytes: &[u8]| {
                fs::create_dir_all(to.parent().expect("a file has a folder")).map_err(failed)?;
                fs::write(to, bytes).map_err(failed)
            };
            match self.kind {
                Kind::Skill => {
                    // Recipes a newer jelly dropped must not stay behind.
                    if existed { fs::remove_dir_all(&self.path).map_err(failed)?; }
                    for (file, bytes) in skill_files() {
                        write(&self.path.join(file), bytes)?;
                    }
                }
                Kind::File(text) => write(&self.path, text.as_bytes())?,
                Kind::Hook { .. } => unreachable!("the hook is written above"),
            }
        }
        Ok(self.line(match (dry_run, existed) {
            (true, true) => "would update",
            (true, false) => "would write",
            (false, true) => "updated",
            (false, false) => "written",
        }))
    }

    /// Adds `command` to Claude Code's hooks for `event`, unless a hook there
    /// already runs it. A hook that runs jelly by another path (a binary that
    /// may be gone) gets `command` instead.
    fn write_hook(&self, event: &str, matcher: Option<&str>, command: &str, dry_run: bool) -> Result<String, String> {
        let path = &self.path;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
            Err(e) => return Err(format!("could not read {}: {e}", path.display())),
        };
        let Ok(mut settings) = serde_json::from_str::<Value>(&text) else {
            return Ok(self.line("left alone: it is not valid JSON"));
        };
        let save = |settings: &Value| -> Result<(), String> {
            if dry_run { return Ok(()); }
            let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())? + "\n";
            fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
        };
        let found = ours(&mut settings, event, command);
        if !found.is_empty() {
            if found.iter().all(|c| *c == command) {
                return Ok(self.line("already there"));
            }
            for c in found {
                *c = json!(command);
            }
            save(&settings)?;
            return Ok(self.line(if dry_run { "would update" } else { "updated" }));
        }
        let hooks = settings.as_object_mut().map(|s| s.entry("hooks").or_insert_with(|| json!({})));
        let listed = hooks.and_then(Value::as_object_mut).map(|h| h.entry(event).or_insert_with(|| json!([])));
        let Some(list) = listed.and_then(Value::as_array_mut) else {
            return Ok(self.line("left alone: its hooks are not in the usual shape"));
        };
        let mut entry = json!({"hooks": [{"type": "command", "command": command, "timeout": 3}]});
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        list.push(entry);
        save(&settings)?;
        Ok(self.line(if dry_run { "would add" } else { "added" }))
    }
}

/// The commands among Claude Code's hooks for `event` that run jelly's
/// `command`: exactly, or with a path to the binary (`/old/bin/jelly hear --hook`).
fn ours<'a>(settings: &'a mut Value, event: &str, command: &str) -> Vec<&'a mut Value> {
    let runs = |c: &str| c == command || c.strip_suffix(command).is_some_and(|dir| dir.ends_with(['/', '\\']));
    settings.get_mut("hooks").and_then(|h| h.get_mut(event)).and_then(Value::as_array_mut).into_iter().flatten()
        .filter_map(|entry| entry.get_mut("hooks").and_then(Value::as_array_mut)).flatten()
        .filter_map(|hook| hook.get_mut("command")).filter(|c| c.as_str().is_some_and(runs)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_writes_for_the_agents_it_finds_and_keeps_what_is_theirs() {
        let home = tempfile::tempdir().unwrap();
        let at = |p: &str| home.path().join(p);
        assert!(run(home.path(), false).is_err(), "no agent, nothing to do");

        fs::create_dir_all(at(".claude")).unwrap();
        fs::write(at(".claude/settings.json"), r#"{"model": "x", "hooks": {"Stop": []}}"#).unwrap();
        fs::create_dir_all(at(".pi/agent/extensions")).unwrap();
        fs::create_dir_all(at(".codex/skills")).unwrap();
        // The person's own checkout, linked in as the skill.
        #[cfg(unix)]
        std::os::unix::fs::symlink(home.path(), at(".codex/skills/jelly")).unwrap();

        let dry = run(home.path(), true).unwrap();
        assert!(dry.iter().any(|l| l.contains("would write")) && !at(".claude/skills").exists(), "{dry:?}");

        let out = run(home.path(), false).unwrap().join("\n");
        assert!(fs::read_to_string(at(".claude/skills/jelly/SKILL.md")).unwrap().starts_with("---"));
        assert!(at(".claude/skills/jelly/recipes/plan/RECIPE.md").exists());
        assert!(at(".pi/agent/skills/jelly/SKILL.md").exists());
        assert!(fs::read_to_string(at(".pi/agent/extensions/jelly.ts")).unwrap().contains("sendUserMessage"));
        #[cfg(unix)]
        assert!(out.contains("left alone: it is a link") && !at(".codex/skills/jelly/SKILL.md").exists(), "{out}");
        let settings: Value = serde_json::from_str(&fs::read_to_string(at(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(settings["model"], "x");
        assert_eq!(settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], HOOK);
        assert_eq!(settings["hooks"]["PostToolUse"][0], json!({"matcher": "ExitPlanMode", "hooks": [{"type": "command", "command": PLAN_HOOK, "timeout": 3}]}));

        // A second run changes nothing it should not.
        let again = run(home.path(), false).unwrap().join("\n");
        assert!(again.contains("already there") && again.contains("updated"), "{again}");
        let after: Value = serde_json::from_str(&fs::read_to_string(at(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(after["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
        assert_eq!(after["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
    }

    /// A source build wrote its own path into the hooks; after moving to the npm
    /// release that binary is gone and every prompt failed with exit 127.
    #[test]
    fn a_hook_that_runs_jelly_by_another_path_is_replaced() {
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join(".claude/settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        let old = |command: &str| json!([{"hooks": [{"type": "command", "command": format!("/gone/bin/{command}"), "timeout": 3}]}]);
        fs::write(&settings, json!({"hooks": {"UserPromptSubmit": old(HOOK), "PostToolUse": old(PLAN_HOOK)}}).to_string()).unwrap();
        let hooks = || items(home.path()).into_iter().filter(|i| matches!(i.kind, Kind::Hook { .. })).collect::<Vec<_>>();
        assert!(hooks().iter().all(|i| matches!(i.state(), State::Stale)), "doctor says to fix it");

        let out = run(home.path(), false).unwrap().join("\n");
        assert!(!out.contains("already there"), "{out}");
        let after: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(after["hooks"]["UserPromptSubmit"], json!([{"hooks": [{"type": "command", "command": HOOK, "timeout": 3}]}]));
        assert_eq!(after["hooks"]["PostToolUse"][0]["hooks"][0]["command"], PLAN_HOOK);
        assert!(hooks().iter().all(|i| matches!(i.state(), State::Current)));
    }
}
