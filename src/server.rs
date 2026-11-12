//! The local daemon: an HTTP API over the store, a live event stream, and the
//! views. The jelly page people look at is itself an internal jelly named `surface`.

use crate::store::{AttentionKind, Error, Event, EventKind, InboxBatch, InboxCommit, InboxItem, InboxRequest, Store, PAGE, PERSON};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::body::Bytes;
use axum::http::{header, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
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
const ASK_VIEW: &str = include_str!("../assets/ask.html");
const ASK_VERBS: &str = include_str!("../assets/ask.verbs.js");
const ARCHIVE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;

struct App {
    store: Mutex<Store>,
    events: broadcast::Sender<Event>,
    /// What some browser last showed of each jelly, as text. Memory only.
    screens: Mutex<std::collections::HashMap<String, ViewScreen>>,
}
type Shared = Arc<App>;

impl App {
    /// Several store changes as one: every event is broadcast.
    fn change_many(&self, f: impl FnOnce(&mut Store) -> Result<Vec<Event>, Error>) -> Result<Json<Value>, ApiError> {
        let mut store = self.store.lock().expect("store lock");
        let evs = f(&mut store)?;
        self.invalidate_screens(&store, &evs);
        for ev in &evs {
            let _ = self.events.send(ev.clone());
        }
        Ok(Json(json!({"jelly": evs.last().map(|e| e.jelly.clone()), "events": evs.len(), "state": evs.last().and_then(|e| e.state.clone())})))
    }

    /// Runs one store change and broadcasts its event. The store lock is what
    /// makes verbs run one at a time.
    fn change(&self, f: impl FnOnce(&mut Store) -> Result<Event, Error>) -> Result<Json<Event>, ApiError> {
        let mut store = self.store.lock().expect("store lock");
        let ev = f(&mut store)?;
        self.invalidate_screens(&store, std::slice::from_ref(&ev));
        let _ = self.events.send(ev.clone());
        Ok(Json(ev))
    }

    // All combined access takes the store lock before the screens lock, so a
    // late post cannot refill a cache entry invalidated by a lifecycle change.
    fn invalidate_screens(&self, store: &Store, events: &[Event]) {
        let mut screens = self.screens.lock().expect("screens lock");
        for ev in events {
            if ev.kind == EventKind::Move {
                if let Some(from) = ev.args.as_ref().and_then(|a| a["from"].as_str()) {
                    screens.remove(from);
                }
            }
            if matches!(ev.kind, EventKind::New | EventKind::Remove | EventKind::Move)
                || screens.get(&ev.jelly).is_some_and(|s| store.get(&ev.jelly).map_or(true, |j| j.view_rev != s.view_rev)) {
                screens.remove(&ev.jelly);
            }
        }
    }

    fn snapshot_owner(&self, name: &str, view_rev: u64) -> Result<String, ApiError> {
        use base64::Engine as _;
        let mut screens = self.screens.lock().expect("screens lock");
        if let Some(entry) = screens.get(name).filter(|s| s.view_rev == view_rev) {
            return Ok(entry.owner.clone());
        }
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|e| ApiError::Internal(format!("could not issue snapshot owner: {e}")))?;
        let owner = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        screens.insert(name.into(), ViewScreen { owner: owner.clone(), view_rev, screen: None });
        Ok(owner)
    }

    fn screen(&self, name: &str, owner: &str) -> Result<Option<Screen>, ApiError> {
        let store = self.store.lock().expect("store lock");
        let j = store.get(name)?;
        let screens = self.screens.lock().expect("screens lock");
        let entry = screens.get(name).filter(|s| s.owner == owner && s.view_rev == j.view_rev)
            .ok_or_else(snapshot_changed)?;
        Ok(entry.screen.clone())
    }

    fn validate_see(&self, name: &str, owner: &str, version: u64) -> Result<(), ApiError> {
        let store = self.store.lock().expect("store lock");
        let j = store.get(name)?;
        let screens = self.screens.lock().expect("screens lock");
        if j.version != version || !screens.get(name).is_some_and(|s| s.owner == owner && s.view_rev == j.view_rev) {
            return Err(snapshot_changed());
        }
        Ok(())
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
        .route("/api/jellies/{name}/set", post(set_state))
        .route("/api/jellies/{name}/undo", post(undo))
        .route("/api/jellies/{name}/pin", post(pin))
        .route("/api/jellies/{name}/move", post(move_project))
        .route("/api/jellies/{name}/restore", post(restore))
        .route("/api/jellies/{name}/archive", post(archive))
        .route("/api/jellies/{name}/assets", get(list_assets))
        .route("/api/jellies/{name}/assets/{file}", post(put_asset).layer(DefaultBodyLimit::max(25 * 1024 * 1024)))
        .route("/api/jellies/{name}/share", get(share))
        .route("/api/inbox", get(inbox).post(read_inbox))
        .route("/api/inbox/commit", post(commit_inbox))
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
enum ApiError {
    Store(Error),
    Conflict(String),
    Internal(String),
}

fn snapshot_changed() -> ApiError {
    ApiError::Conflict("the jelly changed; reload the page or retry see".into())
}
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError::Store(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Store(e) => {
                let status = match e {
                    Error::NotFound(_) => StatusCode::NOT_FOUND,
                    Error::Exists(_) => StatusCode::CONFLICT,
                    Error::Invalid(_) => StatusCode::BAD_REQUEST,
                };
                (status, e.to_string())
            }
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(json!({"error": message}))).into_response()
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
        let ev = match ev {
            Ok(ev) => ev,
            // Comments wake state readers without becoming events for CLI consumers.
            Err(_) => return Some(Ok(SseEvent::default().comment("resync"))),
        };
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
    let owner = app.snapshot_owner(&name, j.view_rev)?;
    let boot = json!({"snapshotOwner": owner, "name": j.name, "about": j.about, "version": j.version, "viewRev": j.view_rev, "state": j.state, "comments": j.comments, "attention": j.attention, "working": j.working});
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

async fn list_assets(State(app): State<Shared>, Path(name): Path<String>) -> Result<Json<Vec<String>>, ApiError> {
    let store = app.store.lock().expect("store lock");
    store.get(&name)?;
    Ok(Json(store.try_assets(&name).map_err(|e| ApiError::Internal(format!("could not list assets for {name}: {e}")))?))
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
    if file.starts_with('.') || file.contains('/') {
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
        (j, store.asset_dir(&name), store.try_assets(&name)
            .map_err(|e| ApiError::Internal(format!("could not share assets for {name}: {e}")))?)
    };
    let mut assets = Vec::new();
    for file in files {
        let bytes = std::fs::read(dir.join(&file))
            .map_err(|e| Error::Invalid(format!("could not share asset {file}: {e}")))?;
        let uri = format!("data:{};base64,{}", content_type(&file), base64::engine::general_purpose::STANDARD.encode(bytes));
        assets.push((file, uri));
    }
    let boot = json!({"name": j.name, "about": j.about, "state": j.state, "attention": j.attention, "assets": assets})
        .to_string().replace("</", "<\\/");
    let inject = format!("<script>window.__JELLY__={boot};</script><script>{SNAPSHOT_JS}</script>");
    let html = lol_html::rewrite_str(&j.view, lol_html::RewriteStrSettings {
        element_content_handlers: vec![lol_html::element!("template[shadowrootmode]", |element| {
            if element.get_attribute("shadowrootmode").is_some_and(|mode| mode.eq_ignore_ascii_case("open") || mode.eq_ignore_ascii_case("closed")) {
                element.prepend("<jelly-snapshot-root hidden></jelly-snapshot-root>", lol_html::html_content::ContentType::Html);
            }
            Ok(())
        })],
        ..Default::default()
    }).map_err(|e| ApiError::Internal(format!("could not prepare shared view: {e}")))?;
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
    let items: Vec<Value> = text.lines()
        .filter_map(|l| serde_json::from_str::<Event>(l).ok())
        .filter(|e| e.at_ms > since && e.is_inbox_item(q.project.as_deref().unwrap_or("")))
        .map(inbox_item)
        .collect();
    Json(json!(items))
}


fn inbox_item(event: Event) -> Value {
    json!(InboxItem::from(event))
}

async fn read_inbox(State(app): State<Shared>, Json(req): Json<InboxRequest>) -> Result<Json<InboxBatch>, ApiError> {
    Ok(Json(app.store.lock().expect("store lock").read_inbox(&req.project, &req.reader)?))
}

async fn commit_inbox(State(app): State<Shared>, Json(req): Json<InboxCommit>) -> Result<Json<Value>, ApiError> {
    app.store.lock().expect("store lock").commit_inbox(req)?;
    Ok(Json(json!({"ok": true})))
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
            Ok(AnchorMeasurement { place, exposed, inline }) => {
                out["anchor"] = json!(place.status());
                if inline { out["anchor_inline"] = json!(true); }
                if exposed { out["anchor_exposed"] = json!(true); }
                if let AnchorPlace::Fallback(f) = place {
                    out["anchor_destination"] = json!(f.destination);
                    out["anchor_reason"] = json!(f.reason);
                }
            },
            Err(why) => out["anchor_unchecked"] = json!(why),
        }
    }
    Ok(Json(out))
}

