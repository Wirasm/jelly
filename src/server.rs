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
    /// What some browser last showed of each jelly, as text. Memory only.
    screens: Mutex<std::collections::HashMap<String, Screen>>,
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
        .route("/api/ws", get(ws))
        .route("/api/jellies", get(list).post(create))
        .route("/api/jellies/{name}", get(show).delete(remove))
        .route("/api/jellies/{name}/state", get(state))
        .route("/api/jellies/{name}/call", post(call))
        .route("/api/jellies/{name}/reshape", post(reshape))
        .route("/api/jellies/{name}/undo", post(undo))
        .route("/api/jellies/{name}/pin", post(pin))
        .route("/api/jellies/{name}/move", post(move_project))
        .route("/api/jellies/{name}/restore", post(restore))
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
            "decision": is_decision(&j.state).then(|| j.state.get("status").cloned()).flatten(),
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
    app.change(|s| s.attend(&name, r.spec, &self::by(by)?))
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
}

/// Everything open for the person, newest first: attention items, plus open
/// asks made with `jelly ask --spec`.
async fn attention_list(State(app): State<Shared>, Query(q): Query<ProjectQ>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    let project = q.project.filter(|p| !p.is_empty());
    let mut items: Vec<Value> = Vec::new();
    for j in store.list().filter(|j| j.name != PAGE && project.as_deref().is_none_or(|p| j.project() == p)) {
        for a in j.attention.iter().filter(|a| a.is_open()) {
            let mut v = serde_json::to_value(a).expect("serializes");
            v["jelly"] = json!(j.name);
            v["about"] = json!(j.about);
            items.push(v);
        }
        if is_decision(&j.state) && j.state.get("status") == Some(&json!("open")) {
            let qs = j.state["questions"].as_array().cloned().unwrap_or_default();
            let done = qs.iter().filter(|q| !q["answer"].is_null() || q["note"]["text"].as_str().is_some_and(|t| !t.is_empty())).count();
            if done < qs.len() {
                items.push(json!({"jelly": j.name, "about": j.about, "id": "ask", "kind": "ask", "title": j.about, "by": j.made_by, "at_ms": j.updated_ms, "form": true, "progress": [done, qs.len()]}));
            }
        }
    }
    items.sort_by_key(|i| std::cmp::Reverse(i["at_ms"].as_u64().unwrap_or(0)));
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

/// Runs headless Chrome on a jelly at a given size. With `png`, writes a
/// screenshot there; either way the page's runtime posts a fresh snapshot.
async fn render(port: u16, name: &str, width: u32, height: u32, png: Option<&std::path::Path>) -> Result<(), String> {
    let bin = chrome().ok_or("no Chrome, Chromium, Edge or Brave found; set JELLY_CHROME to a browser binary")?;
    // Headless Chrome will not make a window narrower than 500px, so phone
    // widths render the jelly in an iframe of exactly that size.
    let url = if width < 500 {
        format!("http://127.0.0.1:{port}/see-frame/{name}?w={width}&h={height}")
    } else {
        format!("http://127.0.0.1:{port}/j/{name}/?agent=1")
    };
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
    match tokio::time::timeout(std::time::Duration::from_secs(20), run).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(format!("could not run the browser: {e}")),
        Err(_) => Err("the browser took longer than 20 s".into()),
    }
}

#[derive(Deserialize)]
struct SeeQ {
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    png: bool,
}

/// Everything an agent needs to pick up a jelly, as plain text: what it is,
/// what the person did, what waits on them, and what is on screen.
async fn see(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<SeeQ>, req_headers: axum::http::HeaderMap) -> Result<Response, ApiError> {
    let (j, home) = {
        let store = app.store.lock().expect("store lock");
        (store.get(&name)?.clone(), store.home().to_path_buf())
    };
    let port = req_headers.get(header::HOST).and_then(|h| h.to_str().ok()).and_then(|h| h.rsplit(':').next()).and_then(|p| p.parse().ok()).unwrap_or(7878);
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
        match render(port, &name, width, height, None).await {
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
        match render(port, &name, width, height, Some(&path)).await {
            Ok(()) => Some(path),
            Err(e) => { notes.push(format!("No screenshot: {e}")); None }
        }
    } else { None };

    let mut out = String::new();
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    line(&mut out, format!("{} · {} · v{} · by {}{}{}", j.name, if j.about.is_empty() { "(no about line)" } else { &j.about }, j.version, j.made_by,
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
    if let Ok(text) = std::fs::read_to_string(home.join("events.jsonl")) {
        let mine: Vec<Event> = text.lines().filter(|l| l.contains(&format!("\"jelly\":\"{name}\""))).filter_map(|l| serde_json::from_str(l).ok()).filter(|e: &Event| e.by == PERSON).collect();
        for e in mine.iter().rev().take(8).rev() {
            line(&mut out, format!("person   {} {}", ago(e.at_ms), e.describe()));
        }
    }
    let project = j.project();
    if !project.is_empty() && app.store.lock().expect("store lock").project_dir(project).join("DESIGN.md").exists() {
        line(&mut out, format!("design   project {project} has a DESIGN.md: jelly design · CSS variables at /p/{project}/design.css"));
    }
    if let Some(p) = &png { line(&mut out, format!("png      {}", p.display())); }
    for n in notes { line(&mut out, format!("!        {n}")); }
    if let Some(s) = snap {
        line(&mut out, format!("\non screen at {}×{} (v{}):", s.width, s.height, s.version));
        out.push_str(&s.text);
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
    Html(format!("<!doctype html><body style=\"margin:0;background:#2b3133\"><iframe src=\"/j/{name}/?agent=1\" style=\"display:block;border:0;width:{w}px;height:{h}px;background:#fff\"></iframe></body>", w = q.w, h = q.h))
}
