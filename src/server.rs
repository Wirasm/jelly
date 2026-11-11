//! The local daemon: an HTTP API over the store, a live event stream, and the
//! views. The jelly page people look at is itself an internal jelly named `surface`.

use crate::store::{AttentionKind, Error, Event, Store, PAGE, PERSON};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::body::Bytes;
use axum::http::{header, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::Instant;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

const RUNTIME_JS: &str = include_str!("../assets/runtime.js");
const SURFACE_VIEW: &str = include_str!("../assets/surface.html");
const SURFACE_VERBS: &str = include_str!("../assets/surface.verbs.js");
const SNAPSHOT_JS: &str = include_str!("../assets/snapshot.js");
const ASK_VIEW: &str = include_str!("../assets/ask.html");
const ASK_VERBS: &str = include_str!("../assets/ask.verbs.js");
const ARCHIVE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;

struct App {
    store: Mutex<Store>,
    events: broadcast::Sender<Event>,
    /// What some browser last showed of each jelly, as text. Memory only.
    screens: Mutex<std::collections::HashMap<String, Screen>>,
}
type Shared = Arc<App>;

impl App {
    /// Broadcasts committed batch events even when a later change fails.
    fn change_many(&self, f: impl FnOnce(&mut Store) -> Result<Vec<Event>, crate::store::BatchError>) -> Result<Json<Value>, ApiError> {
        let result = f(&mut self.store.lock().expect("store lock"));
        let (evs, error) = match result {
            Ok(events) => (events, None),
            Err(failed) => (failed.events, Some(failed.error)),
        };
        for ev in &evs {
            let _ = self.events.send(ev.clone());
        }
        if let Some(error) = error {
            if let Some(last) = evs.last() {
                return Err(Error::Invalid(format!("{error}; committed {} changes to {} through seq:{}. Inspect with `jelly show {}` before retrying", evs.len(), last.jelly, last.seq, last.jelly)).into());
            }
            return Err(error.into());
        }
        Ok(Json(json!({"jelly": evs.last().map(|e| e.jelly.clone()), "events": evs.len(), "seq": evs.last().map(|e| e.seq), "state": evs.last().and_then(|e| e.state.clone())})))
    }

    /// Runs one store change and broadcasts its event. The store lock is what
    /// makes verbs run one at a time.
    fn change(&self, f: impl FnOnce(&mut Store) -> Result<Event, Error>) -> Result<Json<Event>, ApiError> {
        let ev = f(&mut self.store.lock().expect("store lock"))?;
        let _ = self.events.send(ev.clone());
        Ok(Json(ev))
    }
}

pub async fn serve(store: Store, port: u16) -> std::io::Result<()> {
    let mut store = store;
    if store.get(PAGE).is_err() {
        store
            .create(PAGE, SURFACE_VIEW.into(), SURFACE_VERBS.into(), json!({}), "jelly", false, "The jelly page", "")
            .and_then(|_| store.pin(PAGE, true, "jelly"))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    let (tx, _) = broadcast::channel(1024);
    let app: Shared = Arc::new(App { store: Mutex::new(store), events: tx, screens: Mutex::default() });

    // Unpinned jellies nobody touched for a week are archived: hidden from the
    // jelly page, restorable, and back on their own as soon as anything changes them.
    let sweeper = app.clone();
    tokio::spawn(async move {
        loop {
            let swept = sweeper.store.lock().expect("store lock").sweep(ARCHIVE_AFTER_MS);
            for ev in swept {
                let _ = sweeper.events.send(ev);
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    let router = Router::new()
        // The jelly page: every project at /, one project at /p/<project>.
        .route("/", get(|st: State<Shared>| async move { view(st, Path(PAGE.into())).await }))
        .route("/runtime.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], RUNTIME_JS) }))
        // Views live at /j/<name>/ so a view can use relative `assets/<file>` paths.
        .route("/j/{name}", get(|Path(name): Path<String>| async move { Redirect::permanent(&format!("/j/{name}/")) }))
        .route("/j/{name}/", get(view))
        .route("/j/{name}/assets/{file}", get(asset))
        // A project's page is the same page, showing only that project's jellies.
        .route("/p/{project}", get(|st: State<Shared>| async move { view(st, Path(PAGE.into())).await }))
        .route("/api/events", get(events))
        .route("/api/wait", get(wait))
        .route("/api/ws", get(ws))
        .route("/api/jellies", get(list).post(create))
        .route("/api/jellies/{name}", get(show).delete(remove))
        .route("/api/jellies/{name}/state", get(state))
        .route("/api/jellies/{name}/call", post(call))
        .route("/api/jellies/{name}/reshape", post(reshape))
        .route("/api/jellies/{name}/set", post(set_state))
        .route("/api/jellies/{name}/undo", post(undo))
        .route("/api/jellies/{name}/pin", post(pin))
        .route("/api/jellies/{name}/move", post(move_project))
        .route("/api/jellies/{name}/restore", post(restore))
        .route("/api/jellies/{name}/archive", post(archive))
        .route("/api/jellies/{name}/assets/{file}", post(put_asset).layer(DefaultBodyLimit::max(25 * 1024 * 1024)))
        .route("/api/jellies/{name}/share", get(share))
        .route("/api/inbox", get(inbox))
        .route("/api/jellies/{name}/screen", post(put_screen))
        .route("/api/jellies/{name}/note", post(note))
        .route("/api/jellies/{name}/see", get(see))
        .route("/see-frame/{name}", get(see_frame))
        .route("/api/projects/{project}/design", get(get_design).put(put_design))
        .route("/p/{project}/DESIGN.md", get(get_design))
        .route("/p/{project}/design.css", get(design_css))
        .route("/api/attention", get(attention_list))
        .route("/api/focus", post(focus))
        .route("/api/jellies/{name}/attention", post(attend))
        .route("/api/jellies/{name}/ask", post(ask))
        .route("/api/jellies/{name}/attention/{id}/answer", post(answer))
        .route("/api/jellies/{name}/attention/{id}/withdraw", post(withdraw))
        .route("/api/jellies/{name}/working", post(working))
        .route("/api/jellies/{name}/comment", post(comment))
        .route("/api/jellies/{name}/resolve", post(resolve))
        .route("/api/jellies/{name}/reply", post(reply))
        .with_state(app);

    // Loopback only: anything that reaches this port can run verbs.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("jelly on http://127.0.0.1:{port}");
    axum::serve(listener, router).await
}

#[derive(Debug)]
struct ApiError(Error);
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Exists(_) => StatusCode::CONFLICT,
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
        };
        (status, Json(json!({"error": self.0.to_string()}))).into_response()
    }
}

/// Every change says who made it. Nothing defaults to the person, so a script
/// that forgets cannot answer on their behalf.
fn by(by: Option<String>) -> Result<String, Error> {
    by.filter(|b| !b.trim().is_empty()).ok_or_else(|| Error::Invalid("say who is making this change: by".into()))
}

async fn list(State(app): State<Shared>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    // The page that shows the jellies is internal; it is not one of yours.
    Json(json!(store
        .list()
        .filter(|j| j.name != PAGE)
        .map(|j| json!({
            "name": j.name, "version": j.version, "view_rev": j.view_rev,
            "pinned": j.pinned, "made_by": j.made_by, "updated_ms": j.updated_ms, "about": j.about, "project": j.project(), "archived": j.archived,
            "created_ms": if j.created_ms > 0 { j.created_ms } else { j.updated_ms },
            "open_comments": j.comments.iter().filter(|f| !f.resolved).count(),
            "replies_waiting": j.comments.iter().filter(|c| c.awaiting_person()).count(),
            "attention": j.attention.iter().filter(|a| a.is_open()).count(),
            "working": j.working,
            "poster": store.assets(&j.name).into_iter().find(|f| f.starts_with("poster.")),
        }))
        .collect::<Vec<_>>()))
}

async fn show(State(app): State<Shared>, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(app.store.lock().expect("store lock").get(&name)?)))
}

#[derive(Deserialize)]
struct PointerQ {
    pointer: Option<String>,
}

async fn state(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<PointerQ>) -> Result<Json<Value>, ApiError> {
    let store = app.store.lock().expect("store lock");
    let s = &store.get(&name)?.state;
    match q.pointer.as_deref() {
        None | Some("") => Ok(Json(s.clone())),
        Some(p) => s.pointer(p).cloned().map(Json).ok_or_else(|| Error::Invalid(format!("{name} has nothing at {p}")).into()),
    }
}

#[derive(Deserialize)]
struct CreateReq {
    name: String,
    view: String,
    verbs: String,
    #[serde(default)]
    state: Value,
    by: Option<String>,
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    about: String,
    #[serde(default)]
    project: String,
}

async fn create(State(app): State<Shared>, Json(r): Json<CreateReq>) -> Result<Json<Event>, ApiError> {
    let state = if r.state.is_null() { json!({}) } else { r.state };
    app.change(|s| s.create(&r.name, r.view, r.verbs, state, &by(r.by)?, r.replace, &r.about, &r.project))
}

#[derive(Deserialize)]
struct CallReq {
    verb: String,
    #[serde(default)]
    args: Value,
    by: Option<String>,
    /// The element the person used, sent by the runtime.
    #[serde(default)]
    source: Option<Value>,
}

async fn call(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<CallReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.call_from(&name, &r.verb, r.args, &by(r.by)?, r.source))
}

#[derive(Deserialize)]
struct SetReq {
    #[serde(default)]
    pointer: String,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    remove: bool,
    by: Option<String>,
}

async fn set_state(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<SetReq>) -> Result<Json<Event>, ApiError> {
    let value = match (r.remove, r.value) {
        (true, _) => None,
        (false, Some(v)) => Some(v),
        (false, None) => return Err(Error::Invalid("give a value, or remove: true".into()).into()),
    };
    app.change(|s| s.set_state(&name, &r.pointer, value, &by(r.by)?))
}

#[derive(Deserialize)]
struct ReshapeReq {
    view: Option<String>,
    verbs: Option<String>,
    about: Option<String>,
    by: Option<String>,
}

async fn reshape(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ReshapeReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.reshape(&name, r.view, r.verbs, r.about, &by(r.by)?))
}

