//! The local daemon: an HTTP API over the store, a live event stream, and the
//! views. The jelly page people look at is itself an internal jelly named `page`.

use crate::store::{named, viewer, Error, Event, EventKind, InboxBatch, InboxCommit, InboxItem, InboxRequest, Past, Store, PAGE};
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
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::time::Instant;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

mod ask;
mod export;
mod feed;
mod hear;
mod look;
mod share;
mod update;
mod wake;

pub(crate) use ask::AskAnchor;
use ask::*;
use export::*;
use hear::*;
pub(crate) use look::{ago, now_ms};
use look::*;

/// jelly's own look, for every part that is jelly's and not the jellyware.
const JELLY_CSS: &str = include_str!("../assets/jelly.css");
/// A script carrying `assets/jelly.css` in place of its `"__JELLY_CSS__"`.
fn with_css(script: &str) -> String {
    script.replace("\"__JELLY_CSS__\"", &serde_json::to_string(JELLY_CSS).expect("a string serializes").replace("</", "<\\/"))
}
/// The runtime as browsers get it, and the one an exported file gets in its
/// place: both carry the stylesheet, so a view, a shared page and an exported
/// file all have it.
static RUNTIME_JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| with_css(include_str!("../assets/runtime.js")));
static SNAPSHOT_JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| with_css(include_str!("../assets/snapshot.js")));
const PAGE_VIEW: &str = include_str!("../assets/page.html");
const PAGE_VERBS: &str = include_str!("../assets/page.verbs.js");
const ASK_VIEW: &str = include_str!("../assets/ask.html");
const ASK_VERBS: &str = include_str!("../assets/ask.verbs.js");
const ARCHIVE_AFTER_MS: u64 = 7 * 24 * 3600 * 1000;
/// An archived jelly nobody restored is removed this long after it was archived.
const REMOVE_AFTER_MS: u64 = 30 * 24 * 3600 * 1000;

struct App {
    store: Mutex<Store>,
    events: broadcast::Sender<Event>,
    /// What some browser last showed of each jelly, as text. Memory only.
    screens: Mutex<std::collections::HashMap<String, ViewScreen>>,
    /// `jelly try` renders in progress, by token. Memory only.
    trials: Mutex<std::collections::HashMap<String, Trial>>,
    /// `jelly look` renders in progress, by token. Memory only.
    probes: Mutex<std::collections::HashMap<String, Probe>>,
    /// Jellies shared through the relay, by name: their changes go there.
    shares: Mutex<std::collections::HashMap<String, Arc<share::Live>>>,
    /// One share or unshare at a time.
    sharing: tokio::sync::Mutex<()>,
    /// The jelly each open browser page shows and whether its tab is in front,
    /// by socket. Memory only.
    open: Mutex<std::collections::HashMap<u64, Open>>,
    /// How each jelly's feed last ran, by jelly. Memory only.
    feeds: Mutex<std::collections::HashMap<String, feed::FeedRun>>,
    /// The latest release and the update the person started. Memory only.
    update: Mutex<update::Status>,
}
type Shared = Arc<App>;

/// One open browser page: the jelly it shows, whether its tab is in front, and
/// whether that browser may show notifications.
struct Open {
    name: String,
    visible: bool,
    notifies: bool,
}

impl App {
    /// How many browser pages have this jelly in front of someone right now.
    /// The jelly page counts for every jelly: it shows them all.
    fn visible(&self, name: &str) -> usize {
        self.open.lock().expect("open lock").values().filter(|o| o.visible && (o.name == name || o.name == PAGE)).count()
    }

    /// Whether an open jelly page shows a notification for a new ask. Only the
    /// jelly page notifies, and only when its tab is not in front.
    fn notifies(&self) -> bool {
        self.open.lock().expect("open lock").values().any(|o| o.name == PAGE && !o.visible && o.notifies)
    }

    /// The jelly's share, when it is shared: then its relay holds the live copy.
    fn live(&self, name: &str) -> Option<Arc<share::Live>> {
        self.shares.lock().expect("shares lock").get(name).cloned()
    }

    /// Refuses what a shared jelly cannot do while its live copy is on the relay.
    fn not_shared(&self, name: &str, what: &str) -> Result<(), ApiError> {
        match self.live(name) {
            Some(_) => Err(Error::Invalid(format!("{name} is shared, so it cannot {what}; `jelly unshare {name}` first")).into()),
            None => Ok(()),
        }
    }

    /// Broadcasts committed batch events even when a later change fails.
    fn change_many(&self, f: impl FnOnce(&mut Store) -> Result<Vec<Event>, crate::store::BatchError>) -> Result<Json<Value>, ApiError> {
        let mut store = self.store.lock().expect("store lock");
        let result = f(&mut store);
        let (evs, error) = match result {
            Ok(events) => (events, None),
            Err(failed) => (failed.events, Some(failed.error)),
        };
        self.invalidate_screens(&store, &evs);
        for ev in &evs {
            let _ = self.events.send(ev.clone());
        }
        if let Some(error) = error {
            if let Some(last) = evs.last() {
                return Err(Error::Invalid(format!("{error}; committed {} changes to {} through seq:{}. Inspect with `jelly state {}` before retrying", evs.len(), last.jelly, last.seq, last.jelly)).into());
            }
            return Err(error.into());
        }
        Ok(Json(json!({"jelly": evs.last().map(|e| e.jelly.clone()), "events": evs.len(), "seq": evs.last().map(|e| e.seq), "state": evs.last().and_then(|e| e.state.clone())})))
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
            if matches!(ev.kind, EventKind::Make | EventKind::Remove | EventKind::Move)
                || screens.get(&ev.jelly).is_some_and(|s| store.get(&ev.jelly).map_or(true, |j| j.view_rev != s.view_rev)) {
                screens.remove(&ev.jelly);
            }
        }
    }

    fn snapshot_owner(&self, name: &str, view_rev: u64) -> Result<String, ApiError> {
        let mut screens = self.screens.lock().expect("screens lock");
        if let Some(entry) = screens.get(name).filter(|s| s.view_rev == view_rev) {
            return Ok(entry.owner.clone());
        }
        let owner = random_token()?;
        screens.insert(name.into(), ViewScreen { owner: owner.clone(), view_rev, screen: None, dark: None });
        Ok(owner)
    }

    fn screen(&self, name: &str, owner: &str) -> Result<Option<Screen>, ApiError> {
        self.screen_in(name, owner, false)
    }

    /// The latest snapshot from a light render (or a person's browser), or from
    /// `jelly look`'s dark render.
    fn screen_in(&self, name: &str, owner: &str, dark: bool) -> Result<Option<Screen>, ApiError> {
        let store = self.store.lock().expect("store lock");
        let j = store.get(name)?;
        let screens = self.screens.lock().expect("screens lock");
        let entry = screens.get(name).filter(|s| s.owner == owner && s.view_rev == j.view_rev)
            .ok_or_else(snapshot_changed)?;
        Ok(if dark { entry.dark.clone() } else { entry.screen.clone() })
    }

    fn validate_render(&self, name: &str, owner: &str, version: u64) -> Result<(), ApiError> {
        let store = self.store.lock().expect("store lock");
        let j = store.get(name)?;
        let screens = self.screens.lock().expect("screens lock");
        if j.version != version || !screens.get(name).is_some_and(|s| s.owner == owner && s.view_rev == j.view_rev) {
            return Err(snapshot_changed());
        }
        Ok(())
    }
}