/// How long an ask waits for a browser before it answers "not checked".
const ANCHOR_CHECK: std::time::Duration = std::time::Duration::from_secs(8);

/// Placement from a fresh browser snapshot; failure to render does not undo the ask.
async fn anchor_place(app: &App, port: u16, name: &str, item: &str, version: u64, asked: u64) -> Result<AnchorMeasurement, String> {
    let owner = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(name).map_err(|e| e.to_string())?;
        app.snapshot_owner(name, j.view_rev).map_err(|_| "could not issue snapshot owner".to_string())?
    };
    let render = render(port, name, 1280, 900, None, ANCHOR_CHECK);
    tokio::pin!(render);
    // The browser often posts its snapshot and then lingers, so answer as soon
    // as the snapshot is in; dropping the render stops the browser.
    let mut rendered: Option<(Result<(), String>, std::time::Instant)> = None;
    loop {
        let snap = app.screen(name, &owner).map_err(|_| "the jelly changed during the anchor check".to_string())?
            .filter(|s| s.version >= version && s.at_ms >= asked);
        if let Some(measured) = snap.and_then(|s| s.placement(item).map(|place| AnchorMeasurement { place, exposed: s.exposed.iter().any(|id| id == item), inline: s.inline })) {
            return Ok(measured);
        }
        match &rendered {
            // The browser is gone: give its snapshot a moment to land, then give up.
            Some((r, at)) if at.elapsed() > std::time::Duration::from_secs(2) => {
                return Err(r.clone().err().unwrap_or_else(|| "the page sent no placement measurement for this question".into()));
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

#[derive(serde::Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum AnchorDestination {
    Top,
    After { selector: String },
}

impl AnchorDestination {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Top => "at the top".into(),
            Self::After { selector } => format!("after {selector}"),
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
struct AnchorFallback {
    id: String,
    destination: AnchorDestination,
    reason: String,
}

#[derive(Debug)]
struct AnchorMeasurement {
    place: AnchorPlace,
    exposed: bool,
    inline: bool,
}

#[derive(Deserialize, Serialize, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnchorStatus {
    Below,
    Invalid,
    Missing,
    Floating,
    Hidden,
    Fallback,
}

#[derive(Debug, PartialEq)]
enum AnchorPlace {
    Below,
    Invalid,
    Missing,
    Floating,
    Hidden,
    Fallback(AnchorFallback),
}

impl AnchorPlace {
    fn status(&self) -> AnchorStatus {
        match self {
            Self::Below => AnchorStatus::Below,
            Self::Invalid => AnchorStatus::Invalid,
            Self::Missing => AnchorStatus::Missing,
            Self::Floating => AnchorStatus::Floating,
            Self::Hidden => AnchorStatus::Hidden,
            Self::Fallback(_) => AnchorStatus::Fallback,
        }
    }
}

/// What a browser shows of a jelly, as text, posted by the runtime for `jelly see`.
#[derive(Deserialize, Clone)]
struct Screen {
    owner: String,
    text: String,
    width: u32,
    height: u32,
    version: u64,
    #[serde(default)]
    at_ms: u64,
    #[serde(default)]
    inline: bool,
    /// Errors the page threw or logged.
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    invalid: Vec<String>,
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
    /// Cards measured at the top because adjacent placement was unavailable.
    #[serde(default)]
    exposed: Vec<String>,
    #[serde(default)]
    page_height: u32,
    #[serde(default)]
    fallback: Vec<AnchorFallback>,
    #[serde(default)]
    below: Vec<String>,
}

impl Screen {
    fn placement(&self, item: &str) -> Option<AnchorPlace> {
        let lists = |l: &[String]| l.iter().any(|m| m.split_once(' ').is_some_and(|(id, _)| id == item));
        Some(if lists(&self.invalid) { AnchorPlace::Invalid }
        else if lists(&self.missing) { AnchorPlace::Missing }
        else if lists(&self.floating) { AnchorPlace::Floating }
        else if lists(&self.hidden) { AnchorPlace::Hidden }
        else if let Some(f) = self.fallback.iter().find(|f| f.id == item) { AnchorPlace::Fallback(f.clone()) }
        else if lists(&self.below) { AnchorPlace::Below }
        else { return None })
    }
}

struct ViewScreen {
    owner: String,
    view_rev: u64,
    screen: Option<Screen>,
}

async fn put_screen(State(app): State<Shared>, Path(name): Path<String>, Json(mut sc): Json<Screen>) -> Result<StatusCode, ApiError> {
    let store = app.store.lock().expect("store lock");
    let j = store.get(&name)?;
    let mut screens = app.screens.lock().expect("screens lock");
    let entry = screens.get_mut(&name).filter(|s| s.owner == sc.owner && s.view_rev == j.view_rev)
        .ok_or_else(snapshot_changed)?;
    sc.at_ms = now_ms();
    entry.screen = Some(sc);
    Ok(StatusCode::NO_CONTENT)
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
    run_browser(&bin, port, name, width, height, png, limit).await
}

async fn run_browser(bin: &str, port: u16, name: &str, width: u32, height: u32, png: Option<&std::path::Path>, limit: std::time::Duration) -> Result<(), String> {
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
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(format!("the browser failed ({status})")),
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
    let port = port_of(&req_headers);
    let render_name = name.clone();
    see_with_renderer(app, name, q, move |width, height, png: Option<std::path::PathBuf>| {
        let name = render_name.clone();
        async move { render(port, &name, width, height, png.as_deref(), RENDER_LIMIT).await }
    }).await
}

async fn see_with_renderer<F, Fut>(app: Shared, name: String, q: SeeQ, mut render_page: F) -> Result<Response, ApiError>
where
    F: FnMut(u32, u32, Option<std::path::PathBuf>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let (j, home, owner) = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(&name)?;
        (j.clone(), store.home().to_path_buf(), app.snapshot_owner(&name, j.view_rev)?)
    };
    let width = q.width.unwrap_or(1280);
    let height = q.height.unwrap_or(if width <= 500 { 844 } else { 900 });
    let fresh = |app: &App| -> Result<Option<Screen>, ApiError> {
        Ok(app.screen(&name, &owner)?.filter(|s| s.version == j.version && s.width == width))
    };
    let mut notes: Vec<String> = Vec::new();
    let mut snap = fresh(&app)?;
    if snap.is_none() {
        let asked = now_ms();
        match render_page(width, height, None).await {
            Ok(()) => {
                for _ in 0..30 {
                    if let Some(s) = fresh(&app)?.filter(|s| s.at_ms >= asked) {
                        snap = Some(s);
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
        match render_page(width, h, Some(path.clone())).await {
            Ok(()) => Some(path),
            Err(e) => { notes.push(format!("No screenshot: {e}")); None }
        }
    } else { None };

    app.validate_see(&name, &owner, j.version)?;
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
        line(&mut out, format!("waits    {}", a.summary()));
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
        let fallback = if sc.inline { "placement not checked: the view places its own attention" } else { "it shows at the top instead" };
        for m in &sc.invalid {
            let (id, sel) = m.split_once(' ').unwrap_or((m, ""));
            line(&mut out, format!("!        {id} asks about {sel}, which has invalid CSS selector syntax; {fallback}"));
        }
        for m in &sc.missing {
            let (id, sel) = m.split_once(' ').unwrap_or((m, ""));
            line(&mut out, format!("!        {id} asks about {sel}, which matches nothing on the page; {fallback}"));
        }
        for m in &sc.floating {
            let (id, sel) = m.split_once(' ').unwrap_or((m, ""));
            line(&mut out, format!("!        {id} asks about {sel}, which is in a fixed or sticky part of the page; {fallback}"));
        }
        for f in &sc.fallback {
            line(&mut out, format!("!        {} shows {} instead; {}", f.id, f.destination.describe(), f.reason));
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
mod share_tests {
    use super::*;
    use base64::Engine as _;

    fn fixture(view: &str) -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.create("test:share", view.into(), "const verbs = {};".into(),
            json!({"picture": "assets/dot.png", "file": "dot.png", "textFile": "hello.txt", "label": "Label: assets/dot.png", "closing": "</script>"}),
            "agent", false, "Shared </script> jelly", "").unwrap();
        for (anchor, title) in [("#pictures", "open"), ("#pictures", "answered"), ("#other", "withdrawn"), ("#other", "other")] {
            store.attend("test:share", json!({"kind": "look", "title": title, "anchor": {"selector": anchor}}), "agent").unwrap();
        }
        store.answer("test:share", "a2", None, "captured answer", PERSON).unwrap();
        store.withdraw("test:share", "a3", "agent").unwrap();
        let (events, _) = broadcast::channel(16);
        (dir, Arc::new(App { store: Mutex::new(store), events, screens: Mutex::default() }))
    }

    async fn exported(app: Shared) -> String {
        share(State(app), Path("test:share".into())).await.unwrap_or_else(|e| panic!("{e:?}")).0
    }

    fn boot(html: &str) -> Value {
        let json = html.split_once("window.__JELLY__=").unwrap().1.split_once(";</script>").unwrap().0;
        serde_json::from_str(&json.replace("<\\/", "</")).unwrap()
    }

    #[tokio::test]
    async fn share_embeds_all_assets_and_preserves_captured_data() {
        let (_dir, app) = fixture("<head></head><img src='assets/dot.png.extra'><img src='assets/dot.png'>");
        let expected = {
            let mut store = app.store.lock().unwrap();
            for (file, bytes) in [("dot.png", &b"png"[..]), ("dot.png.extra", &b"long"[..]), ("unused.txt", &b"unused"[..])] {
                store.put_asset("test:share", file, bytes, "agent").unwrap();
            }
            serde_json::to_value(store.get("test:share").unwrap()).unwrap()
        };
        let html = exported(app).await;
        let boot = boot(&html);
        assert_eq!(boot["state"], expected["state"]);
        assert_eq!(boot["attention"], expected["attention"]);
        assert_eq!(boot["about"], expected["about"]);
        assert!(!html.contains("Shared </script>"));
        let assets: Vec<(String, String)> = serde_json::from_value(boot["assets"].clone()).unwrap();
        let assets: std::collections::HashMap<_, _> = assets.into_iter().collect();
        assert_eq!(assets.len(), 3);
        assert_eq!(assets["dot.png"], "data:image/png;base64,cG5n");
        assert_eq!(assets["dot.png.extra"], "data:application/octet-stream;base64,bG9uZw==");
        assert_eq!(assets["unused.txt"], "data:text/plain; charset=utf-8;base64,dW51c2Vk");
        assert!(html.contains("<img src='assets/dot.png.extra'><img src='assets/dot.png'>"));
    }

    #[tokio::test]
    async fn share_without_assets() {
        let (_dir, app) = fixture("<p>No assets</p>");
        let html = exported(app).await;
        assert_eq!(boot(&html)["assets"], json!([]));
        assert!(html.contains("<p>No assets</p>"));
    }

    #[tokio::test]
    async fn share_refuses_an_unreadable_listed_asset() {
        let (_dir, app) = fixture("");
        let path = app.store.lock().unwrap().asset_dir("test:share").join("broken.png");
        std::fs::create_dir_all(path).unwrap();
        let error = share(State(app), Path("test:share".into())).await.expect_err("a directory cannot be read as an asset");
        assert!(format!("{error:?}").contains("could not share asset broken.png"));
    }

    #[tokio::test]
    async fn share_refuses_asset_enumeration_failure() {
        let (_dir, app) = fixture("");
        let path = app.store.lock().unwrap().asset_dir("test:share");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"not a directory").unwrap();
        let error = share(State(app), Path("test:share".into())).await.expect_err("asset enumeration must succeed");
        assert!(matches!(error, ApiError::Internal(message) if message.contains("could not share assets for test:share")));
    }

    #[tokio::test]
    async fn shared_file_renders_assets_and_reading_api() {
        let browser = chrome().expect("Chrome/Chromium required; set JELLY_CHROME to the browser binary");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let callback = format!("http://{}/result", listener.local_addr().unwrap());
        let (send, mut receive) = tokio::sync::mpsc::channel(1);
        let results = Router::new().route("/result", post(move |body: String| async move {
            let result: Value = serde_json::from_str(&body).unwrap();
            send.send(result).await.unwrap();
            StatusCode::NO_CONTENT
        }));
        let server = tokio::spawn(async move { axum::serve(listener, results).await.unwrap(); });
        let view = include_str!("../tests/fixtures/share/view.html")
            .replace("__TEST_RESULT__", &callback)
            .replace("__LIVE_RUNTIME__", &serde_json::to_string(RUNTIME_JS).unwrap().replace("</", "<\\/"))
            .replace("__SNAPSHOT_RUNTIME__", &serde_json::to_string(SNAPSHOT_JS).unwrap().replace("</", "<\\/"));
        let (dir, app) = fixture(&view);
        {
            let mut store = app.store.lock().unwrap();
            let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAEklEQVR4nGP4z8DwHxkzkC4AADxAH+HggXe0AAAAAElFTkSuQmCC").unwrap();
            for file in ["dot.png", "unused.png"] {
                store.put_asset("test:share", file, &png, "agent").unwrap();
            }
            store.put_asset("test:share", "hello.txt", b"hello offline", "agent").unwrap();
            store.put_asset("test:share", "green.svg", br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><path fill="green" d="M0 0h4v4H0z"/></svg>"##, "agent").unwrap();
            store.put_asset("test:share", "red.svg", br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><path fill="red" d="M0 0h4v4H0z"/></svg>"##, "agent").unwrap();
            store.put_asset("test:share", "theme.css", b"@import 'nested.css'; #linked { background-image: url(dot.png); }", "agent").unwrap();
            store.put_asset("test:share", "nested.css", b"@import 'theme.css'; #imported { background-image: url(dot.png); }", "agent").unwrap();
            store.put_asset("test:share", "helper.js", b"window.localHelper = true;", "agent").unwrap();
            for (file, prefix, encoding) in [
                ("legacy", "@charset \"windows-1252\";", "legacy"),
                ("utf16", "", "utf16"),
                ("utf8-bom", "@charset \"windows-1252\";", "utf8"),
            ] {
                let css = format!("{prefix} #encoding-{file}.café {{ background-image: url(dot.png); }} #encoding-{file}.café::after {{ content: \"café\"; }}");
                let bytes = match encoding {
                    "legacy" => css.chars().map(|c| c as u8).collect(),
                    "utf16" => [vec![0xff, 0xfe], css.encode_utf16().flat_map(u16::to_le_bytes).collect()].concat(),
                    _ => [vec![0xef, 0xbb, 0xbf], css.into_bytes()].concat(),
                };
                store.put_asset("test:share", &format!("{file}.css"), &bytes, "agent").unwrap();
            }
            store.put_asset("test:share", "inherited.css", b"@charset \"windows-1252\"; @import 'child.css';", "agent").unwrap();
            store.put_asset("test:share", "child.css", b"#encoding-inherited.caf\xe9 { background-image: url(dot.png); } #encoding-inherited.caf\xe9::after { content: 'caf\xe9'; }", "agent").unwrap();
        }
        let html = exported(app).await;
        let path = dir.path().join("shared.html");
        std::fs::write(&path, html).unwrap();
        let mut browser = tokio::process::Command::new(browser)
            .args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--enable-automation", "--disable-background-timer-throttling", "--disable-component-update", "--remote-debugging-port=0"])
            .arg(format!("--user-data-dir={}", dir.path().join("chrome").display()))
            .arg(format!("file://{}", path.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true).spawn().expect("browser could not start");
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), receive.recv()).await;
        server.abort();
        browser.kill().await.expect("browser could not stop");
        let output = browser.wait_with_output().await.expect("browser output unavailable");
        let result = result.unwrap_or_else(|_| panic!("browser did not report a result within 30 seconds: {}", String::from_utf8_lossy(&output.stderr)))
            .expect("browser result channel closed");
        assert_eq!(result["complete"], true, "{result}");
        assert_eq!(result["ok"], true, "{result}");
        println!("{result}");
    }
}

#[cfg(test)]
mod screen_tests {
    use super::Screen;

    #[test]
    fn older_screens_have_no_exposed_cards() {
        let screen: Screen = serde_json::from_value(serde_json::json!({
            "owner": "test", "text": "page", "width": 1280, "height": 900, "version": 1
        })).unwrap();
        assert!(screen.exposed.is_empty());
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    #[test]
    fn snapshot_selects_the_actual_destination_for_this_item() {
        let mut screen: Screen = serde_json::from_value(json!({
            "owner": "test", "text": "", "width": 1280, "height": 900, "version": 3,
            "fallback": [
                {"id": "a1", "destination": {"kind": "after", "selector": "#tiles"}, "reason": "layout"},
                {"id": "a2", "destination": {"kind": "top"}, "reason": "no boundary"}
            ]
        })).unwrap();
        assert_eq!(screen.placement("other"), None);
        let AnchorPlace::Fallback(first) = screen.placement("a1").unwrap() else { panic!("expected fallback") };
        assert_eq!(first.destination.describe(), "after #tiles");
        assert_eq!(first.reason, "layout");
        let AnchorPlace::Fallback(second) = screen.placement("a2").unwrap() else { panic!("expected fallback") };
        assert_eq!(second.destination, AnchorDestination::Top);
        screen.hidden.push("a1 #tile".into());
        assert_eq!(screen.placement("a1"), Some(AnchorPlace::Hidden));
        screen.floating.push("a1 #tile".into());
        assert_eq!(screen.placement("a1"), Some(AnchorPlace::Floating));
        screen.missing.push("a1 #tile".into());
        assert_eq!(screen.placement("a1"), Some(AnchorPlace::Missing));
        screen.invalid.push("a3 [".into());
        assert_eq!(screen.placement("a3"), Some(AnchorPlace::Invalid));
        assert_eq!(screen.placement("a3").unwrap().status(), AnchorStatus::Invalid);
    }

    #[tokio::test]
    async fn attention_response_preserves_snapshot_destination_and_reason() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.create("placement", "<p>Test</p>".into(), "const verbs = {};".into(), json!({}), "test", false, "", "").unwrap();
        let (events, _) = broadcast::channel(16);
        let app = Arc::new(App { store: Mutex::new(store), events, screens: Mutex::default() });
        for kind in ["ask", "look"] {
            for destination in [json!({"kind":"after", "selector":"#tiles"}), json!({"kind":"top"})] {
                let mut received = app.events.subscribe();
                let post_snapshot = async {
                    let ev = received.recv().await.unwrap();
                    let owner = {
                        let store = app.store.lock().unwrap();
                        app.snapshot_owner("placement", store.get("placement").unwrap().view_rev).unwrap()
                    };
                    let snapshot: Screen = serde_json::from_value(json!({
                        "owner":owner, "text":"", "width":1280, "height":900, "version":ev.version,
                        "fallback":[{"id":ev.args.as_ref().unwrap()["id"], "destination":destination, "reason":"layout cannot take a question row"}]
                    })).unwrap();
                    let mut unmeasured = snapshot.clone();
                    unmeasured.fallback.clear();
                    put_screen(State(app.clone()), Path("placement".into()), Json(unmeasured)).await.unwrap();
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    assert_eq!(put_screen(State(app.clone()), Path("placement".into()), Json(snapshot)).await.unwrap(), StatusCode::NO_CONTENT);
                };
                let request = attend(State(app.clone()), Path("placement".into()), axum::http::HeaderMap::new(), Json(AttendReq {
                    spec: json!({"kind":kind, "title":"Choose", "by":"test", "anchor":{"selector":"#tile"}})
                }));
                let (response, ()) = tokio::join!(request, post_snapshot);
                let Json(response) = response.unwrap_or_else(|_| panic!("attention request failed"));
                assert_eq!(response["anchor"], "fallback");
                assert_eq!(response["anchor_destination"], destination);
                assert_eq!(response["anchor_reason"], "layout cannot take a question row");
                let location = if destination["kind"] == "after" { "after #tiles" } else { "at the top" };
                assert_eq!(crate::placement(&response), format!("it shows {location} instead; layout cannot take a question row"));
            }
        }
        let log = std::fs::read_to_string(app.store.lock().unwrap().events_path()).unwrap();
        assert!(!log.contains("anchor_destination") && !log.contains("anchor_reason"));
    }

    #[test]
    fn unmeasured_snapshots_do_not_confirm_placement() {
        let mut screen: Screen = serde_json::from_value(json!({"owner":"test", "text":"", "width":1, "height":1, "version":1})).unwrap();
        assert!(screen.invalid.is_empty());
        assert!(screen.fallback.is_empty());
        assert_eq!(screen.placement("a1"), None);
        screen.below.push("a1 #tile".into());
        assert_eq!(screen.placement("a1"), Some(AnchorPlace::Below));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let (events, _) = broadcast::channel(32);
        let app = Arc::new(App { store: Mutex::new(Store::open(dir.path()).unwrap()), events, screens: Mutex::default() });
        (dir, app)
    }

    #[tokio::test]
    async fn asset_inventory_is_sorted_read_only_and_requires_a_jelly() {
        let (dir, app) = app();
        let missing = list_assets(State(app.clone()), Path("test:missing".into())).await.unwrap_err();
        assert_eq!(missing.into_response().status(), StatusCode::NOT_FOUND);
        create_page(&app, "PAGE", false).await;
        assert!(list_assets(State(app.clone()), Path("test:x".into())).await.unwrap().0.is_empty());
        for file in ["z.txt", "a.bin"] {
            let _ = put_asset(State(app.clone()), Path(("test:x".into(), file.into())),
                Query(AssetQ { by: Some("test".into()) }), Bytes::from_static(b"asset")).await.unwrap();
        }
        let before = serde_json::to_value(app.store.lock().unwrap().get("test:x").unwrap()).unwrap();
        let log = std::fs::read(dir.path().join("events.jsonl")).unwrap();
        assert_eq!(list_assets(State(app.clone()), Path("test:x".into())).await.unwrap().0, ["a.bin", "z.txt"]);
        assert_eq!(serde_json::to_value(app.store.lock().unwrap().get("test:x").unwrap()).unwrap(), before);
        assert_eq!(std::fs::read(dir.path().join("events.jsonl")).unwrap(), log);
    }

    #[tokio::test]
    async fn asset_route_rejects_hidden_files_and_traversal() {
        let (_dir, app) = app();
        create_page(&app, "PAGE", false).await;
        let dir = app.store.lock().unwrap().asset_dir("test:x");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for path in [dir.join(".hidden"), dir.join("sub/secret.txt"), dir.parent().unwrap().join("secret.txt")] {
            std::fs::write(path, b"secret").unwrap();
        }
        for file in [".hidden", ".", "..", "../secret.txt", "sub/secret.txt"] {
            let response = asset(State(app.clone()), Path(("test:x".into(), file.into()))).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{file}");
        }
    }

    #[tokio::test]
    async fn lagging_sse_subscribers_are_told_to_resync_even_when_filtered() {
        for jelly in [None, Some("other:q".into())] {
            let (_dir, app) = app();
            let response = events(State(app.clone()), Query(ByQ { by: None, jelly })).await;
            let mut body = response.into_response().into_body().into_data_stream();
            create_page(&app, "PAGE", false).await;
            for _ in 0..32 {
                let _ = app.change(|s| s.set_state("test:x", "", Some(json!({})), "test")).unwrap();
            }
            let frame = tokio::time::timeout(std::time::Duration::from_secs(1), body.next())
                .await.expect("lag must wake the subscriber").unwrap().unwrap();
            assert_eq!(&frame[..], b": resync\n\n");
        }
    }

    async fn create_page(app: &Shared, text: &str, replace: bool) {
        let _ = create(State(app.clone()), Json(CreateReq {
            name: "test:x".into(), view: format!("<h1>{text}</h1>"), verbs: "const verbs = {};".into(),
            state: json!({}), by: Some("test".into()), replace, about: String::new(), project: String::new(),
        })).await.unwrap();
    }

    async fn owner(app: &Shared, name: &str) -> String {
        let Html(html) = view(State(app.clone()), Path(name.into())).await.unwrap();
        let boot = html.split_once("window.__JELLY__=").unwrap().1.split_once(";</script>").unwrap().0;
        serde_json::from_str::<Value>(boot).unwrap()["snapshotOwner"].as_str().unwrap().into()
    }

    fn snapshot(owner: &str, text: &str, version: u64) -> Screen {
        serde_json::from_value(json!({"owner": owner, "text": text, "width": 1280, "height": 900, "version": version})).unwrap()
    }

    async fn post(app: &Shared, sc: Screen) -> Result<StatusCode, ApiError> {
        put_screen(State(app.clone()), Path("test:x".into()), Json(sc)).await
    }

    async fn seen(app: &Shared) -> String {
        let response = see(State(app.clone()), Path("test:x".into()),
            Query(SeeQ { width: None, height: None, png: false, full: false }), axum::http::HeaderMap::new()).await.unwrap();
        String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn bundled_runtime_posts_served_ownership_and_reloads_obsolete_pages() {
        let (_dir, app) = app();
        create_page(&app, "PAGE", false).await;
        let Html(html) = view(State(app.clone()), Path("test:x".into())).await.unwrap();
        let boot = html.split_once("window.__JELLY__=").unwrap().1.split_once(";</script>").unwrap().0;
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        let eval = |script: &str| ctx.with(|ctx| {
            ctx.eval::<(), _>(script).unwrap_or_else(|err| panic!("{err}: {:?}", ctx.catch()));
        });
        eval(include_str!("../tests/runtime-browser.js"));
        eval(&format!("window.__JELLY__={boot};"));
        eval(RUNTIME_JS);
        eval("loaded();");
        while rt.execute_pending_job().unwrap() {}
        let request: Value = ctx.with(|ctx| {
            serde_json::from_str(&ctx.eval::<String, _>("JSON.stringify(requests[0])").unwrap()).unwrap()
        });
        assert_eq!(request["url"], "/api/jellies/test%3Ax/screen");
        assert_eq!(post(&app, serde_json::from_value(request["body"].clone()).unwrap()).await.unwrap(), StatusCode::NO_CONTENT);
        eval("if (reloads !== 0) throw Error('accepted post reloaded');");
        eval("message({jelly:'other:x', kind:'new', view_rev:1}); if (reloads !== 0) throw Error('other jelly reloaded');");
        eval("message({jelly:'test:x', kind:'new', view_rev:1, version:1}); if (reloads !== 1) throw Error('same-revision replacement did not reload');");
        create_page(&app, "REPLACEMENT", true).await;
        let stale_status = post(&app, serde_json::from_value(request["body"].clone()).unwrap()).await.unwrap_err().into_response().status();
        assert_eq!(stale_status, StatusCode::CONFLICT);
        eval(&format!("responseStatus={}; loaded();", stale_status.as_u16()));
        while rt.execute_pending_job().unwrap() {}
        eval("if (reloads !== 2) throw Error('stale post did not reload');");
    }

    #[tokio::test]
    async fn see_rechecks_currentness_after_text_and_png_rendering() {
        for png in [false, true] {
            for replace in [false, true] {
                let (_dir, app) = app();
                create_page(&app, "OLD PAGE", false).await;
                let current = owner(&app, "test:x").await;
                if png { post(&app, snapshot(&current, "OLD PAGE", 1)).await.unwrap(); }
                let (started, suspended) = tokio::sync::oneshot::channel();
                let (resume, resumed) = tokio::sync::oneshot::channel();
                let mut rendezvous = Some((started, resumed));
                let seeing = see_with_renderer(app.clone(), "test:x".into(),
                    SeeQ { width: None, height: None, png, full: false },
                    move |_, _, path| {
                        assert_eq!(path.is_some(), png);
                        let (started, resumed) = rendezvous.take().expect("one render");
                        async move {
                            started.send(()).unwrap();
                            resumed.await.unwrap();
                            // A failed text render bypasses polling, so only the final
                            // currentness check can reject the captured jelly here.
                            if png { Ok(()) } else { Err("controlled render failure".into()) }
                        }
                    });
                let changing = async {
                    suspended.await.unwrap();
                    if replace { create_page(&app, "NEW PAGE", true).await; }
                    else { let _ = app.change(|s| s.set_state("test:x", "", Some(json!({"n":1})), "test")).unwrap(); }
                    resume.send(()).unwrap();
                };
                let (result, _) = tokio::join!(seeing, changing);
                assert_eq!(result.expect_err("obsolete see must fail").into_response().status(), StatusCode::CONFLICT,
                    "png={png}, replace={replace}");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn see_png_reports_only_a_screenshot_the_browser_made() {
        use std::os::unix::fs::PermissionsExt;
        for (exit, old_png) in [(3, false), (3, true), (0, false)] {
            let (dir, app) = app();
            create_page(&app, "PAGE", false).await;
            let current = owner(&app, "test:x").await;
            post(&app, snapshot(&current, "PAGE", 1)).await.unwrap();
            let png = dir.path().join("see/test_x-1280.png");
            if old_png {
                std::fs::create_dir_all(png.parent().unwrap()).unwrap();
                std::fs::write(&png, b"old").unwrap();
            }
            let browser = dir.path().join("browser.sh");
            std::fs::write(&browser, format!("#!/bin/sh\nexit {exit}\n")).unwrap();
            std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
            let browser = browser.to_str().unwrap().to_string();
            let response = see_with_renderer(app.clone(), "test:x".into(),
                SeeQ { width: None, height: None, png: true, full: false },
                move |width, height, path| {
                    let browser = browser.clone();
                    async move {
                        run_browser(&browser, 0, "test:x", width, height, path.as_deref(), std::time::Duration::from_secs(5)).await
                    }
                }).await.unwrap();
            let out = String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
            let png_line = format!("png      {}", png.display());
            if exit == 0 {
                assert!(out.contains(&png_line), "{out}");
            } else {
                assert!(!out.contains("png      "), "old_png={old_png}: {out}");
                assert!(out.contains("No screenshot: the browser failed (exit status: 3)"), "{out}");
            }
        }
    }

    #[tokio::test]
    async fn cold_see_waits_for_matching_version_and_width() {
        for wrong_width in [false, true] {
            let (_dir, app) = app();
            create_page(&app, "PAGE", false).await;
            let current = owner(&app, "test:x").await;
            let (rendered, polling) = tokio::sync::oneshot::channel();
            let mut rendered = Some(rendered);
            let posting_app = app.clone();
            let posting_owner = current.clone();
            let seeing = see_with_renderer(app.clone(), "test:x".into(),
                SeeQ { width: None, height: None, png: false, full: false },
                move |_, _, path| {
                    assert!(path.is_none());
                    let app = posting_app.clone();
                    let owner = posting_owner.clone();
                    let rendered = rendered.take().expect("one render");
                    async move {
                        let mut wrong = snapshot(&owner, "WRONG SNAPSHOT", if wrong_width { 1 } else { 2 });
                        if wrong_width { wrong.width = 390; }
                        post(&app, wrong).await.unwrap();
                        rendered.send(()).unwrap();
                        Ok(())
                    }
                });
            let updating = async {
                polling.await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                post(&app, snapshot(&current, "MATCHING SNAPSHOT", 1)).await.unwrap();
            };
            let (result, _) = tokio::join!(seeing, updating);
            let response = result.unwrap();
            let output = String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
            assert!(output.contains("MATCHING SNAPSHOT"), "{output}");
            assert!(!output.contains("WRONG SNAPSHOT"), "{output}");
        }
    }

    #[tokio::test]
    async fn replaced_and_recreated_pages_never_reuse_old_screens() {
        let (_dir, app) = app();
        create_page(&app, "OLD PAGE", false).await;
        let old = owner(&app, "test:x").await;
        assert_eq!(post(&app, snapshot(&old, "OLD PAGE", 1)).await.unwrap(), StatusCode::NO_CONTENT);
        assert!(seen(&app).await.contains("OLD PAGE"));
        for (text, remove_first) in [("NEW PAGE", false), ("RECREATED PAGE", true)] {
            let previous = owner(&app, "test:x").await;
            if remove_first {
                let _ = remove(State(app.clone()), Path("test:x".into()), Query(ByQ { by: Some("test".into()), jelly: None })).await.unwrap();
                assert!(!app.screens.lock().unwrap().contains_key("test:x"));
                assert_eq!(post(&app, snapshot(&previous, "OLD PAGE", 1)).await.unwrap_err().into_response().status(), StatusCode::NOT_FOUND);
            }
            create_page(&app, text, !remove_first).await;
            assert!(!app.screens.lock().unwrap().contains_key("test:x"));
            assert_eq!(post(&app, snapshot(&previous, "OLD PAGE", 1)).await.unwrap_err().into_response().status(), StatusCode::CONFLICT);
            let current = owner(&app, "test:x").await;
            assert_eq!(owner(&app, "test:x").await, current);
            for invalid in [old.as_str(), previous.as_str(), "", "unknown"] {
                assert_eq!(post(&app, snapshot(invalid, "OLD PAGE", 1)).await.unwrap_err().into_response().status(), StatusCode::CONFLICT);
            }
            assert!(app.screen("test:x", &current).unwrap().is_none());
            post(&app, snapshot(&current, text, 1)).await.unwrap();
            assert!(post(&app, snapshot(&previous, "OLD PAGE", 1)).await.is_err());
            let output = seen(&app).await;
            assert!(output.contains(text), "{output}");
            assert!(!output.contains("OLD PAGE"), "{output}");
            assert!(app.validate_see("test:x", &previous, 1).is_err());
        }
        assert!(serde_json::from_value::<Screen>(json!({"text": "tokenless", "width":1280, "height":900, "version":1})).is_err());
        let dir = app.store.lock().unwrap().home().to_path_buf();
        let (events, _) = broadcast::channel(32);
        let restarted = Arc::new(App { store: Mutex::new(Store::open(&dir).unwrap()), events, screens: Mutex::default() });
        let previous = owner(&app, "test:x").await;
        let current = owner(&restarted, "test:x").await;
        assert!(post(&restarted, snapshot(&previous, "OLD PAGE", 1)).await.is_err());
        post(&restarted, snapshot(&current, "RECREATED PAGE", 1)).await.unwrap();
    }

    #[tokio::test]
    async fn view_changes_invalidate_ownership_but_state_and_failed_validation_preserve_it() {
        let (_dir, app) = app();
        create_page(&app, "OLD PAGE", false).await;
        let first = owner(&app, "test:x").await;
        post(&app, snapshot(&first, "OLD PAGE", 1)).await.unwrap();
        assert!(app.change(|s| s.create("test:x", "bad".into(), "invalid JS!".into(), json!({}), "test", true, "", "")).is_err());
        assert!(app.change(|s| s.reshape("test:x", Some("bad".into()), Some("invalid JS!".into()), None, "test")).is_err());
        assert!(seen(&app).await.contains("OLD PAGE"));
        let _ = app.change(|s| s.set_state("test:x", "", Some(json!({"n":1})), "test")).unwrap();
        assert_eq!(owner(&app, "test:x").await, first);
        assert_eq!(app.screen("test:x", &first).unwrap().unwrap().version, 1);
        assert!(app.validate_see("test:x", &first, 1).is_err());
        let _ = app.change(|s| s.reshape("test:x", Some("<h1>NEW PAGE</h1>".into()), None, None, "test")).unwrap();
        assert!(post(&app, snapshot(&first, "OLD PAGE", 1)).await.is_err());
        let reshaped = owner(&app, "test:x").await;
        let _ = app.change(|s| s.undo("test:x", "test")).unwrap();
        assert!(post(&app, snapshot(&reshaped, "NEW PAGE", 3)).await.is_err());
        let undone = owner(&app, "test:x").await;
        let _ = app.change(|s| s.set_project("test:x", "moved", "test")).unwrap();
        assert!(!app.screens.lock().unwrap().contains_key("test:x"));
        assert!(post(&app, snapshot(&undone, "OLD PAGE", 4)).await.is_err());
        create_page(&app, "REUSED NAME", false).await;
        assert!(post(&app, snapshot(&undone, "OLD PAGE", 1)).await.is_err());
        let moved = owner(&app, "moved:x").await;
        assert_ne!(moved, undone);
    }

    #[tokio::test]
    async fn ask_rounds_invalidate_reshaped_views_and_anchor_checks_require_fresh_owned_screens() {
        let (_dir, app) = app();
        let spec = json!({"question":"Which?", "options":[{"id":"a", "label":"A"}, {"id":"b", "label":"B"}]});
        let _ = app.change_many(|s| s.ask_round("test:x", &spec, ASK_VIEW, ASK_VERBS, "", "test")).unwrap();
        let original = owner(&app, "test:x").await;
        let _ = app.change(|s| s.reshape("test:x", Some("different".into()), None, None, "test")).unwrap();
        let different = owner(&app, "test:x").await;
        let _ = app.change_many(|s| s.ask_round("test:x", &spec, ASK_VIEW, ASK_VERBS, "", "test")).unwrap();
        assert!(post(&app, snapshot(&different, "different", 1)).await.is_err());
        assert!(app.screen("test:x", &original).is_err());
        let current = owner(&app, "test:x").await;
        let version = app.store.lock().unwrap().get("test:x").unwrap().version;
        let asked = now_ms();
        let mut sc = snapshot(&current, "ASK PAGE", version);
        sc.missing.push("a2 #q-q1".into());
        post(&app, sc).await.unwrap();
        assert_eq!(anchor_place(&app, 1, "test:x", "a2", version, asked).await.unwrap().place, AnchorPlace::Missing);
        // The wait must ignore a screen from before the ask and an older state.
        app.screens.lock().unwrap().get_mut("test:x").unwrap().screen.as_mut().unwrap().at_ms = asked - 1;
        let checking = anchor_place(&app, 1, "test:x", "a2", version, asked);
        let update = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let mut sc = snapshot(&current, "AFTER ASK", version);
            sc.below.push("a2 #q-q1".into());
            post(&app, sc).await.unwrap();
        };
        let (result, _) = tokio::join!(checking, update);
        assert_eq!(result.unwrap().place, AnchorPlace::Below);
        let checking = anchor_place(&app, 1, "test:x", "a2", version + 1, asked);
        let update = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let mut sc = snapshot(&current, "NEW STATE", version + 1);
            sc.hidden.push("a2 #q-q1".into());
            post(&app, sc).await.unwrap();
        };
        let (result, _) = tokio::join!(checking, update);
        assert_eq!(result.unwrap().place, AnchorPlace::Hidden);
        let checking = anchor_place(&app, 1, "test:x", "a2", version + 2, now_ms());
        let replace = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            create_page(&app, "REPLACED", true).await;
        };
        let (result, _) = tokio::join!(checking, replace);
        assert!(result.unwrap_err().contains("changed"));
    }
}