#[derive(Deserialize)]
struct ByReq {
    by: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
}

async fn undo(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.undo(&name, &by(r.by)?))
}

async fn pin(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.pin(&name, r.pinned.unwrap_or(true), &by(r.by)?))
}

#[derive(Deserialize)]
struct CommentReq {
    #[serde(default)]
    anchor: Value,
    text: String,
    by: Option<String>,
}

async fn comment(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<CommentReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.comment(&name, r.anchor, &r.text, &by(r.by)?))
}

#[derive(Deserialize)]
struct ResolveReq {
    id: String,
    by: Option<String>,
}

async fn resolve(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ResolveReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.resolve(&name, &r.id, &by(r.by)?))
}

#[derive(Deserialize)]
struct ReplyReq {
    id: String,
    text: String,
    #[serde(default)]
    resolve: bool,
    by: Option<String>,
}

async fn reply(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ReplyReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.reply(&name, &r.id, &r.text, &by(r.by)?, r.resolve))
}

#[derive(Deserialize)]
struct ProjectReq {
    project: String,
    by: Option<String>,
}

async fn move_project(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ProjectReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_project(&name, &r.project, &by(r.by)?))
}

async fn remove(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<ByQ>) -> Result<Json<Event>, ApiError> {
    if name == PAGE {
        return Err(Error::Invalid("the jelly page cannot be removed; reshape or undo it instead".into()).into());
    }
    app.change(|s| s.remove(&name, &by(q.by)?))
}

#[derive(Deserialize)]
struct ByQ {
    by: Option<String>,
    jelly: Option<String>,
}