/// `note` is printed after the `jelly on URL` line, which stays first for scripts.
/// The browser `look` and `try` would drive, for `jelly doctor`.
pub fn browser() -> Option<String> {
    look::chrome()
}

pub async fn serve(store: Store, listener: tokio::net::TcpListener, note: Option<String>) -> std::io::Result<()> {
    let mut store = store;
    if store.get(PAGE).is_err() {
        store
            .make(PAGE, PAGE_VIEW.into(), PAGE_VERBS.into(), json!({}), "jelly", false, "The jelly page", "")
            .and_then(|_| store.keep(PAGE, true, "jelly"))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    // The jelly page is jelly's own: a home made by an older binary gets this one's.
    let page = store.get(PAGE).map_err(|e| std::io::Error::other(e.to_string()))?;
    if page.view != PAGE_VIEW || page.verbs != PAGE_VERBS {
        store.shape(PAGE, Some(PAGE_VIEW.into()), Some(PAGE_VERBS.into()), None, "jelly").map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    let (tx, _) = broadcast::channel(1024);
    let app: Shared = Arc::new(App { store: Mutex::new(store), events: tx, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default(), update: Mutex::default() });
    share::resume(&app);
    feed::start(app.clone());
    wake::start(&app);
    update::start(&app);

    // Jellies nobody keeps or touched for a week are archived: hidden from the
    // jelly page, restorable, and back on their own as soon as anything changes them.
    // Thirty quiet days after that they are removed, unless they are shared.
    let sweeper = app.clone();
    tokio::spawn(async move {
        loop {
            let swept = {
                let mut store = sweeper.store.lock().expect("store lock");
                let mut swept = store.sweep(ARCHIVE_AFTER_MS);
                for name in store.forgotten(REMOVE_AFTER_MS) {
                    if sweeper.live(&name).is_none() {
                        swept.extend(store.remove(&name, "jelly").ok());
                    }
                }
                swept
            };
            for ev in swept {
                let _ = sweeper.events.send(ev);
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    let port = listener.local_addr()?.port();
    let _ = writeln!(std::io::stderr().lock(), "jelly on http://127.0.0.1:{port}");
    if let Some(note) = note {
        let _ = writeln!(std::io::stderr().lock(), "{note}");
    }
    axum::serve(listener, router(app)).await
}

fn router(app: Shared) -> Router {
    Router::new()
        // The jelly page: every project at /, one project at /p/<project>.
        .route("/", get(|st: State<Shared>| async move { view(st, Path(PAGE.into()), Query(ViewQ::default())).await }))
        .route("/runtime.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript")], RUNTIME_JS.as_str()) }))
        // Views live at /j/<name>/ so a view can use relative `assets/<file>` paths.
        .route("/j/{name}", get(|Path(name): Path<String>| async move { Redirect::permanent(&format!("/j/{name}/")) }))
        .route("/j/{name}/", get(view))
        .route("/j/{name}/assets/{file}", get(asset))
        // A project's page is the same page, showing only that project's jellies.
        .route("/p/{project}", get(|st: State<Shared>| async move { view(st, Path(PAGE.into()), Query(ViewQ::default())).await }))
        .route("/api/events", get(events))
        .route("/api/wait", get(wait))
        .route("/api/ws", get(ws))
        .route("/api/jellies", get(list).post(create))
        .route("/api/jellies/{name}", get(show).delete(remove))
        .route("/api/jellies/{name}/state", get(state))
        .route("/api/jellies/{name}/do", post(call))
        .route("/api/jellies/{name}/shape", post(shape))
        .route("/api/jellies/{name}/set", post(set_state))
        .route("/api/jellies/{name}/undo", post(undo))
        .route("/api/jellies/{name}/keep", post(keep))
        .route("/api/jellies/{name}/move", post(move_jelly))
        .route("/api/jellies/{name}/restore", post(restore))
        .route("/api/jellies/{name}/archive", post(archive))
        .route("/api/jellies/{name}/assets", get(list_assets))
        .route("/api/jellies/{name}/assets/{file}", post(put_asset).delete(remove_asset).layer(DefaultBodyLimit::max(25 * 1024 * 1024)))
        .route("/api/jellies/{name}/share", get(share).post(share_live).delete(unshare_live))
        .route("/api/jellies/{name}/share/pin", post(pin_live).delete(unpin_live))
        .route("/api/shares", get(shares))
        .route("/api/inbox", get(inbox).post(read_inbox))
        .route("/api/inbox/commit", post(commit_inbox))
        .route("/api/jellies/{name}/screen", post(put_screen))
        .route("/api/jellies/{name}/note", post(note))
        .route("/api/jellies/{name}/look", get(look))
        .route("/look-frame/{name}", get(look_frame))
        .route("/probe-frame", get(probe_frame))
        .route("/api/probes/{token}", post(put_probe_screen))
        .route("/api/jellies/{name}/try", post(try_steps))
        .route("/api/tries/{token}", post(put_trial_result))
        .route("/api/tries/{token}/do", post(trial_call))
        .route("/api/projects", get(projects).post(make_project))
        .route("/api/projects/{project}", axum::routing::delete(remove_project))
        .route("/api/projects/{project}/move", post(move_project))
        .route("/api/projects/{project}/design", get(get_design).put(put_design))
        .route("/p/{project}/DESIGN.md", get(get_design))
        .route("/p/{project}/design.css", get(design_css))
        .route("/api/asks", get(asks_list))
        .route("/api/show", post(show_full))
        .route("/api/jellies/{name}/asks", post(ask))
        .route("/api/jellies/{name}/asks/round", post(ask_round))
        .route("/api/jellies/{name}/asks/{id}/answer", post(answer))
        .route("/api/jellies/{name}/asks/{id}/unask", post(unask))
        .route("/api/jellies/{name}/send", post(send))
        .route("/api/wake", post(wake::add))
        .route("/api/version", get(update::version))
        .route("/api/update", post(update::update))
        .route("/api/restart", post(update::restart))
        .route("/api/jellies/{name}/working", post(working))
        .route("/api/jellies/{name}/comment", post(comment))
        .route("/api/jellies/{name}/resolve", post(resolve))
        .route("/api/jellies/{name}/reply", post(reply))
        .layer(axum::middleware::from_fn(local_only))
        .with_state(app)
}

/// jelly has no login, so only this machine may use it (#8, #66). A request's
/// `Host` must be a loopback address, which stops DNS rebinding. A browser's
/// `Origin`, when it sends one (every WebSocket and cross-site request does), must
/// be the daemon itself, so another site cannot read events or call verbs. The CLI
/// sends no `Origin` and passes. Another site's `<img>` sends no `Origin` either,
/// but the browser marks it `Sec-Fetch-Site: cross-site` (or `same-site`), so it
/// cannot start a render or write a file through a GET such as look (#148). The
/// daemon's own pages are `same-origin`, an address opened from outside a browser
/// is `none`, and the CLI sends no `Sec-Fetch-Site`. Pages may be framed only by
/// the daemon's own pages.
async fn local_only(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let allowed = {
        let text = |name| req.headers().get(name).and_then(|v: &axum::http::HeaderValue| v.to_str().ok());
        same_local_origin(text(header::HOST), text(header::ORIGIN)) && !matches!(text(header::HeaderName::from_static("sec-fetch-site")), Some("cross-site" | "same-site"))
    };
    if !allowed {
        return (StatusCode::FORBIDDEN, "jelly accepts requests from this machine only").into_response();
    }
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(header::CONTENT_SECURITY_POLICY, axum::http::HeaderValue::from_static("frame-ancestors 'self'"));
    resp
}

fn same_local_origin(host: Option<&str>, origin: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    local_authority(host) && origin.is_none_or(|o| o.strip_prefix("http://").is_some_and(|a| a.eq_ignore_ascii_case(host)))
}

fn local_authority(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

#[derive(Debug)]
enum ApiError {
    Store(Error),
    Conflict(String),
    Internal(String),
}

fn snapshot_changed() -> ApiError {
    ApiError::Conflict("the jelly changed; reload the page or retry".into())
}
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        ApiError::Store(e)
    }
}
impl From<String> for ApiError {
    fn from(e: String) -> Self {
        ApiError::Store(Error::Invalid(e))
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
            "kept": j.kept, "made_by": j.made_by, "owner": j.owner(), "updated_ms": j.updated_ms, "about": j.about, "project": j.project(), "archived": j.archived,
            "created_ms": j.created_ms,
            "open_comments": j.comments.iter().filter(|f| !f.resolved).count(),
            "replies_waiting": j.comments.iter().filter(|c| c.awaiting_person()).count(),
            "open_asks": j.asks.iter().filter(|a| a.is_open()).count(),
            "answered": j.asks.iter().filter(|a| a.answer.is_some() && a.unasked.is_none()).count(),
            // Threads about the whole jelly (no selector); the jelly page shows them in the header.
            "whole_jelly_comments": j.comments.iter().filter(|c| c.is_whole_jelly()).collect::<Vec<_>>(),
            "working": j.working,
            // When it will be archived if nobody keeps or touches it; null when it will not.
            "archives_ms": j.archives_ms(ARCHIVE_AFTER_MS),
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
    app.change(|s| s.make(&r.name, r.view, r.verbs, state, &by(r.by)?, r.replace, &r.about, &r.project))
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
    /// `jelly.do(verb, args, { quiet: true })`: not counted toward Send.
    #[serde(default)]
    quiet: bool,
}

async fn call(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<CallReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "do", "verb": r.verb, "args": r.args, "source": r.source, "quiet": r.quiet, "by": by(r.by)?})).await;
    }
    app.change(|s| s.do_verb_from(&name, &r.verb, r.args, &by(r.by)?, r.source, r.quiet))
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
    if let Some(live) = app.live(&name) {
        let op = json!({"op": "set", "pointer": r.pointer, "remove": value.is_none(), "value": value, "by": by(r.by)?});
        return share::forward(&app, &live, op).await;
    }
    app.change(|s| s.set_state(&name, &r.pointer, value, &by(r.by)?))
}

