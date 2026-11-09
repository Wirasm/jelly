//! The local daemon: an HTTP API over the store, a live event stream, and the
//! views. The surface people look at is itself a jelly named `surface`.

use crate::store::{Error, Event, Store};
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
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

const RUNTIME_JS: &str = include_str!("../assets/runtime.js");
const SURFACE_VIEW: &str = include_str!("../assets/surface.html");
const SURFACE_VERBS: &str = include_str!("../assets/surface.verbs.js");
const SNAPSHOT_JS: &str = include_str!("../assets/snapshot.js");
const ARCHIVE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;

struct App {
    store: Mutex<Store>,
    events: broadcast::Sender<Event>,
}
type Shared = Arc<App>;

impl App {
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
    if store.get("surface").is_err() {
        store
            .create("surface", SURFACE_VIEW.into(), SURFACE_VERBS.into(), json!({"layout": {}}), "jelly", false, "Where every jelly lives", "")
            .and_then(|_| store.pin("surface", true, "jelly"))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    let (tx, _) = broadcast::channel(1024);
    let app: Shared = Arc::new(App { store: Mutex::new(store), events: tx });

    // Unpinned jellies nobody touched for a week are archived: hidden from the
    // surface, restorable, and back on their own as soon as anything changes them.
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
        .route("/", get(|| async { Redirect::to("/j/surface") }))
        .route("/runtime.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], RUNTIME_JS) }))
        // Views live at /j/<name>/ so a view can use relative `assets/<file>` paths.
        .route("/j/{name}", get(|Path(name): Path<String>| async move { Redirect::permanent(&format!("/j/{name}/")) }))
        .route("/j/{name}/", get(view))
        .route("/j/{name}/assets/{file}", get(asset))
        // A project's page is the surface, showing only that project's jellies.
        .route("/p/{project}", get(|st: State<Shared>| async move { view(st, Path("surface".into())).await }))
        .route("/api/events", get(events))
        .route("/api/ws", get(ws))
        .route("/api/jellies", get(list).post(create))
        .route("/api/jellies/{name}", get(show).delete(remove))
        .route("/api/jellies/{name}/state", get(state))
        .route("/api/jellies/{name}/call", post(call))
        .route("/api/jellies/{name}/reshape", post(reshape))
        .route("/api/jellies/{name}/undo", post(undo))
        .route("/api/jellies/{name}/pin", post(pin))
        .route("/api/jellies/{name}/project", post(project))
        .route("/api/jellies/{name}/restore", post(restore))
        .route("/api/jellies/{name}/assets/{file}", post(put_asset).layer(DefaultBodyLimit::max(25 * 1024 * 1024)))
        .route("/api/jellies/{name}/snapshot", get(snapshot))
        .route("/api/inbox", get(inbox))
        .route("/api/attention", get(attention_list))
        .route("/api/focus", post(focus))
        .route("/api/jellies/{name}/attention", post(attend))
        .route("/api/jellies/{name}/attention/{id}/answer", post(answer))
        .route("/api/jellies/{name}/attention/{id}/withdraw", post(withdraw))
        .route("/api/jellies/{name}/working", post(working))
        .route("/api/jellies/{name}/comment", post(comment))
        .route("/api/jellies/{name}/resolve", post(resolve))
        .route("/api/jellies/{name}/reply", post(reply))
        .with_state(app);

    // Loopback only: anything that reaches this port can run verbs.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("jelly surface on http://127.0.0.1:{port}");
    axum::serve(listener, router).await
}

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

fn by(by: Option<String>) -> String {
    by.unwrap_or_else(|| "person".into())
}

async fn list(State(app): State<Shared>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    // The page that shows the jellies is internal; it is not one of yours.
    Json(json!(store
        .list()
        .filter(|j| j.name != "surface")
        .map(|j| json!({
            "name": j.name, "version": j.version, "view_rev": j.view_rev,
            "pinned": j.pinned, "made_by": j.made_by, "updated_ms": j.updated_ms, "about": j.about, "project": j.project(), "archived": j.archived,
            "created_ms": if j.created_ms > 0 { j.created_ms } else { j.updated_ms },
            "open_comments": j.feedback.iter().filter(|f| !f.resolved).count(),
            // Threads where the agent answered and the person has not: the agent may be asking.
            "answered_comments": j.feedback.iter().filter(|f| !f.resolved && f.replies.last().is_some_and(|r| r.by != "person")).count(),
            "decision": is_decision(&j.state).then(|| j.state.get("status").cloned()).flatten(),
            "attention": j.attention.iter().filter(|a| a.answer.is_none()).count(),
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
    app.change(|s| s.create(&r.name, r.view, r.verbs, state, &by(r.by), r.replace, &r.about, &r.project))
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
    app.change(|s| s.call_from(&name, &r.verb, r.args, &by(r.by), r.source))
}

#[derive(Deserialize)]
struct ReshapeReq {
    view: Option<String>,
    verbs: Option<String>,
    about: Option<String>,
    by: Option<String>,
}

async fn reshape(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ReshapeReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.reshape(&name, r.view, r.verbs, r.about, &by(r.by)))
}

#[derive(Deserialize)]
struct ByReq {
    by: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
}

async fn undo(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.undo(&name, &by(r.by)))
}

async fn pin(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.pin(&name, r.pinned.unwrap_or(true), &by(r.by)))
}

#[derive(Deserialize)]
struct CommentReq {
    #[serde(default)]
    anchor: Value,
    text: String,
    by: Option<String>,
}

async fn comment(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<CommentReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.comment(&name, r.anchor, &r.text, &by(r.by)))
}

#[derive(Deserialize)]
struct ResolveReq {
    id: String,
    by: Option<String>,
}

async fn resolve(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ResolveReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.resolve(&name, &r.id, &by(r.by)))
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
    app.change(|s| s.reply(&name, &r.id, &r.text, &by(r.by), r.resolve))
}

#[derive(Deserialize)]
struct ProjectReq {
    project: String,
    by: Option<String>,
}

async fn project(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ProjectReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_project(&name, &r.project, &by(r.by)))
}

async fn remove(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<ByQ>) -> Result<Json<Event>, ApiError> {
    if name == "surface" {
        return Err(Error::Invalid("the surface cannot be removed; reshape or undo it instead".into()).into());
    }
    app.change(|s| s.remove(&name, &by(q.by)))
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
    let boot = json!({"name": j.name, "about": j.about, "version": j.version, "viewRev": j.view_rev, "state": j.state, "feedback": j.feedback, "attention": j.attention, "working": j.working});
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

async fn restore(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_archived(&name, false, &by(r.by)))
}

#[derive(Deserialize)]
struct AssetQ {
    by: Option<String>,
}

async fn put_asset(State(app): State<Shared>, Path((name, file)): Path<(String, String)>, Query(q): Query<AssetQ>, body: Bytes) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.put_asset(&name, &file, &body, &by(q.by)))
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
async fn snapshot(State(app): State<Shared>, Path(name): Path<String>) -> Result<Html<String>, ApiError> {
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
/// agent should hear about at the start of its next turn.
async fn inbox(State(app): State<Shared>, Query(q): Query<InboxQ>) -> Json<Value> {
    let path = app.store.lock().expect("store lock").events_path();
    let since = q.since.unwrap_or(0);
    let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let prefix = q.project.filter(|p| !p.is_empty()).map(|p| format!("{p}:"));
    let items: Vec<Value> = text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["at_ms"].as_u64().unwrap_or(0) > since && e["by"] == "person" && e["jelly"] != "surface")
        .filter(|e| prefix.as_deref().is_none_or(|p| e["jelly"].as_str().unwrap_or("").starts_with(p)))
        .filter(|e| matches!(e["kind"].as_str(), Some("call" | "comment" | "reply" | "resolve" | "undo" | "answer")))
        .collect();
    Json(json!(items))
}