async fn events(State(app): State<Shared>, Query(q): Query<ByQ>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(app.events.subscribe()).filter_map(move |ev| {
        let ev = ev.ok()?;
        if q.jelly.as_ref().is_some_and(|j| *j != ev.jelly) {
            return None;
        }
        SseEvent::default().json_data(&ev).ok().map(Ok)
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize, Clone)]
struct WaitQ {
    since: u64,
    by: String,
    /// Resolve short names against the jellies present at the starting sequence.
    project_jelly: Option<String>,
    jelly: Option<String>,
    timeout: Option<u64>,
}

async fn wait(State(app): State<Shared>, Query(q): Query<WaitQ>) -> Result<Response, ApiError> {
    by(Some(q.by.clone()))?;
    match wait_since(&app, &q).await? {
        Some(ev) => Ok(Json(ev).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

fn history_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("could not read event history: {e}"))
}

fn recorded_change(reader: &mut impl BufRead, q: &WaitQ) -> Result<Option<Event>, Error> {
    let mut line = String::new();
    while reader.read_line(&mut line).map_err(history_error)? != 0 {
        let ev: Event = serde_json::from_str(&line).map_err(history_error)?;
        if ev.seq > q.since && ev.by != q.by
            && q.jelly.as_ref().map_or(ev.jelly != PAGE, |j| *j == ev.jelly)
        {
            return Ok(Some(ev));
        }
        line.clear();
    }
    Ok(None)
}

async fn wait_since(app: &App, q: &WaitQ) -> Result<Option<Event>, Error> {
    wait_since_at_handoff(app, q, |_| {}).await
}

async fn wait_since_at_handoff(app: &App, q: &WaitQ, after_scan: impl FnOnce(&mut Store)) -> Result<Option<Event>, Error> {
    let mut q = q.clone();
    let rx = app.events.subscribe();
    let deadline = q.timeout.map(|t| Instant::now().checked_add(Duration::from_secs(t))
        .ok_or_else(|| Error::Invalid("timeout is too large".into()))).transpose()?;
    let mut reader = {
        let mut store = app.store.lock().expect("store lock");
        let mut reader = BufReader::new(File::open(store.events_path()).map_err(history_error)?);
        if let Some(preferred) = &q.project_jelly {
            let (project, short) = preferred.split_once(':').unwrap_or(("", preferred));
            let mut identities = std::collections::BTreeSet::new();
            let mut line = String::new();
            while reader.read_line(&mut line).map_err(history_error)? != 0 {
                let ev: Event = serde_json::from_str(&line).map_err(history_error)?;
                if ev.seq > q.since { break; }
                if ev.kind == crate::store::EventKind::Move {
                    if let Some(from) = ev.args.as_ref().and_then(|args| args["from"].as_str()) {
                        identities.remove(from);
                    }
                }
                if ev.jelly.split_once(':').map_or(ev.jelly.as_str(), |(_, short)| short) == short {
                    if ev.kind == crate::store::EventKind::Remove {
                        identities.remove(&ev.jelly);
                    } else {
                        identities.insert(ev.jelly);
                    }
                }
                line.clear();
            }
            if !identities.is_empty() {
                q.jelly = Some(crate::store::resolve_name(short, project, identities.iter().map(String::as_str)));
            }
            std::io::Seek::rewind(&mut reader).map_err(history_error)?;
        }
        if let Some(ev) = recorded_change(&mut reader, &q)? {
            return Ok(Some(ev));
        }
        if let Some(name) = &q.jelly { store.get(name)?; }
        after_scan(&mut store);
        reader
    };
    wait_for_record(app, &q, &mut reader, rx, deadline).await
}

async fn wait_for_record(
    app: &App,
    q: &WaitQ,
    reader: &mut BufReader<File>,
    mut rx: broadcast::Receiver<Event>,
    deadline: Option<Instant>,
) -> Result<Option<Event>, Error> {
    loop {
        let notification = if let Some(end) = deadline {
            if Instant::now() >= end { return Ok(None); }
            match tokio::time::timeout_at(end, rx.recv()).await {
                Ok(result) => result,
                Err(_) => return Ok(None),
            }
        } else {
            rx.recv().await
        };
        // Commits are ordered under the store lock; broadcasts happen after it
        // is released, so notifications only wake the reader, never choose a result.
        let _store = app.store.lock().expect("store lock");
        if let Some(ev) = recorded_change(reader, q)? { return Ok(Some(ev)); }
        if matches!(notification, Err(broadcast::error::RecvError::Closed)) {
            return Err(Error::Invalid("the event stream ended".into()));
        }
    }
}

/// Browsers get events over one WebSocket per page. Server-sent events would
/// use an HTTP connection each, and a page with a few jellies in it would hit
/// the browser's six-connections-per-host limit and stall every request.
async fn ws(State(app): State<Shared>, up: WebSocketUpgrade) -> Response {
    let rx = app.events.subscribe();
    up.on_upgrade(move |socket| pump(socket, rx))
}

async fn pump(mut socket: WebSocket, mut rx: broadcast::Receiver<Event>) {
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let text = match ev {
                    Ok(ev) => serde_json::to_string(&ev).expect("an event serializes"),
                    // Missed events: tell the page to reload its state.
                    Err(broadcast::error::RecvError::Lagged(_)) => r#"{"kind":"resync"}"#.to_string(),
                    Err(_) => break,
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => if !matches!(msg, Some(Ok(_))) { break },
        }
    }
}

/// Serves a jelly's view with the runtime injected, so `window.jelly` exists
/// before any of the view's own scripts run.
async fn view(State(app): State<Shared>, Path(name): Path<String>) -> Result<Html<String>, ApiError> {
    let store = app.store.lock().expect("store lock");
    let j = store.get(&name)?;
    let boot = json!({"name": j.name, "about": j.about, "version": j.version, "viewRev": j.view_rev, "state": j.state, "comments": j.comments, "attention": j.attention, "working": j.working});
    // `</` cannot appear inside the script, or state text could close the tag.
    let boot = boot.to_string().replace("</", "<\\/");
    let inject = format!("<script>window.__JELLY__={boot};</script><script src=\"/runtime.js\"></script>");
    Ok(Html(inject_head(&j.view, &inject)))
}

/// Puts `inject` first thing in `<head>`, so it runs before the view's scripts.
fn inject_head(html: &str, inject: &str) -> String {
    let at = html.to_ascii_lowercase().find("<head>").map(|i| i + "<head>".len());
    match at {
        Some(i) => format!("{}{inject}{}", &html[..i], &html[i..]),
        None => format!("<!doctype html><meta charset=\"utf-8\">{inject}{html}"),
    }
}

async fn archive(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_archived(&name, true, &by(r.by)?))
}

async fn restore(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_archived(&name, false, &by(r.by)?))
}

#[derive(Deserialize)]
struct AssetQ {
    by: Option<String>,
}

async fn put_asset(State(app): State<Shared>, Path((name, file)): Path<(String, String)>, Query(q): Query<AssetQ>, body: Bytes) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.put_asset(&name, &file, &body, &by(q.by)?))
}

fn content_type(file: &str) -> &'static str {
    match file.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "glb" => "model/gltf-binary",
        "json" => "application/json",
        "csv" => "text/csv",
        "txt" | "md" => "text/plain; charset=utf-8",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "woff2" => "font/woff2",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