#[derive(Deserialize)]
struct ShapeReq {
    view: Option<String>,
    verbs: Option<String>,
    about: Option<String>,
    by: Option<String>,
}

async fn shape(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ShapeReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        // The relay trusts the daemon to have checked them, as `Store::shape` does.
        if let Some(src) = &r.verbs {
            crate::verbs::check(src).map_err(|e| Error::Invalid(format!("new verbs rejected, jelly unchanged: {e}")))?;
        }
        return share::forward(&app, &live, json!({"op": "shape", "view": r.view, "verbs": r.verbs, "about": r.about, "by": by(r.by)?})).await;
    }
    app.change(|s| s.shape(&name, r.view, r.verbs, r.about, &by(r.by)?))
}

#[derive(Deserialize)]
struct ByReq {
    by: Option<String>,
    #[serde(default)]
    kept: Option<bool>,
}

async fn undo(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.not_shared(&name, "be undone")?;
    app.change(|s| s.undo(&name, &by(r.by)?))
}

async fn keep(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.keep(&name, r.kept.unwrap_or(true), &by(r.by)?))
}

#[derive(Deserialize)]
struct CommentReq {
    #[serde(default)]
    anchor: Value,
    text: String,
    by: Option<String>,
}

async fn comment(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<CommentReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "comment", "anchor": r.anchor, "text": r.text, "by": by(r.by)?})).await;
    }
    app.change(|s| s.comment(&name, r.anchor, &r.text, &by(r.by)?))
}

#[derive(Deserialize)]
struct ResolveReq {
    id: String,
    by: Option<String>,
}

async fn resolve(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ResolveReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "resolve", "id": r.id, "by": by(r.by)?})).await;
    }
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
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "reply", "id": r.id, "text": r.text, "resolve": r.resolve, "by": by(r.by)?})).await;
    }
    app.change(|s| s.reply(&name, &r.id, &r.text, &by(r.by)?, r.resolve))
}

#[derive(Deserialize)]
struct ProjectReq {
    project: String,
    by: Option<String>,
}

async fn move_jelly(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ProjectReq>) -> Result<Json<Event>, ApiError> {
    app.not_shared(&name, "move")?;
    app.change(|s| s.set_project(&name, &r.project, &by(r.by)?))
}

/// Every project with the number of jellies in it, empty ones too.
async fn projects(State(app): State<Shared>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    Json(json!(store.projects().into_iter().map(|(name, jellies)| json!({"name": name, "jellies": jellies})).collect::<Vec<_>>()))
}

#[derive(Deserialize)]
struct MakeProjectReq {
    name: String,
}

async fn make_project(State(app): State<Shared>, Json(r): Json<MakeProjectReq>) -> Result<Json<Value>, ApiError> {
    app.store.lock().expect("store lock").make_project(&r.name)?;
    Ok(Json(json!({"name": r.name, "jellies": 0})))
}

async fn remove_project(State(app): State<Shared>, Path(project): Path<String>) -> Result<Json<Value>, ApiError> {
    app.store.lock().expect("store lock").remove_project(&project)?;
    Ok(Json(json!({"name": project})))
}

#[derive(Deserialize)]
struct MoveProjectReq {
    to: String,
    by: Option<String>,
}

