//! `jelly setup`: gives the coding agents on this machine the jelly skill and,
//! where an agent has one, the part that tells it the person pressed Send.

use crate::recipes::RECIPES;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const SKILL: &str = include_str!("../skill/SKILL.md");
const PI_EXTENSION: &str = include_str!("../assets/pi-extension.ts");
/// Runs at the start of every Claude Code turn; prints nothing when nothing is new.
const HOOK: &str = "jelly hear --hook";

/// Sets up every agent that has a folder in `home`, and says what it did, one
/// line per thing. With `dry_run` nothing is written.
pub fn run(home: &Path, dry_run: bool) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let claude = home.join(".claude");
    if claude.is_dir() {
        out.push(skill("Claude Code", &claude.join("skills/jelly"), dry_run)?);
        out.push(claude_hook(&claude.join("settings.json"), dry_run)?);
    }
    let codex = home.join(".codex");
    if codex.is_dir() {
        out.push(skill("Codex", &codex.join("skills/jelly"), dry_run)?);
        out.push("Codex        note   jelly must reach its daemon, which Codex's sandbox blocks: approve `jelly` commands to run outside it. \
            To hand the turn to the person, Codex runs `jelly hear --wake NAME` and ends its turn".into());
    }
    let pi = home.join(".pi/agent");
    if pi.is_dir() {
        out.push(skill("Pi", &pi.join("skills/jelly"), dry_run)?);
        out.push(file("Pi", "wake", &pi.join("extensions/jelly.ts"), PI_EXTENSION, dry_run)?);
    }
    if out.is_empty() {
        return Err("found no agent to set up: there is no ~/.claude, ~/.codex or ~/.pi/agent".into());
    }
    Ok(out)
}

fn line(agent: &str, what: &str, path: &Path, did: &str) -> String {
    format!("{agent:<12} {what:<6} {} ({did})", path.display())
}

/// The skill folder: SKILL.md and the recipes it points at. A link there is the
/// person's own arrangement (a checkout of jelly), so it stays.
fn skill(agent: &str, dir: &Path, dry_run: bool) -> Result<String, String> {
    if let Ok(target) = fs::read_link(dir) {
        return Ok(line(agent, "skill", dir, &format!("left alone: it is a link to {}", target.display())));
    }
    let existed = dir.exists();
    if !dry_run {
        let failed = |e: std::io::Error| format!("could not write the skill to {}: {e}", dir.display());
        // Recipes a newer jelly dropped must not stay behind.
        if existed { fs::remove_dir_all(dir).map_err(failed)?; }
        fs::create_dir_all(dir).map_err(failed)?;
        fs::write(dir.join("SKILL.md"), SKILL).map_err(failed)?;
        for recipe in RECIPES {
            for (path, bytes) in recipe.files {
                let to = dir.join("recipes").join(recipe.name).join(path);
                fs::create_dir_all(to.parent().expect("a file has a folder")).map_err(failed)?;
                fs::write(to, bytes).map_err(failed)?;
            }
        }
    }
    Ok(line(agent, "skill", dir, done(dry_run, existed)))
}

fn file(agent: &str, what: &str, path: &Path, text: &str, dry_run: bool) -> Result<String, String> {
    if let Ok(target) = fs::read_link(path) {
        return Ok(line(agent, what, path, &format!("left alone: it is a link to {}", target.display())));
    }
    let existed = path.exists();
    if !dry_run {
        let failed = |e: std::io::Error| format!("could not write {}: {e}", path.display());
        fs::create_dir_all(path.parent().expect("a file has a folder")).map_err(failed)?;
        fs::write(path, text).map_err(failed)?;
    }
    Ok(line(agent, what, path, done(dry_run, existed)))
}

fn done(dry_run: bool, existed: bool) -> &'static str {
    match (dry_run, existed) {
        (true, true) => "would update",
        (true, false) => "would write",
        (false, true) => "updated",
        (false, false) => "written",
    }
}

/// Adds `jelly hear --hook` to Claude Code's UserPromptSubmit hooks, unless a
/// hook there already runs it.
fn claude_hook(path: &Path, dry_run: bool) -> Result<String, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    if text.contains(HOOK) {
        return Ok(line("Claude Code", "hook", path, "already there"));
    }
    let Ok(mut settings) = serde_json::from_str::<Value>(&text) else {
        return Ok(line("Claude Code", "hook", path, "left alone: it is not valid JSON"));
    };
    let hooks = settings.as_object_mut().map(|s| s.entry("hooks").or_insert_with(|| json!({})));
    let prompt = hooks.and_then(Value::as_object_mut).map(|h| h.entry("UserPromptSubmit").or_insert_with(|| json!([])));
    let Some(list) = prompt.and_then(Value::as_array_mut) else {
        return Ok(line("Claude Code", "hook", path, "left alone: its hooks are not in the usual shape"));
    };
    list.push(json!({"hooks": [{"type": "command", "command": HOOK, "timeout": 3}]}));
    if !dry_run {
        let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())? + "\n";
        fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    Ok(line("Claude Code", "hook", path, if dry_run { "would add" } else { "added" }))
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

        // A second run changes nothing it should not.
        let again = run(home.path(), false).unwrap().join("\n");
        assert!(again.contains("already there") && again.contains("updated"), "{again}");
        let after: Value = serde_json::from_str(&fs::read_to_string(at(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(after["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
    }
}