async fn asset(State(app): State<Shared>, Path((name, file)): Path<(String, String)>) -> Response {
    let dir = app.store.lock().expect("store lock").asset_dir(&name);
    if file.starts_with('.') || file.contains('/') || file.contains("..") {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(dir.join(&file)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(&file))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// One file you can send anyone: the view with its state baked in, assets
/// inlined, and a read-only `jelly` in place of the live runtime.
async fn share(State(app): State<Shared>, Path(name): Path<String>) -> Result<Html<String>, ApiError> {
    use base64::Engine as _;
    let (j, dir, files) = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(&name)?.clone();
        (j, store.asset_dir(&name), store.assets(&name))
    };
    let mut html = j.view.clone();
    for f in files {
        if let Ok(bytes) = std::fs::read(dir.join(&f)) {
            let uri = format!("data:{};base64,{}", content_type(&f), base64::engine::general_purpose::STANDARD.encode(bytes));
            html = html.replace(&format!("assets/{f}"), &uri);
        }
    }
    let boot = json!({"name": j.name, "about": j.about, "state": j.state}).to_string().replace("</", "<\\/");
    let inject = format!("<script>window.__JELLY__={boot};</script><script>{SNAPSHOT_JS}</script>");
    Ok(Html(inject_head(&html, &inject)))
}

#[derive(Deserialize)]
struct InboxQ {
    project: Option<String>,
    since: Option<u64>,
}

/// What the person did since `since`, for jellies in `project`: the events an
/// agent should hear about at the start of its next turn, each with a line of text.
async fn inbox(State(app): State<Shared>, Query(q): Query<InboxQ>) -> Json<Value> {
    let path = app.store.lock().expect("store lock").events_path();
    let since = q.since.unwrap_or(0);
    let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let prefix = q.project.filter(|p| !p.is_empty()).map(|p| format!("{p}:"));
    let items: Vec<Value> = text.lines()
        .filter_map(|l| serde_json::from_str::<Event>(l).ok())
        .filter(|e| e.at_ms > since && e.by == PERSON && e.jelly != PAGE && e.kind.is_for_agent())
        .filter(|e| prefix.as_deref().is_none_or(|p| e.jelly.starts_with(p)))
        .map(|e| { let text = e.describe(); let mut v = json!(e); v["text"] = json!(text); v })
        .collect();
    Json(json!(items))
}


#[derive(Deserialize)]
struct AttendReq {
    #[serde(flatten)]
    spec: Value,
}

async fn attend(State(app): State<Shared>, Path(name): Path<String>, headers: axum::http::HeaderMap, Json(r): Json<AttendReq>) -> Result<Json<Value>, ApiError> {
    let by = r.spec["by"].as_str().map(String::from);
    let asked = now_ms();
    let Json(ev) = app.change(|s| s.attend(&name, r.spec, &self::by(by)?))?;
    let mut out = json!(ev);
    // Tell the asker where the card really shows. The item is made either way,
    // and the event that listeners and the log get is unchanged.
    let args = ev.args.unwrap_or_default();
    if args["anchor"]["selector"].is_string() {
        match anchor_place(&app, port_of(&headers), &name, args["id"].as_str().unwrap_or(""), ev.version, asked).await {
            Ok(place) => out["anchor"] = json!(place),
            Err(why) => out["anchor_unchecked"] = json!(why),
        }
    }
    Ok(Json(out))
}

/// How long an ask waits for a browser before it answers "not checked".
const ANCHOR_CHECK: std::time::Duration = std::time::Duration::from_secs(8);

/// Where an open item's card lands, from a snapshot taken after it was asked
/// (one rendered now, or an open view's): "below" its anchor, or at the top
/// because the anchor is "missing" or "floating" (in a fixed or sticky part),
/// or below an anchor that is "hidden" right now. `Err` says why it could not
/// be checked.
async fn anchor_place(app: &App, port: u16, name: &str, item: &str, version: u64, asked: u64) -> Result<&'static str, String> {
    let render = render(port, name, 1280, 900, None, ANCHOR_CHECK);
    tokio::pin!(render);
    // The browser often posts its snapshot and then lingers, so answer as soon
    // as the snapshot is in; dropping the render stops the browser.
    let mut rendered: Option<(Result<(), String>, std::time::Instant)> = None;
    loop {
        // Screens are kept by name, so one from before the ask may belong to
        // a jelly this one replaced: only a snapshot taken since counts.
        let snap = app.screens.lock().expect("screens lock").get(name).filter(|s| s.version >= version && s.at_ms >= asked).cloned();
        if let Some(s) = snap {
            let lists = |l: &[String]| l.iter().any(|m| m.split_once(' ').is_some_and(|(id, _)| id == item));
            return Ok(if lists(&s.missing) { "missing" } else if lists(&s.floating) { "floating" } else if lists(&s.hidden) { "hidden" } else { "below" });
        }
        match &rendered {
            // The browser is gone: give its snapshot a moment to land, then give up.
            Some((r, at)) if at.elapsed() > std::time::Duration::from_secs(2) => {
                return Err(r.clone().err().unwrap_or_else(|| "the page rendered but sent no snapshot".into()));
            }
            Some(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            None => tokio::select! {
                r = &mut render => rendered = Some((r, std::time::Instant::now())),
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            },
        }
    }
}

/// The port this request came in on, for rendering our own pages.
fn port_of(headers: &axum::http::HeaderMap) -> u16 {
    headers.get(header::HOST).and_then(|h| h.to_str().ok()).and_then(|h| h.rsplit(':').next()).and_then(|p| p.parse().ok()).unwrap_or(7878)
}

#[derive(Deserialize)]
struct AnswerReq {
    option: Option<String>,
    #[serde(default)]
    text: String,
    by: Option<String>,
}

async fn answer(State(app): State<Shared>, Path((name, id)): Path<(String, String)>, Json(r): Json<AnswerReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.answer(&name, &id, r.option.as_deref(), &r.text, &by(r.by)?))
}

async fn withdraw(State(app): State<Shared>, Path((name, id)): Path<(String, String)>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.withdraw(&name, &id, &by(r.by)?))
}

#[derive(Deserialize)]
struct WorkingReq {
    what: Option<String>,
    by: Option<String>,
}

async fn working(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<WorkingReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_working(&name, r.what.as_deref().filter(|w| !w.trim().is_empty()), &by(r.by)?))
}

#[derive(Deserialize)]
struct FocusReq {
    name: Option<String>,
    by: Option<String>,
}

/// Opens a jelly full size on the person's screen (or closes it with no name).
async fn focus(State(app): State<Shared>, Json(r): Json<FocusReq>) -> Result<Json<Event>, ApiError> {
    if let Some(n) = &r.name {
        app.store.lock().expect("store lock").get(n)?;
    }
    app.change(|s| s.call(PAGE, "focus", json!({"name": r.name}), &by(r.by)?))
}

#[derive(Deserialize)]
struct ProjectQ {
    project: Option<String>,
    /// List answered items instead of open ones: the decision record.
    #[serde(default)]
    answered: bool,
}

/// Everything open for the person, newest first; with `answered=true`, every
/// answered item instead: what was asked, what was picked, by whom, and when.
async fn attention_list(State(app): State<Shared>, Query(q): Query<ProjectQ>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    let project = q.project.filter(|p| !p.is_empty());
    let mut items: Vec<Value> = Vec::new();
    for j in store.list().filter(|j| j.name != PAGE && project.as_deref().is_none_or(|p| j.project() == p)) {
        for a in j.attention.iter().filter(|a| if q.answered { a.answer.is_some() } else { a.is_open() }) {
            let mut v = serde_json::to_value(a).expect("serializes");
            v["jelly"] = json!(j.name);
            v["about"] = json!(j.about);
            items.push(v);
        }
    }
    let when = |i: &Value| i["answer"]["at_ms"].as_u64().or(i["at_ms"].as_u64()).unwrap_or(0);
    items.sort_by_key(|i| std::cmp::Reverse(when(i)));
    Json(json!(items))
}

