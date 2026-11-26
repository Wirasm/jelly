//! The daemon's part in updating jelly: it knows the latest release, says so
//! at `/api/version`, and can install it and start again on the new binary.

use super::{now_ms, ApiError, Shared};
use crate::update::{self, Install, VERSION};
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

/// When this daemon started and the binary it runs, read at start: on Linux a
/// replaced binary's path can no longer be read from the running process.
static STARTED: LazyLock<(u64, Option<PathBuf>)> = LazyLock::new(|| (now_ms(), std::env::current_exe().ok()));

#[derive(Default)]
pub(super) struct Status {
    /// The newest release npm had when last asked. None until an answer came.
    latest: Option<String>,
    run: Run,
}

/// The update the person started from the jelly page.
#[derive(Default)]
enum Run {
    #[default]
    Idle,
    Running,
    Failed(String),
}

/// Asks for the latest release now and once a day. No answer is no news:
/// being offline is normal, and it asks again at the next look.
///
/// The day is counted on the wall clock, looked at every few minutes. A timer
/// of a day would not do: it stands still while the machine sleeps, so on a
/// laptop "a day" could be most of a week.
pub(super) fn start(app: &Shared) {
    LazyLock::force(&STARTED);
    let app = app.clone();
    tokio::spawn(async move {
        let mut asked_ms = None;
        loop {
            if due(asked_ms, now_ms()) {
                if let Ok(Ok(latest)) = tokio::task::spawn_blocking(update::latest).await {
                    app.update.lock().expect("update lock").latest = Some(latest);
                    asked_ms = Some(now_ms());
                }
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    });
}

const ASK_EVERY_MS: u64 = 24 * 3600 * 1000;
const LOOK_EVERY: Duration = Duration::from_secs(10 * 60);

/// Whether to ask npm now: never answered, or the last answer is a day old.
fn due(asked_ms: Option<u64>, now_ms: u64) -> bool {
    asked_ms.is_none_or(|asked| now_ms.saturating_sub(asked) >= ASK_EVERY_MS)
}

fn install() -> Option<Install> {
    STARTED.1.as_deref().map(Install::of)
}

/// `started_ms` changes when the daemon restarts, which is how the jelly page
/// and `jelly update` know the new one is up.
pub(super) async fn version(State(app): State<Shared>) -> Json<Value> {
    let status = app.update.lock().expect("update lock");
    let source = install() != Some(Install::Npm);
    Json(json!({
        "version": VERSION,
        "started_ms": STARTED.0,
        "latest": status.latest,
        "newer": status.latest.as_deref().is_some_and(|latest| update::newer(latest, VERSION)),
        "install": if source { "source" } else { "npm" },
        "commands": if source { update::source_commands().to_vec() } else { Vec::new() },
        "updating": matches!(status.run, Run::Running),
        "failed": match &status.run { Run::Failed(why) => Some(why), _ => None },
    }))
}

/// The jelly page's "Update now": what `jelly update` does, run by the daemon.
/// The request names nothing: the package and the commands are fixed here.
pub(super) async fn update(State(app): State<Shared>) -> Result<Json<Value>, ApiError> {
    let exe = STARTED.1.clone().filter(|exe| Install::of(exe) == Install::Npm)
        .ok_or_else(|| ApiError::Conflict("this jelly was built from source: `jelly update` prints the commands that update it".into()))?;
    let latest = {
        let mut status = app.update.lock().expect("update lock");
        if matches!(status.run, Run::Running) {
            return Ok(Json(json!({"updating": true})));
        }
        let latest = status.latest.clone().filter(|latest| update::newer(latest, VERSION))
            .ok_or_else(|| ApiError::Conflict(format!("jelly {VERSION} is the latest this daemon knows of")))?;
        status.run = Run::Running;
        latest
    };
    tokio::spawn(async move {
        let applied = tokio::task::spawn_blocking(move || update::apply(&exe, &latest)).await.unwrap_or_else(|e| Err(e.to_string()));
        let failed = match applied {
            Ok(_) => restart_now(&app),
            Err(why) => why,
        };
        app.update.lock().expect("update lock").run = Run::Failed(failed);
    });
    Ok(Json(json!({"updating": true})))
}

/// Starts the daemon again on the binary now at its path, with the arguments
/// it was started with. `jelly update` asks for this after installing.
pub(super) async fn restart(State(app): State<Shared>) -> Result<Json<Value>, ApiError> {
    if cfg!(not(unix)) || STARTED.1.is_none() {
        return Err(ApiError::Conflict(CANNOT_RESTART.into()));
    }
    tokio::spawn(async move {
        // Let the answer to this request leave first.
        tokio::time::sleep(Duration::from_millis(200)).await;
        eprintln!("jelly: {}", restart_now(&app));
    });
    Ok(Json(json!({"restarting": true})))
}

const CANNOT_RESTART: &str = "this daemon cannot start itself again: stop it, and the next jelly command starts the new one";

/// Replaces this process with the binary at the daemon's path. Returns only
/// when that failed, with why.
fn restart_now(app: &Shared) -> String {
    #[cfg(unix)]
    if let Some(exe) = &STARTED.1 {
        use std::os::unix::process::CommandExt;
        // No verb or write is half done while the store lock is held.
        let _store = app.store.lock();
        let e = std::process::Command::new(exe).args(std::env::args_os().skip(1)).exec();
        return format!("could not start {} again: {e}. Stop this daemon, and the next jelly command starts the new one", exe.display());
    }
    let _ = app;
    CANNOT_RESTART.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_release_is_asked_for_once_a_day_by_the_wall_clock() {
        assert!(due(None, 5), "no answer yet: ask");
        assert!(!due(Some(1_000), 1_000 + ASK_EVERY_MS - 1));
        // A laptop asleep for three days wakes with the same timer but a later clock.
        assert!(due(Some(1_000), 1_000 + 3 * ASK_EVERY_MS));
    }
}
