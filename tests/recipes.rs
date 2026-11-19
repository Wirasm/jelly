mod common;

use common::served_url;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_jelly");
const RECIPES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/skill/recipes");

fn ok(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

/// Recipes come from the binary, so every command here runs outside the checkout.
#[test]
fn recipes_list_write_and_make_jellies_from_any_directory() {
    let temp = tempfile::tempdir().unwrap();
    let mut daemon = Command::new(BIN).args(["serve", "--port", "0", "--home"]).arg(temp.path().join("daemon"))
        .stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let url = served_url(&mut daemon);
    let jelly = |args: &[&str]| {
        Command::new(BIN).current_dir(temp.path()).args(["--url", &url, "--project", "p"]).args(args)
            .env_remove("JELLY_AS").stdin(Stdio::null()).output().unwrap()
    };

    let mut names: Vec<String> = std::fs::read_dir(RECIPES).unwrap().flatten()
        .filter(|e| e.path().is_dir()).map(|e| e.file_name().into_string().unwrap()).collect();
    names.sort();
    let list = ok(jelly(&["recipes"]));
    for name in &names {
        let line = list.lines().find(|l| l.split_whitespace().next() == Some(name.as_str()))
            .unwrap_or_else(|| panic!("{name} missing from:\n{list}"));
        assert!(line.trim_end().ends_with('.') && line.len() > name.len() + 20, "no use-when line: {line}");
    }

    let guide = ok(jelly(&["recipes", "plan"]));
    assert_eq!(guide, std::fs::read_to_string(Path::new(RECIPES).join("plan/RECIPE.md")).unwrap());

    ok(jelly(&["recipes", "monitor", "--out", "mon"]));
    for file in ["view.html", "verbs.js", "state.json", "start.json", "driver.sh", "RECIPE.md"] {
        assert_eq!(std::fs::read(temp.path().join("mon").join(file)).unwrap(),
            std::fs::read(Path::new(RECIPES).join("monitor").join(file)).unwrap(), "{file}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(temp.path().join("mon/driver.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "driver.sh stays runnable");
    }

    let made = ok(jelly(&["new", "my-plan", "--recipe", "plan"]));
    let mut assets: Vec<String> = std::fs::read_dir(Path::new(RECIPES).join("plan/assets")).unwrap().flatten()
        .map(|e| e.file_name().into_string().unwrap()).collect();
    assets.sort();
    for file in &assets {
        assert!(made.contains(&format!("assets/{file}")), "{made}");
    }
    let j: Value = ureq::get(&format!("{url}/api/jellies/p:my-plan")).call().unwrap().into_json().unwrap();
    let source = |f: &str| std::fs::read_to_string(Path::new(RECIPES).join("plan").join(f)).unwrap();
    assert_eq!(j["view"], source("view.html"));
    assert_eq!(j["verbs"], source("verbs.js"));
    assert_eq!(j["state"], serde_json::from_str::<Value>(&source("state.json")).unwrap());
    let stored: Value = ureq::get(&format!("{url}/api/jellies/p:my-plan/assets")).call().unwrap().into_json().unwrap();
    assert_eq!(stored, serde_json::json!(assets));

    // --state replaces the example state, and the example's assets stay out:
    // the review recipe's full.diff would show beside the agent's own review.
    std::fs::write(temp.path().join("s.json"), r#"{"groups":[]}"#).unwrap();
    ok(jelly(&["new", "mine", "--recipe", "review", "--state", "s.json"]));
    let state: Value = ureq::get(&format!("{url}/api/jellies/p:mine/state")).call().unwrap().into_json().unwrap();
    assert_eq!(state, serde_json::json!({"groups": []}));
    let stored: Value = ureq::get(&format!("{url}/api/jellies/p:mine/assets")).call().unwrap().into_json().unwrap();
    assert_eq!(stored, serde_json::json!([]));

    let unknown = jelly(&["new", "x", "--recipe", "nope"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("there are map, monitor"), "{unknown:?}");

    let _ = daemon.kill();
    let _ = daemon.wait();
}
