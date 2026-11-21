//! Runs the home's feeds: each command on its timer, its output into its verb.

use super::*;
use crate::feed::Feed;

/// How a feed's last run went. Memory only.
#[derive(Clone)]
pub(super) struct FeedRun {
    pub(super) at_ms: u64,
    pub(super) error: Option<String>,
}

/// A command that runs longer than this is stopped.
const LIMIT: Duration = Duration::from_secs(60);

/// Checks every second which feeds are due. The file is read each time, so
/// `jelly feed` takes effect without restarting the daemon.
pub(super) fn start(app: Shared) {
    tokio::spawn(async move {
        let mut started: std::collections::HashMap<String, Instant> = Default::default();
        let running: Arc<Mutex<std::collections::HashSet<String>>> = Default::default();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let home = app.store.lock().expect("store lock").home().to_path_buf();
            let feeds = crate::feed::read(&home).unwrap_or_default();
            started.retain(|name, _| feeds.iter().any(|f| f.jelly == *name));
            for feed in feeds {
                // An archived jelly's feed rests until someone restores the jelly.
                let awake = app.store.lock().expect("store lock").get(&feed.jelly).is_ok_and(|j| !j.archived);
                let due = started.get(&feed.jelly).is_none_or(|at| at.elapsed() >= Duration::from_secs(feed.every.max(crate::feed::FASTEST)));
                if !awake || !due || !running.lock().expect("running lock").insert(feed.jelly.clone()) {
                    continue;
                }
                started.insert(feed.jelly.clone(), Instant::now());
                let (app, running) = (app.clone(), running.clone());
                tokio::spawn(async move {
                    let error = run(&app, &feed).await.err();
                    app.feeds.lock().expect("feeds lock").insert(feed.jelly.clone(), FeedRun { at_ms: now_ms(), error });
                    running.lock().expect("running lock").remove(&feed.jelly);
                });
            }
        }
    });
}

/// One run: the command's standard output, as JSON, becomes the verb's arguments.
async fn run(app: &Shared, feed: &Feed) -> Result<(), String> {
    let (program, args) = feed.command.split_first().ok_or("the feed has no command")?;
    let child = tokio::process::Command::new(program).args(args).current_dir(&feed.cwd)
        .stdin(std::process::Stdio::null()).kill_on_drop(true).output();
    let out = tokio::time::timeout(LIMIT, child).await
        .map_err(|_| format!("the command ran longer than {} s", LIMIT.as_secs()))?
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        return Err(format!("the command failed ({}): {}", out.status, look::clip(said.trim(), 300)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let args: Value = if text.trim().is_empty() { Value::Null } else {
        serde_json::from_str(text.trim()).map_err(|e| format!("the command did not print JSON: {e}"))?
    };
    let done = match app.live(&feed.jelly) {
        Some(live) => share::forward(app, &live, json!({"op": "do", "verb": feed.verb, "args": args, "by": crate::store::FEED})).await.map(|_| ()),
        None => app.change(|s| s.do_verb(&feed.jelly, &feed.verb, args, crate::store::FEED)).map(|_| ()),
    };
    done.map_err(|e| match e {
        ApiError::Store(e) => e.to_string(),
        ApiError::Conflict(e) | ApiError::Internal(e) => e,
    })
}

/// The jelly's feed and how its last run went, for `jelly look`.
pub(super) fn line(app: &App, home: &std::path::Path, jelly: &str) -> Option<String> {
    let feed = crate::feed::read(home).ok()?.into_iter().find(|f| f.jelly == jelly)?;
    let last = match app.feeds.lock().expect("feeds lock").get(jelly) {
        Some(FeedRun { at_ms, error: None }) => format!("ran {}", ago(*at_ms)),
        Some(FeedRun { at_ms, error: Some(e) }) => format!("FAILED {}: {e}", ago(*at_ms)),
        None => "has not run yet".into(),
    };
    Some(format!("feed     {} · {last}", feed.line()))
}