// ---- the agent view -------------------------------------------------------

/// What a browser shows of a jelly, as text, posted by the runtime for `jelly see`.
#[derive(Deserialize, Clone)]
struct Screen {
    text: String,
    width: u32,
    height: u32,
    version: u64,
    #[serde(default)]
    at_ms: u64,
    /// Errors the page threw or logged.
    #[serde(default)]
    errors: Vec<String>,
    /// Open questions whose anchor matches nothing: "a2 #f2".
    #[serde(default)]
    missing: Vec<String>,
    /// Open questions whose anchor is in a fixed or sticky part of the page,
    /// so their card shows at the top too.
    #[serde(default)]
    floating: Vec<String>,
    /// Open questions whose anchor is on the page but not shown right now.
    #[serde(default)]
    hidden: Vec<String>,
    #[serde(default)]
    page_height: u32,
}

async fn put_screen(State(app): State<Shared>, Path(name): Path<String>, Json(mut sc): Json<Screen>) -> StatusCode {
    sc.at_ms = now_ms();
    app.screens.lock().expect("screens lock").insert(name, sc);
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct NoteReq {
    text: String,
    by: Option<String>,
}

async fn note(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<NoteReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.note(&name, &r.text, &by(r.by)?))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A browser the daemon can drive headless: JELLY_CHROME, or the usual installs.
fn chrome() -> Option<String> {
    let candidates = [
        std::env::var("JELLY_CHROME").unwrap_or_default(),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into(),
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".into(),
        "/usr/bin/google-chrome".into(),
        "/usr/bin/chromium".into(),
        "/usr/bin/chromium-browser".into(),
    ];
    candidates.into_iter().find(|c| !c.is_empty() && std::path::Path::new(c).exists())
}

/// How long `jelly see` waits for the browser.
const RENDER_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Runs headless Chrome on a jelly at a given size. With `png`, writes a
/// screenshot there; either way the page's runtime posts a fresh snapshot.
async fn render(port: u16, name: &str, width: u32, height: u32, png: Option<&std::path::Path>, limit: std::time::Duration) -> Result<(), String> {
    let bin = chrome().ok_or("no Chrome, Chromium, Edge or Brave found; set JELLY_CHROME to a browser binary")?;
    // Headless Chrome will not make a window narrower than 500px, so phone
    // widths render the jelly in an iframe of exactly that size.
    let url = if width < 500 {
        format!("http://127.0.0.1:{port}/see-frame/{name}?w={width}&h={height}")
    } else {
        format!("http://127.0.0.1:{port}/j/{name}/?agent=1")
    };
    // The phone frame is scaled up to 500px wide, so the window grows with it.
    let height = if width < 500 { (height as f64 * 500.0 / width.max(1) as f64).ceil() as u32 } else { height };
    let width = width.max(500);
    let profile = std::env::temp_dir().join("jelly-chrome");
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run", "--no-default-browser-check", "--mute-audio"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={width},{height}"))
        .arg("--virtual-time-budget=2500")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match png {
        Some(p) => { cmd.arg(format!("--screenshot={}", p.display())); }
        None => { cmd.arg("--dump-dom"); }
    }
    let run = cmd.arg(url).status();
    match tokio::time::timeout(limit, run).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(format!("could not run the browser: {e}")),
        Err(_) => Err(format!("the browser took longer than {} s", limit.as_secs())),
    }
}

#[derive(Deserialize)]
struct SeeQ {
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    png: bool,
    /// The whole page, not just the first screen.
    #[serde(default)]
    full: bool,
}