/// Renames a project or merges it into another: each of its jellies moves, so
/// none of them may be shared, as for `move_jelly`.
async fn move_project(State(app): State<Shared>, Path(project): Path<String>, Json(r): Json<MoveProjectReq>) -> Result<Json<Value>, ApiError> {
    let by = by(r.by)?;
    let names: Vec<String> = app.store.lock().expect("store lock").list().filter(|j| j.project() == project).map(|j| j.name.clone()).collect();
    for name in &names {
        app.not_shared(name, "move")?;
    }
    app.change_many(|s| s.move_project(&project, &r.to, &by))
}

async fn remove(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<ByQ>) -> Result<Json<Event>, ApiError> {
    if name == PAGE {
        return Err(Error::Invalid("the jelly page cannot be removed; shape or undo it instead".into()).into());
    }
    app.not_shared(&name, "be removed")?;
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
    up.on_upgrade(move |socket| pump(app, socket, rx))
}

async fn pump(app: Shared, mut socket: WebSocket, mut rx: broadcast::Receiver<Event>) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            msg = socket.recv() => match msg {
                // The page says which jelly it shows and whether its tab is in front.
                Some(Ok(Message::Text(text))) => {
                    if let Ok(Value::Object(m)) = serde_json::from_str(&text) {
                        if let (Some(name), Some(visible)) = (m.get("open").and_then(Value::as_str), m.get("visible").and_then(Value::as_bool)) {
                            let notifies = m.get("notifies").and_then(Value::as_bool).unwrap_or(false);
                            app.open.lock().expect("open lock").insert(id, Open { name: name.into(), visible, notifies });
                        }
                    }
                }
                Some(Ok(_)) => {}
                _ => break,
            },
        }
    }
    app.open.lock().expect("open lock").remove(&id);
}

#[derive(Deserialize, Default)]
struct ViewQ {
    /// A `jelly try` token: the page does that trial's steps against the trial's state.
    #[serde(rename = "try")]
    trial: Option<String>,
    /// A `jelly look` token: the page posts its snapshot to that probe.
    probe: Option<String>,
}

/// Serves a jelly's view with the runtime injected, so `window.jelly` exists
/// before any of the view's own scripts run.
async fn view(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<ViewQ>) -> Result<Html<String>, ApiError> {
    let store = app.store.lock().expect("store lock");
    let j = store.get(&name)?;
    let owner = app.snapshot_owner(&name, j.view_rev)?;
    let mut boot = json!({"snapshotOwner": owner, "name": j.name, "about": j.about, "version": j.version, "viewRev": j.view_rev, "state": j.state, "comments": j.comments, "asks": j.asks, "working": j.working, "unsent": j.unsent});
    if let Some(token) = q.trial {
        if let Some(t) = app.trials.lock().expect("trials lock").get(&token).filter(|t| t.name == j.name) {
            boot["state"] = t.state.clone();
            boot["version"] = json!(t.version);
            boot["try"] = json!({"token": token, "steps": t.steps});
        }
    }
    let mut view = &j.view;
    let probes = app.probes.lock().expect("probes lock");
    if let Some((token, probe)) = q.probe.and_then(|t| probes.get(&t).filter(|p| p.name == j.name).map(|p| (t, p))) {
        boot["screenUrl"] = json!(format!("/api/probes/{token}"));
        if let Some(past) = &probe.past {
            view = &past.view;
            boot["state"] = past.state.clone();
            boot["version"] = json!(past.version);
            boot["past"] = json!(true);
        }
    }
    // `</` cannot appear inside the script, or state text could close the tag.
    let boot = boot.to_string().replace("</", "<\\/");
    let inject = format!("<script>window.__JELLY__={boot};</script><script src=\"/runtime.js\"></script>");
    Ok(Html(inject_head(view, &inject)))
}

/// Puts `inject` first thing in `<head>`, so it runs before the view's scripts.
fn inject_head(html: &str, inject: &str) -> String {
    let at = html.to_ascii_lowercase().find("<head>").map(|i| i + "<head>".len());
    match at {
        Some(i) => format!("{}{inject}{}", &html[..i], &html[i..]),
        None => format!("<!doctype html><meta charset=\"utf-8\">{inject}{html}"),
    }
}

/// The jelly page's Restore, and its inverse. The CLI has neither: a jelly nobody
/// keeps is put away by the weekly sweep, and any change brings it back.
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
    crate::store::valid_file(&file)?;
    if let Some(live) = app.live(&name) {
        share::put_asset(&app, &live, &file, body.to_vec()).await?;
    }
    app.change(|s| s.put_asset(&name, &file, &body, &by(q.by)?))
}