/// Decision jellies have `questions` (older ones had a single `options` list).
fn is_decision(state: &Value) -> bool {
    state.get("questions").or_else(|| state.get("options")).is_some()
}

#[derive(Deserialize)]
struct AttendReq {
    #[serde(flatten)]
    spec: Value,
}

async fn attend(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<AttendReq>) -> Result<Json<Event>, ApiError> {
    let by = r.spec["by"].as_str().map(String::from);
    app.change(|s| s.attend(&name, r.spec, &self::by(by)))
}

#[derive(Deserialize)]
struct AnswerReq {
    option: Option<String>,
    #[serde(default)]
    text: String,
    by: Option<String>,
}

async fn answer(State(app): State<Shared>, Path((name, id)): Path<(String, String)>, Json(r): Json<AnswerReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.answer(&name, &id, r.option.as_deref(), &r.text, &by(r.by)))
}

async fn withdraw(State(app): State<Shared>, Path((name, id)): Path<(String, String)>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.withdraw(&name, &id, &by(r.by)))
}

#[derive(Deserialize)]
struct WorkingReq {
    what: Option<String>,
    by: Option<String>,
}

async fn working(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<WorkingReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.set_working(&name, r.what.as_deref().filter(|w| !w.trim().is_empty()), &by(r.by)))
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
    app.change(|s| s.call("surface", "focus", json!({"name": r.name}), &by(r.by)))
}

#[derive(Deserialize)]
struct ProjectQ {
    project: Option<String>,
}

/// Everything open for the person, newest first: attention items, plus open
/// asks made with `jelly ask --spec`.
async fn attention_list(State(app): State<Shared>, Query(q): Query<ProjectQ>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    let project = q.project.filter(|p| !p.is_empty());
    let mut items: Vec<Value> = Vec::new();
    for j in store.list().filter(|j| j.name != "surface" && project.as_deref().is_none_or(|p| j.project() == p)) {
        for a in j.attention.iter().filter(|a| a.answer.is_none()) {
            let mut v = serde_json::to_value(a).expect("serializes");
            v["jelly"] = json!(j.name);
            v["about"] = json!(j.about);
            items.push(v);
        }
        if is_decision(&j.state) && j.state.get("status") == Some(&json!("open")) {
            items.push(json!({"jelly": j.name, "about": j.about, "id": "ask", "kind": "ask", "title": j.about, "by": j.made_by, "at_ms": j.updated_ms, "form": true}));
        }
    }
    items.sort_by_key(|i| std::cmp::Reverse(i["at_ms"].as_u64().unwrap_or(0)));
    Json(json!(items))
}