/// Everything an agent needs to pick up a jelly, as plain text: what it is,
/// what the person did, what waits on them, and what is on screen.
async fn see(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<SeeQ>, req_headers: axum::http::HeaderMap) -> Result<Response, ApiError> {
    let (j, home) = {
        let store = app.store.lock().expect("store lock");
        (store.get(&name)?.clone(), store.home().to_path_buf())
    };
    let port = port_of(&req_headers);
    let width = q.width.unwrap_or(1280);
    let height = q.height.unwrap_or(if width <= 500 { 844 } else { 900 });
    // Use a snapshot from an open view if it is current and the right width; otherwise render one.
    // An open view's screen counts if it shows this version at the width asked for (1280 by default).
    let fresh = |app: &App| app.screens.lock().expect("screens lock").get(&name).cloned()
        .filter(|s| s.version == j.version && s.width == width);
    let mut notes: Vec<String> = Vec::new();
    let mut snap = fresh(&app);
    if snap.is_none() {
        let asked = now_ms();
        match render(port, &name, width, height, None, RENDER_LIMIT).await {
            Ok(()) => {
                for _ in 0..30 {
                    if let Some(s) = app.screens.lock().expect("screens lock").get(&name).filter(|s| s.at_ms >= asked) {
                        snap = Some(s.clone());
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                if snap.is_none() { notes.push("The page rendered but sent no snapshot.".into()); }
            }
            Err(e) => notes.push(format!("No snapshot: {e}")),
        }
    }
    let png = if q.png {
        let dir = home.join("see");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}-{width}.png", name.replace(':', "_")));
        // The full page: as tall as the page said it was, within reason.
        let h = if q.full { snap.as_ref().map(|s| s.page_height).filter(|h| *h > height).unwrap_or(height).min(12000) } else { height };
        match render(port, &name, width, h, Some(&path), RENDER_LIMIT).await {
            Ok(()) => Some(path),
            Err(e) => { notes.push(format!("No screenshot: {e}")); None }
        }
    } else { None };

    let mut out = String::new();
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    let history: Vec<Event> = std::fs::read_to_string(home.join("events.jsonl")).unwrap_or_default().lines()
        .filter(|l| l.contains(&format!("\"jelly\":\"{name}\""))).filter_map(|l| serde_json::from_str(l).ok()).collect();
    let last = history.last().map(|e| format!(" · last change {} by {}", ago(e.at_ms), e.by)).unwrap_or_default();
    line(&mut out, format!("{} · {} · v{} · made by {}{last}{}{}", j.name, if j.about.is_empty() { "(no about line)" } else { &j.about }, j.version, j.made_by,
        if j.pinned { " · pinned" } else { "" }, if j.archived { " · archived" } else { "" }));
    if let Some(w) = &j.working { line(&mut out, format!("working  {} is working on it: {}", w["by"].as_str().unwrap_or("?"), w["what"].as_str().unwrap_or(""))); }
    for n in &j.notes { line(&mut out, format!("note     {}: {}", n["by"].as_str().unwrap_or("?"), n["text"].as_str().unwrap_or(""))); }
    let verbs = crate::verbs::check(&j.verbs).unwrap_or_default();
    line(&mut out, format!("verbs    {}", if verbs.is_empty() { "none (read-only)".to_string() } else { verbs.iter().map(|v| verb_sig(&j.verbs, v)).collect::<Vec<_>>().join(" · ") }));
    line(&mut out, format!("state    {}", shape(&j.state)));
    let assets = app.store.lock().expect("store lock").assets(&name);
    if !assets.is_empty() { line(&mut out, format!("assets   {}", assets.join(", "))); }
    for a in j.attention.iter().filter(|a| a.is_open()) {
        let opts = a.options.iter().map(|o| format!("{}{}", o.id, if a.recommend.as_deref() == Some(o.id.as_str()) { "★" } else { "" })).collect::<Vec<_>>().join("/");
        let kind = if a.kind == AttentionKind::Look { "look" } else { "ask" };
        line(&mut out, format!("waits    {} {kind} {:?}{}{}", a.id, a.title, if opts.is_empty() { String::new() } else { format!(" [{opts}]") }, a.anchor["selector"].as_str().map(|s| format!(" at {s}")).unwrap_or_default()));
    }
    for a in j.attention.iter().filter(|a| a.answer.is_some()).rev().take(5) {
        let ans = a.answer.as_ref().expect("filtered");
        let pick = ans.option.as_deref().unwrap_or(if a.kind == AttentionKind::Look { "seen" } else { "" });
        line(&mut out, format!("answered {} {:?} → {pick}{}", a.id, a.title, if ans.text.is_empty() { String::new() } else { format!(" {:?}", ans.text) }));
    }
    for f in j.comments.iter().filter(|f| !f.resolved) {
        line(&mut out, format!("comment  {} on {}: {:?}{}", f.id, f.anchor["selector"].as_str().unwrap_or("the page"), f.text, if f.replies.is_empty() { "" } else { " (thread)" }));
    }
    let mine: Vec<&Event> = history.iter().filter(|e| e.by == PERSON).collect();
    for e in mine.iter().rev().take(8).rev() {
        line(&mut out, format!("person   {} {}", ago(e.at_ms), e.describe()));
    }
    let project = j.project();
    if !project.is_empty() && app.store.lock().expect("store lock").project_dir(project).join("DESIGN.md").exists() {
        line(&mut out, format!("design   project {project} has a DESIGN.md: jelly design · CSS variables at /p/{project}/design.css"));
    }
    if let Some(p) = &png { line(&mut out, format!("png      {}", p.display())); }
    if let Some(sc) = &snap {
        for e in &sc.errors { line(&mut out, format!("error    {e}")); }
        for m in &sc.missing {
            let (id, sel) = m.split_once(' ').unwrap_or((m, ""));
            line(&mut out, format!("!        {id} asks about {sel}, which matches nothing on the page; it shows at the top instead"));
        }
        for m in &sc.floating {
            let (id, sel) = m.split_once(' ').unwrap_or((m, ""));
            line(&mut out, format!("!        {id} asks about {sel}, which is in a fixed or sticky part of the page; it shows at the top instead"));
        }
        if sc.page_height > sc.height && !q.full && png.is_some() { line(&mut out, format!("!        the page is {}px tall and the png shows the first {}px; --full for all of it", sc.page_height, sc.height)); }
    }
    for n in notes { line(&mut out, format!("!        {n}")); }
    if let Some(s) = snap {
        line(&mut out, format!("\non screen at {}×{} (v{}):", s.width, s.height, s.version));
        out.push_str(&s.text);
        out.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], out).into_response())
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

fn ago(ms: u64) -> String {
    let s = now_ms().saturating_sub(ms) / 1000;
    match s { 0..=59 => "just now".into(), 60..=3599 => format!("{}m ago", s / 60), 3600..=86399 => format!("{}h ago", s / 3600), _ => format!("{}d ago", s / 86400) }
}

/// `pick({id, value})`: the verb's name and its second parameter, which holds
/// the args (destructured keys, or the parameter's name).
fn verb_sig(src: &str, name: &str) -> String {
    let Some(at) = src.find(&format!("{name}(")) else { return name.into() };
    let rest = &src[at + name.len() + 1..];
    // Split the parameter list on top-level commas.
    let (mut depth, mut params, mut cur) = (0i32, Vec::new(), String::new());
    for c in rest.chars() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' if depth == 0 => break,
            ')' | '}' | ']' => depth -= 1,
            ',' if depth == 0 => { params.push(std::mem::take(&mut cur)); continue; }
            _ => {}
        }
        cur.push(c);
    }
    params.push(cur);
    let Some(args) = params.get(1).map(|p| p.trim()) else { return name.into() };
    match args.strip_prefix('{').and_then(|a| a.rsplit_once('}')).map(|(k, _)| k) {
        Some(keys) => {
            let keys: Vec<&str> = keys.split(',').map(|x| x.split([':', '=']).next().unwrap_or("").trim()).filter(|x| !x.is_empty()).collect();
            format!("{name}({{{}}})", keys.join(", "))
        }
        None if !args.is_empty() => format!("{name}({args})"),
        None => name.into(),
    }
}

/// A one-line sketch of state: top-level keys with their kind and size.
fn shape(v: &Value) -> String {
    match v {
        Value::Object(m) if m.is_empty() => "{}".into(),
        Value::Object(m) => m.iter().take(12).map(|(k, v)| format!("{k}: {}", match v {
            Value::Array(a) => format!("[{}]", a.len()),
            Value::Object(o) => format!("{{{} keys}}", o.len()),
            Value::String(s) => format!("{:?}", clip(s, 30)),
            other => clip(&other.to_string(), 30),
        })).collect::<Vec<_>>().join(" · "),
        other => clip(&other.to_string(), 80),
    }
}

// ---- DESIGN.md per project --------------------------------------------------

async fn put_design(State(app): State<Shared>, Path(project): Path<String>, body: String) -> Result<StatusCode, ApiError> {
    let dir = app.store.lock().expect("store lock").project_dir(&project);
    std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("DESIGN.md"), body)).map_err(|e| Error::Invalid(format!("could not save DESIGN.md: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_design(State(app): State<Shared>, Path(project): Path<String>) -> Response {
    let path = app.store.lock().expect("store lock").project_dir(&project).join("DESIGN.md");
    match std::fs::read_to_string(path) {
        Ok(t) => ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], t).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, format!("project {project} has no DESIGN.md")).into_response(),
    }
}

/// The DESIGN.md tokens as CSS variables: `--color-primary`, `--rounded-md`,
/// `--spacing-sm`, `--typography-h1-font-size`, …
async fn design_css(State(app): State<Shared>, Path(project): Path<String>) -> Response {
    let path = app.store.lock().expect("store lock").project_dir(&project).join("DESIGN.md");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let css = format!(":root {{\n{}}}\n", crate::design::css_vars(&text));
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], css).into_response()
}

