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
/// It reads Sends from the event log, as a wait does, and the live channel only
/// wakes it, so a burst that overflows the channel cannot drop one.
pub(super) fn start(app: &Shared) {
    let mut events = app.events.subscribe();
    let (mut q, mut history) = {
        let store = app.store.lock().expect("store lock");
        let q = hear::WaitQ { since: Some(store.sequence()), by: "agent".into(), jelly: None, timeout: None, every: false };
        match hear::History::at_end(&store) {
            Ok(history) => (q, history),
            Err(e) => { eprintln!("jelly: --wake cannot read the event log: {e}"); return; }
        }
    };
    let app = app.clone();
    tokio::spawn(async move {
        loop {
            if let Err(broadcast::error::RecvError::Closed) = events.recv().await { return; }
            loop {
                let next = history.next_change(&app.store.lock().expect("store lock"), &q);
                let ev = match next {
                    Ok(Some(ev)) => ev,
                    Ok(None) => break,
                    // Tried again on the next event.
                    Err(e) => { eprintln!("jelly: --wake could not read the event log: {e}"); break; }
                };
                q.since = Some(ev.seq);
                wake(ev);
            }
        }
    });
}

/// Wakes the sessions waiting on this Send's jelly or project.
fn wake(ev: Event) {
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

async fn queue(thread: &str, jelly: &str) -> Result<(), String> {
    // JELLY_CODEX names another binary, as JELLY_CHROME does for the browser.
    // On Windows the native Codex is codex.exe, which a bare name finds, and
    // the npm one is codex.cmd, which it does not: try one, then the other.
    let named = std::env::var("JELLY_CODEX").ok();
    let tries = match named {
        Some(codex) => vec![codex],
        None if cfg!(windows) => vec!["codex".into(), "codex.cmd".into()],
        None => vec!["codex".into()],
    };
    let message = format!("The person pressed Send on {jelly}: they are done there and hand the turn back. Run `jelly hear` for what they did.");
    let mut tries = tries.iter().peekable();
    let out = loop {
        let codex = tries.next().expect("one name to try");
        let child = tokio::process::Command::new(codex).args(["queue", "--thread", thread, "--message", &message])
            .stdin(std::process::Stdio::null()).kill_on_drop(true).output();
        match tokio::time::timeout(LIMIT, child).await {
            Err(_) => return Err(format!("`{codex} queue` ran longer than {} s", LIMIT.as_secs())),
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound && tries.peek().is_some() => continue,
            Ok(Err(e)) => return Err(format!("could not run {codex}: {e}")),
            Ok(Ok(out)) => break out,
        }
    };
    if out.status.success() { Ok(()) } else { Err(look::clip(String::from_utf8_lossy(&out.stderr).trim(), 300)) }
}