async fn remove_asset(State(app): State<Shared>, Path((name, file)): Path<(String, String)>, Query(q): Query<AssetQ>) -> Result<Json<Event>, ApiError> {
    crate::store::valid_file(&file)?;
    if let Some(live) = app.live(&name) {
        share::remove_asset(&app, &live, &file).await?;
    }
    app.change(|s| s.remove_asset(&name, &file, &by(q.by)?))
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
    if crate::store::valid_file(&file).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let dir = {
        let store = app.store.lock().expect("store lock");
        match store.get(&name) {
            Ok(_) => store.asset_dir(&name),
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        }
    };
    match tokio::fs::read(dir.join(&file)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, content_type(&file))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct ShareReq {
    /// Whom the link is for; without it, the view-only link.
    to: Option<String>,
    days: u64,
}

/// Mints a live link to the jelly on the relay, sharing it first if needed.
async fn share_live(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ShareReq>) -> Result<Json<Value>, ApiError> {
    let who = r.to.as_deref().map(str::trim).unwrap_or("");
    if r.to.is_some() && who.is_empty() {
        return Err(Error::Invalid("say whom the link is for: --to ana@example.com".into()).into());
    }
    if !(1..=365).contains(&r.days) {
        return Err(Error::Invalid("a link lasts 1 to 365 days".into()).into());
    }
    Ok(Json(share::share(&app, &name, who, r.days).await?))
}

#[derive(Deserialize)]
struct UnshareQ {
    /// Revoke only this person's link ("" for the view-only link).
    to: Option<String>,
}

#[derive(Deserialize, Default)]
struct SharesQ {
    #[serde(default)]
    here: bool,
}

/// Every shared jelly with its people, for the jelly page. `?here=true` asks
/// the relay who opened a link and who is there now; without it the answer
/// comes from this daemon alone, so asking often costs the relay nothing.
async fn shares(State(app): State<Shared>, Query(q): Query<SharesQ>) -> Result<Json<Value>, ApiError> {
    Ok(Json(share::list(&app, q.here).await?))
}

async fn unshare_live(State(app): State<Shared>, Path(name): Path<String>, Query(q): Query<UnshareQ>) -> Result<Json<Value>, ApiError> {
    let stop = q.to.as_deref().map(str::trim).map_or(share::Stop::Share, share::Stop::Link);
    Ok(Json(share::unshare(&app, &name, stop).await?))
}

#[derive(Deserialize)]
struct PinReq {
    hours: u64,
    days: u64,
}

/// A new PIN for a room on the jelly, sharing it first if needed.
async fn pin_live(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<PinReq>) -> Result<Json<Value>, ApiError> {
    if !(1..=24).contains(&r.hours) {
        return Err(Error::Invalid("a PIN lasts 1 to 24 hours".into()).into());
    }
    if !(1..=365).contains(&r.days) {
        return Err(Error::Invalid("a share lasts 1 to 365 days".into()).into());
    }
    Ok(Json(share::pin(&app, &name, r.hours, r.days).await?))
}

async fn unpin_live(State(app): State<Shared>, Path(name): Path<String>) -> Result<Json<Value>, ApiError> {
    Ok(Json(share::unshare(&app, &name, share::Stop::Pin).await?))
}

/// The port this request came in on, for rendering our own pages.
fn port_of(headers: &axum::http::HeaderMap) -> u16 {
    headers.get(header::HOST).and_then(|h| h.to_str().ok()).and_then(|h| h.rsplit(':').next()).and_then(|p| p.parse().ok()).unwrap_or(7878)
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
struct ShowReq {
    name: Option<String>,
    by: Option<String>,
}

/// Opens a jelly full size on the person's screen (or closes it with no name).
async fn show_full(State(app): State<Shared>, Json(r): Json<ShowReq>) -> Result<Json<Value>, ApiError> {
    if let Some(n) = &r.name {
        app.store.lock().expect("store lock").get(n)?;
    }
    let Json(ev) = app.change(|s| s.do_verb(PAGE, "show", json!({"name": r.name}), &by(r.by)?))?;
    let mut out = json!(ev);
    // `show` only moves the jelly page, so it reaches nobody unless that page is open.
    out["visible"] = json!(app.visible(PAGE));
    Ok(Json(out))
}

// ---- DESIGN.md per project --------------------------------------------------

async fn put_design(State(app): State<Shared>, Path(project): Path<String>, body: String) -> Result<StatusCode, ApiError> {
    let dir = app.store.lock().expect("store lock").project_dir(&project)?;
    std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("DESIGN.md"), body)).map_err(|e| Error::Invalid(format!("could not save DESIGN.md: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_design(State(app): State<Shared>, Path(project): Path<String>) -> Response {
    let Ok(dir) = app.store.lock().expect("store lock").project_dir(&project) else { return StatusCode::NOT_FOUND.into_response() };
    match std::fs::read_to_string(dir.join("DESIGN.md")) {
        Ok(t) => ([(header::CONTENT_TYPE, "text/markdown; charset=utf-8")], t).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, format!("project {project} has no DESIGN.md")).into_response(),
    }
}

/// The DESIGN.md tokens as CSS variables: `--color-primary`, `--rounded-md`,
/// `--spacing-sm`, `--typography-h1-font-size`, …
async fn design_css(State(app): State<Shared>, Path(project): Path<String>) -> Response {
    let Ok(dir) = app.store.lock().expect("store lock").project_dir(&project) else { return StatusCode::NOT_FOUND.into_response() };
    let text = std::fs::read_to_string(dir.join("DESIGN.md")).unwrap_or_default();
    let css = format!(":root {{\n{}}}\n", crate::design::css_vars(&text));
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], css).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::PERSON;

    fn app() -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let (events, _) = broadcast::channel(32);
        let app = Arc::new(App { store: Mutex::new(Store::open(dir.path()).unwrap()), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default(), update: Mutex::default() });
        (dir, app)
    }

    #[tokio::test]
    async fn asks_and_owner_follow_the_maker_or_last_shaper() {
        let (_dir, app) = app();
        {
            let mut store = app.store.lock().unwrap();
            for (name, agent) in [("a", "ana"), ("b", "bo")] {
                store.make(name, "".into(), "const verbs = {};".into(), json!({}), agent, false, "", "p").unwrap();
                store.ask(&format!("p:{name}"), json!({"title": format!("Which {name}?")}), agent).unwrap();
            }
        }
        let asked = |mine: Option<&str>| {
            let app = app.clone();
            let mine = mine.map(String::from);
            async move {
                let q = ProjectQ { project: Some("p".into()), answered: false, mine };
                asks_list(State(app), Query(q)).await.0.as_array().unwrap().iter()
                    .map(|i| i["jelly"].as_str().unwrap().to_string()).collect::<Vec<_>>()
            }
        };
        assert_eq!(asked(Some("ana")).await, ["p:a"]);
        assert_eq!(asked(Some("bo")).await, ["p:b"]);
        assert_eq!(asked(None).await.len(), 2);

        app.store.lock().unwrap().shape("p:a", Some("<p>v2</p>".into()), None, None, "bo").unwrap();
        assert!(asked(Some("ana")).await.is_empty());
        let owners: Vec<_> = list(State(app.clone())).await.0.as_array().unwrap().iter()
            .map(|j| (j["name"].as_str().unwrap().to_string(), j["owner"].as_str().unwrap().to_string())).collect();
        assert!(owners.contains(&("p:a".into(), "bo".into())) && owners.contains(&("p:b".into(), "bo".into())));
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
        let absolute = dir.parent().unwrap().join("secret.txt").to_string_lossy().into_owned();
        for file in [".hidden", ".", "..", "../secret.txt", "sub/secret.txt", "..\\secret.txt", "sub\\secret.txt", absolute.as_str()] {
            let response = asset(State(app.clone()), Path(("test:x".into(), file.into()))).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{file}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn asset_urls_only_reach_an_existing_jellys_assets() {
        let (home, app) = app();
        create_page(&app, "PAGE", false).await;
        for (file, bytes) in [("shot.png", &b"png bytes"[..]), ("notes..v2.txt", b"two dots")] {
            let _ = put_asset(State(app.clone()), Path(("test:x".into(), file.into())),
                Query(AssetQ { by: Some("test".into()) }), Bytes::copy_from_slice(bytes)).await.unwrap();
        }
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("note.txt"), b"SECRET-OUTSIDE").unwrap();
        for dir in [home.path().join("beside"), home.path().join("assets").join(crate::disk::name("test:missing"))] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("note.txt"), b"SECRET-OUTSIDE").unwrap();
        }

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(std::future::IntoFuture::into_future(axum::serve(listener, router(app))));
        let fetch = |path: String| {
            let url = format!("{base}{path}");
            tokio::task::spawn_blocking(move || match ureq::get(&url).call() {
                Ok(r) | Err(ureq::Error::Status(_, r)) => {
                    let (status, kind) = (r.status(), r.content_type().to_string());
                    let mut body = Vec::new();
                    r.into_reader().read_to_end(&mut body).unwrap();
                    (status, kind, body)
                }
                Err(e) => panic!("{url}: {e}"),
            })
        };

        let absolute = outside.path().to_str().unwrap().replace('/', "%2F");
        for name in [absolute.as_str(), "..%2Fbeside", "test%3Amissing"] {
            let (status, _, body) = fetch(format!("/j/{name}/assets/note.txt")).await.unwrap();
            assert_eq!(status, 404, "{name}");
            assert!(!String::from_utf8_lossy(&body).contains("SECRET-OUTSIDE"), "{name}");
        }
        for (file, kind, bytes) in [("shot.png", "image/png", &b"png bytes"[..]), ("notes..v2.txt", "text/plain", b"two dots")] {
            assert_eq!(fetch(format!("/j/test:x/assets/{file}")).await.unwrap(), (200, kind.to_string(), bytes.to_vec()));
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
        let Html(html) = view(State(app.clone()), Path(name.into()), Query(ViewQ::default())).await.unwrap();
        let boot = html.split_once("window.__JELLY__=").unwrap().1.split_once(";</script>").unwrap().0;
        serde_json::from_str::<Value>(boot).unwrap()["snapshotOwner"].as_str().unwrap().into()
    }

    /// A snapshot from one of `jelly look`'s own light renders.
    fn snapshot(owner: &str, text: &str, version: u64) -> Screen {
        serde_json::from_value(json!({"owner": owner, "text": text, "width": 1280, "height": 900, "version": version, "problems": []})).unwrap()
    }

    async fn post(app: &Shared, sc: Screen) -> Result<StatusCode, ApiError> {
        put_screen(State(app.clone()), Path("test:x".into()), Json(sc)).await
    }

    /// Posts the light snapshot and its dark twin, as `jelly look`'s two renders do,
    /// so a later `look` needs no browser.
    async fn post_both(app: &Shared, sc: Screen) -> Result<StatusCode, ApiError> {
        let mut dark = sc.clone();
        dark.dark = true;
        post(app, dark).await?;
        post(app, sc).await
    }

    async fn seen(app: &Shared) -> String {
        let response = look(State(app.clone()), Path("test:x".into()),
            Query(LookQ { width: Some(1280), height: None, png: false, full: false, json: false, compare: None, save: None }), axum::http::HeaderMap::new()).await.unwrap();
        String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn bundled_runtime_posts_served_ownership_and_reloads_obsolete_pages() {
        let (_dir, app) = app();
        create_page(&app, "PAGE", false).await;
        let Html(html) = view(State(app.clone()), Path("test:x".into()), Query(ViewQ::default())).await.unwrap();
        let boot = html.split_once("window.__JELLY__=").unwrap().1.split_once(";</script>").unwrap().0;
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        let eval = |script: &str| ctx.with(|ctx| {
            ctx.eval::<(), _>(script).unwrap_or_else(|err| panic!("{err}: {:?}", ctx.catch()));
        });
        eval(include_str!("../tests/runtime-browser.js"));
        eval(&format!("window.__JELLY__={boot};"));
        eval(&RUNTIME_JS);
        eval("loaded();");
        while rt.execute_pending_job().unwrap() {}
        let request: Value = ctx.with(|ctx| {
            serde_json::from_str(&ctx.eval::<String, _>("JSON.stringify(requests[0])").unwrap()).unwrap()
        });
        assert_eq!(request["url"], "/api/jellies/test%3Ax/screen");
        assert_eq!(post(&app, serde_json::from_value(request["body"].clone()).unwrap()).await.unwrap(), StatusCode::NO_CONTENT);
        eval("if (reloads !== 0) throw Error('accepted post reloaded');");
        eval("message({jelly:'other:x', kind:'make', view_rev:1}); if (reloads !== 0) throw Error('other jelly reloaded');");
        eval("message({jelly:'test:x', kind:'make', view_rev:1, version:1}); if (reloads !== 1) throw Error('same-revision replacement did not reload');");
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
                if png { post_both(&app, snapshot(&current, "OLD PAGE", 1)).await.unwrap(); }
                let (started, suspended) = tokio::sync::oneshot::channel();
                let (resume, resumed) = tokio::sync::oneshot::channel();
                let mut rendezvous = Some((started, resumed));
                let seeing = look_at_width(app.clone(), "test:x".into(),
                    LookQ { width: Some(1280), height: None, png, full: false, json: false, compare: None, save: None },
                    move |_, _, path, dark| {
                        assert_eq!(path.is_some(), png);
                        let light = (!dark).then(|| rendezvous.take().expect("one light render"));
                        async move {
                            let Some((started, resumed)) = light else { return Err("no dark render here".into()) };
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
                assert_eq!(result.expect_err("obsolete look must fail").into_response().status(), StatusCode::CONFLICT,
                    "png={png}, replace={replace}");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn see_png_reports_only_a_screenshot_the_browser_made() {
        use std::os::unix::fs::PermissionsExt;
        // The daemon writes the png from what the browser sends over DevTools, so
        // a browser that quits, even with success, made none.
        for (exit, old_png) in [(3, false), (3, true), (0, false)] {
            let (dir, app) = app();
            create_page(&app, "PAGE", false).await;
            let current = owner(&app, "test:x").await;
            post_both(&app, snapshot(&current, "PAGE", 1)).await.unwrap();
            let png = dir.path().join("renders").join(format!("{}-1280.png", crate::disk::name("test:x")));
            if old_png {
                std::fs::create_dir_all(png.parent().unwrap()).unwrap();
                std::fs::write(&png, b"old").unwrap();
            }
            let browser = dir.path().join("browser.sh");
            std::fs::write(&browser, format!("#!/bin/sh\nexit {exit}\n")).unwrap();
            std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
            let browser = browser.to_str().unwrap().to_string();
            let response = look_at_width(app.clone(), "test:x".into(),
                LookQ { width: Some(1280), height: None, png: true, full: false, json: false, compare: None, save: None },
                move |width, height, path, dark| {
                    let browser = browser.clone();
                    async move {
                        run_browser(&browser, 0, "test:x", None, width, height, path.as_deref(), dark, RENDER_LIMIT).await
                    }
                }).await.unwrap();
            let out = String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
            assert!(!out.contains("png      "), "old_png={old_png}: {out}");
            assert!(out.contains(&format!("failed   no screenshot: the browser failed (exit status: {exit}). Run `jelly look` again; if it keeps failing, report it")), "{out}");
            assert!(!out.contains("!  "), "{out}");
        }
    }

    /// The light render takes the png. When only its screenshot fails, what the
    /// page measured still counts, and only the png is reported missing.
    #[tokio::test]
    async fn a_failed_screenshot_keeps_the_snapshot_it_came_with() {
        let (_dir, app) = app();
        create_page(&app, "PAGE", false).await;
        let current = owner(&app, "test:x").await;
        let posting = app.clone();
        let response = look_at_width(app.clone(), "test:x".into(),
            LookQ { width: Some(1280), height: None, png: true, full: false, json: false, compare: None, save: None },
            move |_, _, path, dark| {
                let (app, owner) = (posting.clone(), current.clone());
                async move {
                    let mut sc = snapshot(&owner, "MEASURED", 1);
                    sc.dark = dark;
                    post(&app, sc).await.unwrap();
                    if path.is_some() { Err("the screenshot broke".to_string()) } else { Ok(()) }
                }
            }).await.unwrap();
        let out = String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(out.contains("MEASURED") && out.contains("failed   no screenshot: the screenshot broke."), "{out}");
        assert!(!out.contains("not checked") && !out.contains("png      ") && !out.contains("!  "), "{out}");
    }

    #[tokio::test]
    async fn cold_see_waits_for_matching_version_and_width() {
        for wrong_width in [false, true] {
            let (_dir, app) = app();
            create_page(&app, "PAGE", false).await;
            let current = owner(&app, "test:x").await;
            let mut dark = snapshot(&current, "DARK", 1);
            dark.dark = true;
            post(&app, dark).await.unwrap();
            let (rendered, polling) = tokio::sync::oneshot::channel();
            let mut rendered = Some(rendered);
            let posting_app = app.clone();
            let posting_owner = current.clone();
            let seeing = look_at_width(app.clone(), "test:x".into(),
                LookQ { width: Some(1280), height: None, png: false, full: false, json: false, compare: None, save: None },
                move |_, _, path, dark| {
                    assert!(path.is_none() && !dark);
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
        assert_eq!(post_both(&app, snapshot(&old, "OLD PAGE", 1)).await.unwrap(), StatusCode::NO_CONTENT);
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
            post_both(&app, snapshot(&current, text, 1)).await.unwrap();
            assert!(post(&app, snapshot(&previous, "OLD PAGE", 1)).await.is_err());
            let output = seen(&app).await;
            assert!(output.contains(text), "{output}");
            assert!(!output.contains("OLD PAGE"), "{output}");
            assert!(app.validate_render("test:x", &previous, 1).is_err());
        }
        assert!(serde_json::from_value::<Screen>(json!({"text": "tokenless", "width":1280, "height":900, "version":1})).is_err());
        let dir = app.store.lock().unwrap().home().to_path_buf();
        let (events, _) = broadcast::channel(32);
        let restarted = Arc::new(App { store: Mutex::new(Store::open(&dir).unwrap()), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default(), update: Mutex::default() });
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
        post_both(&app, snapshot(&first, "OLD PAGE", 1)).await.unwrap();
        assert!(app.change(|s| s.make("test:x", "bad".into(), "invalid JS!".into(), json!({}), "test", true, "", "")).is_err());
        assert!(app.change(|s| s.shape("test:x", Some("bad".into()), Some("invalid JS!".into()), None, "test")).is_err());
        assert!(seen(&app).await.contains("OLD PAGE"));
        let _ = app.change(|s| s.set_state("test:x", "", Some(json!({"n":1})), "test")).unwrap();
        assert_eq!(owner(&app, "test:x").await, first);
        assert_eq!(app.screen("test:x", &first).unwrap().unwrap().version, 1);
        assert!(app.validate_render("test:x", &first, 1).is_err());
        let _ = app.change(|s| s.shape("test:x", Some("<h1>NEW PAGE</h1>".into()), None, None, "test")).unwrap();
        assert!(post(&app, snapshot(&first, "OLD PAGE", 1)).await.is_err());
        let shaped = owner(&app, "test:x").await;
        let _ = app.change(|s| s.undo("test:x", "test")).unwrap();
        assert!(post(&app, snapshot(&shaped, "NEW PAGE", 3)).await.is_err());
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
        let _ = app.change(|s| s.shape("test:x", Some("different".into()), None, None, "test")).unwrap();
        let different = owner(&app, "test:x").await;
        let _ = app.change_many(|s| s.ask_round("test:x", &spec, ASK_VIEW, ASK_VERBS, "", "test")).unwrap();
        assert!(post(&app, snapshot(&different, "different", 1)).await.is_err());
        assert!(app.screen("test:x", &original).is_err());
        let current = owner(&app, "test:x").await;
        let version = app.store.lock().unwrap().get("test:x").unwrap().version;
        let asked = now_ms();
        let mut sc = snapshot(&current, "ASK PAGE", version);
        let shown = |anchor: &str| -> Vec<AskShown> { serde_json::from_value(json!([{"id": "a2", "drawn": true, "anchor": anchor, "at": "#q-q1"}])).unwrap() };
        sc.asks = shown("missing");
        post(&app, sc).await.unwrap();
        assert_eq!(ask_shown(&app, 1, "test:x", "a2", version, asked).await.unwrap().anchor, AskAnchor::Missing);
        // The wait must ignore a screen from before the ask and an older state.
        app.screens.lock().unwrap().get_mut("test:x").unwrap().screen.as_mut().unwrap().at_ms = asked - 1;
        let checking = ask_shown(&app, 1, "test:x", "a2", version, asked);
        let update = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let mut sc = snapshot(&current, "AFTER ASK", version);
            sc.asks = shown("matched");
            post(&app, sc).await.unwrap();
        };
        let (result, _) = tokio::join!(checking, update);
        assert_eq!(result.unwrap().anchor, AskAnchor::Matched);
        let checking = ask_shown(&app, 1, "test:x", "a2", version + 1, asked);
        let update = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let mut sc = snapshot(&current, "NEW STATE", version + 1);
            sc.asks = shown("invalid");
            post(&app, sc).await.unwrap();
        };
        let (result, _) = tokio::join!(checking, update);
        assert_eq!(result.unwrap().anchor, AskAnchor::Invalid);
        let checking = ask_shown(&app, 1, "test:x", "a2", version + 2, now_ms());
        let replace = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            create_page(&app, "REPLACED", true).await;
        };
        let (result, _) = tokio::join!(checking, replace);
        assert!(result.unwrap_err().contains("changed"));
    }

    #[tokio::test]
    async fn the_inbox_reads_back_to_the_cursor_and_keeps_the_person_after_a_trim() {
        let (_d, app) = app();
        let verbs = "const verbs = { label(s, { id }) { s[id] = 1; } };";
        let ask = |since: u64| inbox(State(app.clone()), Query(InboxQ { project: Some("p".into()), since: Some(since) }));
        let ids = |r: Result<Json<Value>, ApiError>| r.unwrap().0.as_array().unwrap().iter().map(|e| e["args"]["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let since = {
            let mut s = app.store.lock().unwrap();
            s.make("j", "".into(), verbs.into(), json!({}), "agent", false, "", "p").unwrap();
            s.do_verb("p:j", "label", json!({"id": "first"}), PERSON).unwrap().at_ms
        };
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        {
            let mut s = app.store.lock().unwrap();
            s.do_verb("p:j", "label", json!({"id": "second"}), PERSON).unwrap();
            for i in 0..200 {
                s.do_verb("p:j", "label", json!({"id": format!("agent{i}")}), "agent").unwrap();
            }
        }
        assert_eq!(ids(ask(since).await), ["second"]);
        assert_eq!(ids(ask(0).await), ["first", "second"]);
        app.store.lock().unwrap().trim_events(2_000, 1 << 20).unwrap();
        assert_eq!(ids(ask(0).await), ["first", "second"], "the person's events outlive a trim");
    }

    #[tokio::test]
    async fn timestamp_inbox_keeps_events_across_a_backward_clock_step() {
        let (dir, app) = app();
        let (recent, later) = {
            let mut store = app.store.lock().unwrap();
            store.make("clock", "".into(), "const verbs = { tick(s) { s.n = 1; } };".into(), json!({}), "agent", false, "", "test").unwrap();
            let mut recent = store.do_verb("test:clock", "tick", json!({}), PERSON).unwrap();
            recent.state = None;
            let mut later = recent.clone();
            later.seq += 1;
            later.at_ms -= 1;
            let mut older = later.clone();
            older.seq += 1;
            older.at_ms = 1;
            std::fs::write(store.events_path(), format!("{}\n{}\n{}\n", serde_json::to_string(&recent).unwrap(), serde_json::to_string(&later).unwrap(), serde_json::to_string(&older).unwrap())).unwrap();
            (recent, later)
        };
        *app.store.lock().unwrap() = Store::open(dir.path()).unwrap();
        let path = app.store.lock().unwrap().events_path();
        let mut bytes = std::fs::read(&path).unwrap();
        let old_start = bytes[..bytes.len() - 1].iter().rposition(|&b| b == b'\n').unwrap() + 1;
        bytes[old_start] = 0xff;
        std::fs::write(&path, bytes).unwrap();
        let empty = inbox(State(app.clone()), Query(InboxQ { project: None, since: Some(recent.at_ms) })).await.unwrap();
        assert!(empty.0.as_array().unwrap().is_empty());
        let result = inbox(State(app.clone()), Query(InboxQ { project: Some("test".into()), since: Some(recent.at_ms - 2) })).await.unwrap();
        assert_eq!(result.0.as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect::<Vec<_>>(), [recent.seq, later.seq]);
        let mut shifted = std::fs::read(&path).unwrap();
        shifted.insert(0, b'\n');
        std::fs::write(&path, shifted).unwrap();
        assert!(inbox(State(app.clone()), Query(InboxQ { project: None, since: Some(recent.at_ms - 2) })).await.is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(inbox(State(app.clone()), Query(InboxQ { project: None, since: Some(u64::MAX) })).await.is_err());
        let reader = crate::store::InboxReader::Named { name: "unreadable".into() };
        assert!(app.store.lock().unwrap().read_inbox(&crate::store::InboxRequest { project: "test".into(), reader, mine: None }).is_err());
    }

    #[test]
    fn see_history_does_not_read_unrelated_retained_events() {
        let (dir, app) = app();
        let mut store = app.store.lock().unwrap();
        for name in ["quiet", "busy"] {
            store.make(name, "".into(), "const verbs = {};".into(), json!({}), "agent", false, "", "test").unwrap();
        }
        for i in 0..10 {
            store.set_state("test:quiet", "/n", Some(json!(i)), PERSON).unwrap();
        }
        let last = store.set_state("test:quiet", "/n", Some(json!(10)), "agent").unwrap();
        for i in 0..200 {
            store.set_state("test:busy", "/n", Some(json!(i)), "agent").unwrap();
        }
        let path = store.events_path();
        drop(store);
        // Removing the backing log makes any per-look log read observable.
        let saved = std::fs::read(&path).unwrap();
        for restarted in [false, true] {
            if restarted {
                *app.store.lock().unwrap() = Store::open(dir.path()).unwrap();
            }
            std::fs::remove_file(&path).unwrap();
            let (event, person) = app.store.lock().unwrap().recent_events("test:quiet");
            assert_eq!(event.unwrap().seq, last.seq);
            assert_eq!(person.iter().map(|e| e.version).collect::<Vec<_>>(), (4..12).collect::<Vec<_>>());
            assert!(app.store.lock().unwrap().recent_events("test:busy").1.is_empty());
            assert!(app.store.lock().unwrap().recent_events("test:missing").0.is_none());
            std::fs::write(&path, &saved).unwrap();
        }
        let mut store = app.store.lock().unwrap();
        store.trim_events(1, 0).unwrap();
        assert!(store.recent_events("test:quiet").0.is_none());
        assert!(store.recent_events("test:quiet").1.is_empty());
        assert!(store.recent_events("test:busy").0.is_some());
        assert!(store.recent_events("test:busy").0.unwrap().state.is_none());
    }

    #[tokio::test]
    async fn see_names_who_made_the_current_version_even_after_a_trim() {
        let (_d, app) = app();
        let verbs = "const verbs = { label(s, { id }) { s[id] = 1; } };";
        {
            let mut s = app.store.lock().unwrap();
            for name in ["x", "y"] {
                s.make(name, "".into(), verbs.into(), json!({}), "agent", false, "", "test").unwrap();
            }
            s.do_verb("test:x", "label", json!({"id": "mine"}), PERSON).unwrap();
            s.do_verb("test:x", "label", json!({"id": "hers"}), "person:ana").unwrap();
            s.comment("test:x", json!({"selector": "#hers"}), "bigger", "person:ana").unwrap();
            s.do_verb("test:x", "label", json!({"id": "theirs"}), "agent").unwrap();
            for i in 0..100 {
                s.do_verb("test:y", "label", json!({"id": format!("n{i}")}), "agent").unwrap();
            }
        }
        let version = app.store.lock().unwrap().get("test:x").unwrap().version;
        let current = owner(&app, "test:x").await;
        post_both(&app, snapshot(&current, "PAGE", version)).await.unwrap();
        let see_x = || async {
            let response = look_at_width(app.clone(), "test:x".into(), LookQ { width: Some(1280), height: None, png: false, full: false, json: false, compare: None, save: None },
                |_, _, _, _| async { Err("no browser".to_string()) }).await.unwrap();
            String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
        };
        let header = |out: &str| out.lines().next().unwrap().to_string();
        let path = app.store.lock().unwrap().events_path();
        let saved = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let out = see_x().await;
        std::fs::write(&path, saved).unwrap();
        assert!(header(&out).ends_with("last change just now by agent"), "{out}");
        assert!(out.contains(r#"person   just now label {"id":"mine"}"#), "{out}");
        assert!(out.contains(r#"person   just now ana label {"id":"hers"}"#), "{out}");
        assert!(out.contains(r#"comment  c1 on #hers by ana: "bigger""#), "{out}");
        app.store.lock().unwrap().trim_events(2_000, 1 << 20).unwrap();
        let out = see_x().await;
        assert!(header(&out).ends_with("last change just now"), "the agent's change was trimmed, so no author: {out}");
        assert!(out.contains(r#"person   just now label {"id":"mine"}"#), "{out}");
    }
}
