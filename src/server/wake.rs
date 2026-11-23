//! Wakes an agent that cannot keep a `jelly hear --wait` running: when the person
//! presses Send, the daemon queues a message to the agent's own session. Codex
//! only today, with `codex queue`.

use super::*;

/// One Codex session to wake on the next Send in its jelly, or in any jelly of
/// its project. Memory only, and used once: it stands in for one `hear --wait`.
struct Wake {
    jelly: Option<String>,
    project: String,
    codex_thread: String,
}

static WAKES: Mutex<Vec<Wake>> = Mutex::new(Vec::new());

/// How long `codex queue` may take.
const LIMIT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
pub(super) struct WakeReq {
    jelly: Option<String>,
    #[serde(default)]
    project: String,
    codex_thread: String,
}

/// `jelly hear --wake`: wake this Codex session on the next Send.
pub(super) async fn add(State(app): State<Shared>, Json(r): Json<WakeReq>) -> Result<Json<Value>, ApiError> {
    // It becomes an argument of `codex queue`, so it must not read as a flag.
    if r.codex_thread.is_empty() || !r.codex_thread.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || r.codex_thread.starts_with('-') {
        return Err(Error::Invalid("codex_thread is a Codex session id (CODEX_THREAD_ID)".into()).into());
    }
    if let Some(jelly) = &r.jelly {
        app.store.lock().expect("store lock").get(jelly)?;
    }
    let mut wakes = WAKES.lock().expect("wakes lock");
    wakes.retain(|w| w.codex_thread != r.codex_thread);
    wakes.push(Wake { jelly: r.jelly, project: r.project, codex_thread: r.codex_thread });
    Ok(Json(json!({"ok": true})))
}

/// Watches for a Send by anyone but an agent and wakes whoever asked for it.
pub(super) fn start(app: &Shared) {
    let mut events = app.events.subscribe();
    tokio::spawn(async move {
        loop {
            let ev = match events.recv().await {
                Ok(ev) => ev,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            if ev.kind != crate::store::EventKind::Send || ev.by == "agent" {
                continue;
            }
            let project = ev.jelly.split_once(':').map_or("", |(p, _)| p);
            let due: Vec<Wake> = {
                let mut wakes = WAKES.lock().expect("wakes lock");
                let (due, rest) = std::mem::take(&mut *wakes).into_iter()
                    .partition(|w| w.jelly.as_ref().map_or(w.project == project, |j| *j == ev.jelly));
                *wakes = rest;
                due
            };
            for wake in due {
                let jelly = ev.jelly.clone();
                tokio::spawn(async move {
                    if let Err(e) = queue(&wake.codex_thread, &jelly).await {
                        eprintln!("jelly: could not wake Codex session {}: {e}", wake.codex_thread);
                    }
                });
            }
        }
    });
}

async fn queue(thread: &str, jelly: &str) -> Result<(), String> {
    // JELLY_CODEX names another binary, as JELLY_CHROME does for the browser.
    let codex = std::env::var("JELLY_CODEX").unwrap_or_else(|_| "codex".into());
    let message = format!("The person pressed Send on {jelly}: they are done there and hand the turn back. Run `jelly hear` for what they did.");
    let child = tokio::process::Command::new(&codex).args(["queue", "--thread", thread, "--message", &message])
        .stdin(std::process::Stdio::null()).kill_on_drop(true).output();
    let out = tokio::time::timeout(LIMIT, child).await
        .map_err(|_| format!("`{codex} queue` ran longer than {} s", LIMIT.as_secs()))?
        .map_err(|e| format!("could not run {codex}: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(look::clip(String::from_utf8_lossy(&out.stderr).trim(), 300)) }
}