#[derive(Deserialize)]
struct FrameQ {
    w: u32,
    h: u32,
}

async fn see_frame(Path(name): Path<String>, Query(q): Query<FrameQ>) -> Html<String> {
    // Scaled up to fill the 500px window, so the screenshot is the phone layout and nothing else.
    let k = 500.0 / q.w.max(1) as f64;
    Html(format!("<!doctype html><body style=\"margin:0;overflow:hidden\"><iframe src=\"/j/{name}/?agent=1\" style=\"display:block;border:0;width:{w}px;height:{h}px;background:#fff;transform:scale({k});transform-origin:0 0\"></iframe></body>", w = q.w, h = q.h))
}

#[derive(Deserialize)]
struct AskReq {
    spec: Value,
    #[serde(default)]
    project: String,
    by: Option<String>,
}

/// A round of questions on an ask page, made or updated in one step.
async fn ask(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<AskReq>) -> Result<Json<Value>, ApiError> {
    let who = by(r.by)?;
    app.change_many(|s| s.ask_round(&name, &r.spec, ASK_VIEW, ASK_VERBS, &r.project, &who))
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    fn fixture(capacity: usize) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        for name in ["j", "other", PAGE] {
            store.create(name, "".into(), "const verbs = {};".into(), json!({}), "agent", false, "", "").unwrap();
        }
        let (events, _) = broadcast::channel(capacity);
        (dir, App { store: Mutex::new(store), events, screens: Mutex::default() })
    }

    fn query(jelly: Option<&str>, since: u64) -> WaitQ {
        WaitQ { since, by: "agent".into(), project_jelly: None, jelly: jelly.map(String::from), timeout: Some(0) }
    }

    fn note(app: &App, jelly: &str, who: &str) -> Event {
        app.change(|s| s.note(jelly, "changed", who)).unwrap().0
    }

    #[tokio::test]
    async fn history_is_exclusive_ordered_and_filtered_by_author_and_jelly() {
        let (_dir, app) = fixture(16);
        let start = note(&app, "j", "person");
        note(&app, "j", "agent");
        let surface = note(&app, PAGE, "person");
        let other = note(&app, "other", "helper");
        let first = note(&app, "j", "helper");
        note(&app, "j", "person");
        assert_eq!(wait_since(&app, &query(Some("j"), start.seq)).await.unwrap().unwrap().seq, first.seq);
        assert_eq!(wait_since(&app, &query(None, start.seq)).await.unwrap().unwrap().seq, other.seq);
        assert_eq!(wait_since(&app, &query(Some(PAGE), start.seq)).await.unwrap().unwrap().seq, surface.seq);
        assert!(wait_since(&app, &query(None, u64::MAX)).await.unwrap().is_none());
        assert!(matches!(wait_since(&app, &query(Some("missing"), 0)).await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn history_survives_replacement_removal_and_restart() {
        let (dir, mut app) = fixture(16);
        let start = note(&app, "j", "agent");
        let replacement = app.change(|s| s.create("j", "".into(), "const verbs = {};".into(), json!({}), "person", true, "", "")).unwrap().0;
        assert!(replacement.version < start.version);
        let removed = app.change(|s| s.remove("j", "person")).unwrap().0;
        app.store = Mutex::new(Store::open(dir.path()).unwrap());
        assert_eq!(wait_since(&app, &query(Some("j"), start.seq)).await.unwrap().unwrap().seq, replacement.seq);
        assert_eq!(wait_since(&app, &query(Some("j"), replacement.seq)).await.unwrap().unwrap().seq, removed.seq);
    }

    #[tokio::test]
    async fn eof_handoff_and_lag_recover_the_first_committed_answer() {
        for lag in [false, true] {
            let (dir, mut app) = fixture(1);
            let ask = app.change(|s| s.attend("j", json!({"title": "Which?"}), "agent")).unwrap().0;
            let q = WaitQ { timeout: Some(1), ..query(Some("j"), ask.seq) };
            let answer = std::cell::RefCell::new(None);
            let result = wait_since_at_handoff(&app, &q, |store| {
                let ev = store.answer("j", "a1", None, "yes", "person").unwrap();
                let _ = app.events.send(ev.clone());
                answer.replace(Some(ev));
                if lag {
                    for _ in 0..4 {
                        let ev = store.note("j", "changed", "agent").unwrap();
                        let _ = app.events.send(ev);
                    }
                }
            }).await.unwrap().unwrap();
            let answer = answer.into_inner().unwrap();
            assert_eq!(result.seq, answer.seq);
            assert_eq!(result.args.unwrap()["text"], "yes");
            assert!(result.state.is_none());
            app.store = Mutex::new(Store::open(dir.path()).unwrap());
            assert_eq!(wait_since(&app, &q).await.unwrap().unwrap().seq, answer.seq);
        }
    }

    #[tokio::test]
    async fn notification_order_does_not_override_commit_order() {
        let (_dir, app) = fixture(4);
        let rx = app.events.subscribe();
        let q = query(Some("j"), 0);
        let mut reader = BufReader::new(File::open(app.store.lock().unwrap().events_path()).unwrap());
        assert!(recorded_change(&mut reader, &q).unwrap().is_none());
        let (first, second) = {
            let mut s = app.store.lock().unwrap();
            (s.note("j", "first", "helper").unwrap(), s.note("j", "second", "person").unwrap())
        };
        app.events.send(second).unwrap();
        let result = wait_for_record(&app, &q, &mut reader, rx, None).await.unwrap().unwrap();
        assert_eq!(result.seq, first.seq);
    }

    #[tokio::test]
    async fn ignored_activity_stays_pending_until_an_eligible_change() {
        let (_dir, app) = fixture(8);
        let mut q = query(Some("j"), 0);
        q.timeout = None;
        let future = wait_since(&app, &q);
        tokio::pin!(future);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut future).await.is_err());
        note(&app, "j", "agent");
        note(&app, "other", "person");
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut future).await.is_err());
        let expected = note(&app, "j", "helper");
        assert_eq!(tokio::time::timeout(Duration::from_secs(1), future).await.unwrap().unwrap().unwrap().seq, expected.seq);
    }

    #[tokio::test]
    async fn ignored_notifications_do_not_extend_the_deadline() {
        let (_dir, app) = fixture(8);
        let q = WaitQ { timeout: Some(1), ..query(Some("j"), 0) };
        let producer = async {
            loop {
                note(&app, "j", "agent");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let start = Instant::now();
        tokio::select! {
            result = wait_since(&app, &q) => assert!(result.unwrap().is_none()),
            _ = producer => unreachable!(),
            _ = tokio::time::sleep(Duration::from_secs(3)) => panic!("timeout was extended"),
        }
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn ask_summary_keeps_its_batch_sequence_after_a_person_answers() {
        let (_dir, app) = fixture(8);
        let summary = app.change_many(|s| s.ask_round("sharing", &json!({"question": "Which?"}), ASK_VIEW, ASK_VERBS, "", "agent")).unwrap().0;
        let seq = summary["seq"].as_u64().unwrap();
        let answer = app.change(|s| s.answer("sharing", "a1", None, "quiet", "person")).unwrap().0;
        assert!(answer.seq > seq);
        let mut reader = BufReader::new(File::open(app.store.lock().unwrap().events_path()).unwrap());
        let q = WaitQ { by: "person".into(), ..query(Some("sharing"), seq - 1) };
        let last = recorded_change(&mut reader, &q).unwrap().unwrap();
        assert_eq!(last.seq, seq);
        assert_eq!(last.kind, crate::store::EventKind::Attention);
        assert_eq!(summary["state"]["round"], 1);
    }

    #[test]
    fn log_failures_leave_answers_and_removals_uncommitted() {
        for failure in ["directory", "readonly", "partial"] {
            let (dir, app) = fixture(8);
            let _ = app.change(|s| s.attend("j", json!({"title": "Which?"}), "agent")).unwrap();
            let mut rx = app.events.subscribe();
            let path = dir.path().join("events.jsonl");
            let original = std::fs::read(&path).unwrap();
            let retained = dir.path().join("retained.jsonl");
            match failure {
                "directory" => {
                    std::fs::rename(&path, &retained).unwrap();
                    std::fs::create_dir(&path).unwrap();
                }
                "readonly" => {
                    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
                    permissions.set_readonly(true);
                    std::fs::set_permissions(&path, permissions).unwrap();
                }
                "partial" => {
                    use std::io::Write;
                    std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"seq\":").unwrap();
                }
                _ => unreachable!(),
            }
            for remove in [false, true] {
                let result = if remove {
                    app.change(|s| s.remove("j", "person"))
                } else {
                    app.change(|s| s.answer("j", "a1", None, "not accepted", "person"))
                };
                let error = result.unwrap_err().0.to_string();
                assert!(error.contains("could not record event"), "{error}");
                assert!(rx.try_recv().is_err(), "failed changes must not be broadcast as accepted");
                let reopened = Store::open(dir.path()).unwrap();
                assert!(reopened.get("j").unwrap().attention[0].answer.is_none());
                assert!(app.store.lock().unwrap().get("j").unwrap().attention[0].answer.is_none());
            }
            if failure == "directory" {
                std::fs::remove_dir(&path).unwrap();
                std::fs::rename(retained, &path).unwrap();
            }
            std::fs::remove_file(&path).unwrap();
            std::fs::write(&path, &original).unwrap();
            let recovered = note(&app, "other", "person");
            let mut reader = BufReader::new(File::open(path).unwrap());
            assert_eq!(recorded_change(&mut reader, &query(Some("other"), 0)).unwrap().unwrap().seq, recovered.seq);
        }
    }

    #[tokio::test]
    async fn partial_batches_publish_every_record_before_the_error() {
        let spec = json!({"questions": [{"question": "First?"}, {"question": "Second?"}]});
        for existing in [false, true] {
            let count = if existing { 6 } else { 4 };
            for successful in 0..count {
                let (_dir, app) = fixture(16);
                if existing {
                    let _ = app.change_many(|s| s.ask_round("sharing", &spec, ASK_VIEW, ASK_VERBS, "", "agent")).unwrap();
                    let _ = app.change(|s| s.reshape("sharing", Some("different view".into()), None, None, "agent")).unwrap();
                }
                let path = app.store.lock().unwrap().events_path();
                let head = std::fs::read_to_string(&path).unwrap().lines().last()
                    .map(|line| serde_json::from_str::<Event>(line).unwrap().seq).unwrap();
                app.store.lock().unwrap().log_failure = Some((head + successful + 1, 10));
                let mut rx = app.events.subscribe();
                let q = WaitQ { by: "person".into(), timeout: Some(1), ..query(None, head) };
                let waiting = wait_since(&app, &q);
                tokio::pin!(waiting);
                std::future::poll_fn(|cx| {
                    assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
                    std::task::Poll::Ready(())
                }).await;
                let error = app.change_many(|s| s.ask_round("sharing", &spec, ASK_VIEW, ASK_VERBS, "", "agent")).unwrap_err();
                let response = error.into_response();
                let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                let message = body["error"].as_str().unwrap();
                assert!(message.contains("could not record event"), "{message}");
                if successful > 0 {
                    assert!(message.contains(&format!("committed {successful} changes to sharing through seq:{}", head + successful)), "{message}");
                    assert!(message.contains("jelly show sharing"), "{message}");
                    assert!(message.contains("before retrying"), "{message}");
                } else {
                    assert!(!message.contains("committed"), "{message}");
                }
                for seq in head + 1..=head + successful {
                    assert_eq!(rx.try_recv().unwrap().seq, seq);
                }
                assert!(rx.try_recv().is_err());
                if successful > 0 {
                    let event = waiting.await.unwrap().unwrap();
                    assert_eq!(event.seq, head + 1);
                    assert!(event.state.is_none());
                }
                let history = std::fs::read_to_string(path).unwrap();
                assert_eq!(history.lines().last().map(|line| serde_json::from_str::<Event>(line).unwrap().seq), Some(head + successful));
            }
        }
    }

    #[tokio::test]
    async fn a_partial_append_restores_history_and_the_unanswered_item() {
        let (dir, app) = fixture(8);
        let ask = app.change(|s| s.attend("j", json!({"title": "Which?"}), "agent")).unwrap().0;
        let path = app.store.lock().unwrap().events_path();
        let original = std::fs::read(&path).unwrap();
        app.store.lock().unwrap().log_failure = Some((ask.seq + 1, 10));
        assert!(app.change(|s| s.answer("j", "a1", None, "yes", "person")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(app.store.lock().unwrap().get("j").unwrap().attention[0].answer.is_none());
        assert!(Store::open(dir.path()).unwrap().get("j").unwrap().attention[0].answer.is_none());
        app.store.lock().unwrap().log_failure = None;
        let answer = app.change(|s| s.answer("j", "a1", None, "yes", "person")).unwrap().0;
        assert_eq!(wait_since(&app, &query(Some("j"), ask.seq)).await.unwrap().unwrap().seq, answer.seq);
    }

    #[tokio::test]
    async fn unreadable_or_malformed_history_is_an_error() {
        let (_dir, app) = fixture(4);
        let path = app.store.lock().unwrap().events_path();
        std::fs::write(&path, "invalid json\n").unwrap();
        assert!(wait_since(&app, &query(None, 0)).await.unwrap_err().to_string().contains("event history"));
        std::fs::remove_file(path).unwrap();
        assert!(wait_since(&app, &query(None, 0)).await.is_err());
    }
}
