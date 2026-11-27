//! Live share links (#108). While a jelly is shared, the copy on your relay is
//! the live one: its verbs run there, people act through their links, and the
//! daemon forwards local changes to it. Every change the relay makes comes back
//! over one WebSocket per shared jelly and is mirrored into the local store, so
//! `inbox`, `wait` and `look` work as before. When the laptop sleeps the relay
//! keeps going; on reconnecting the daemon catches up from the last event it saw.
//! The daemon tells the relay what its record has saved, and the relay deletes
//! those of them older than a week; a daemon further behind catches up from the
//! whole jelly and records a `missed` event saying so.

use super::{content_type, ApiError, App, Shared, RUNTIME_JS};
use crate::relay::{self, Fail, Record, Relay};
use crate::op::Change;
use crate::store::{Error, Event, Mirrored, MirroredDoc, PAGE};
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

/// A shared jelly in the daemon: its record, and the task that mirrors it.
pub(crate) struct Live {
    /// Held while changes are mirrored, so they apply once and in order. Never
    /// held while a change waits on the relay: the socket would wait behind it.
    record: tokio::sync::Mutex<Record>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Each change as mirrored here, with the relay's seq, for `forward` to answer with.
    mirrored: broadcast::Sender<(u64, Event)>,
    /// The relay's seq as the record on disk has it. The socket tells the
    /// relay, which then deletes the events up to it.
    saved: AtomicU64,
}

/// Events from the relay, with the jelly's parts as they are after the last one.
#[derive(Deserialize)]
struct Batch {
    events: Vec<Mirrored>,
    #[serde(default)]
    doc: MirroredDoc,
}

fn home(app: &App) -> relay::Home {
    relay::Home(app.store.lock().expect("store lock").home().to_path_buf())
}

fn log(line: std::fmt::Arguments) {
    let _ = writeln!(std::io::stderr().lock(), "jelly: {line}");
}

/// Runs a blocking relay call off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, Fail> + Send + 'static) -> Result<T, Fail> {
    tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(Fail::Other(e.to_string())))
}

fn api(e: Fail) -> ApiError {
    match e {
        Fail::Refused(m) => Error::Invalid(m).into(),
        e => ApiError::Internal(e.to_string()),
    }
}

/// Picks up the shares this home had when the daemon stopped.
pub(crate) fn resume(app: &Shared) {
    let records = match home(app).records() {
        Ok(r) => r,
        Err(e) => return log(format_args!("could not read shares: {e}")),
    };
    for record in records {
        let name = record.jelly.clone();
        if app.store.lock().expect("store lock").get(&name).is_err() {
            log(format_args!("{name} is shared but no longer here; `jelly unshare` cannot reach it, so the share runs until it expires"));
            continue;
        }
        run(app, &track(app, record));
    }
}

/// From here on, local changes to the jelly go to the relay.
fn track(app: &Shared, record: Record) -> Arc<Live> {
    let name = record.jelly.clone();
    let saved = AtomicU64::new(record.seq);
    let live = Arc::new(Live { record: tokio::sync::Mutex::new(record), task: Mutex::new(None), mirrored: broadcast::channel(1024).0, saved });
    app.shares.lock().expect("shares lock").insert(name, live.clone());
    live
}

fn run(app: &Shared, live: &Arc<Live>) {
    *live.task.lock().expect("task lock") = Some(tokio::spawn(sync(app.clone(), live.clone())));
}

/// Ends a share here: no more forwarding or mirroring. The jelly stays as mirrored.
fn stop(app: &App, record: &Record) {
    if let Some(live) = app.shares.lock().expect("shares lock").remove(&record.jelly) {
        if let Some(task) = live.task.lock().expect("task lock").take() {
            task.abort();
        }
    }
    if let Err(e) = home(app).forget(&record.share) {
        log(format_args!("{e}"));
    }
}

/// Mirrors the batch's events that are new here into the store, in order, and
/// returns them with the relay's seq. A `whole` batch (the catch-up a socket
/// starts with, or the last one of an unshare) carries the whole jelly. When
/// the relay no longer has the events right after this home's (it keeps those
/// a daemon saved for a week), or this home lost changes its record says it
/// had, the jelly lands as the relay has it, with one `missed` event for the
/// agent. Any other batch with a gap is None, and the socket starts again.
fn apply(app: &App, live: &Live, record: &mut Record, batch: &Batch, whole: bool) -> Result<Option<Vec<(u64, Event)>>, Error> {
    let fresh: Vec<&Mirrored> = batch.events.iter().filter(|e| e.seq > record.seq).collect();
    let missed = fresh.first().map_or(0, |e| e.seq - record.seq - 1);
    if missed > 0 && !whole {
        return Ok(None);
    }
    let mut store = app.store.lock().expect("store lock");
    let behind = whole && fresh.is_empty() && store.behind(&record.jelly, &batch.doc);
    if fresh.is_empty() && !behind {
        return Ok(Some(Vec::new()));
    }
    let mut out = Vec::new();
    let mut failed = None;
    for (i, e) in fresh.iter().enumerate() {
        let last = i + 1 == fresh.len();
        let doc = (last && missed == 0).then_some(&batch.doc);
        match store.mirror(&record.jelly, e, doc) {
            // After a gap the record moves past the last event only once the jelly has landed.
            Ok(ev) => { if doc.is_some() || !last { record.seq = e.seq; } out.push((e.seq, ev)); }
            Err(err) => { failed = Some(err); break; }
        }
    }
    if failed.is_none() && (missed > 0 || behind) {
        let seq = fresh.last().map_or(record.seq, |e| e.seq);
        match store.caught_up(&record.jelly, missed, &batch.doc) {
            Ok(ev) => { record.seq = seq; out.push((seq, ev)); }
            Err(err) => failed = Some(err),
        }
    }
    let events: Vec<Event> = out.iter().map(|(_, e)| e.clone()).collect();
    app.invalidate_screens(&store, &events);
    drop(store);
    for ev in events {
        let _ = app.events.send(ev);
    }
    for change in &out {
        let _ = live.mirrored.send(change.clone());
    }
    match home(app).save(record) {
        Ok(()) => live.saved.store(record.seq, Ordering::SeqCst),
        Err(e) => log(format_args!("{e}")),
    }
    failed.map_or(Ok(Some(out)), Err)
}

/// Makes one change on the relay and returns its event as mirrored here. The
/// socket mirrors every change, this one too, so people's changes keep coming
/// in however many local ones are waiting on the relay.
pub(crate) async fn forward(app: &Shared, live: &Live, change: &Change) -> Result<Json<Event>, ApiError> {
    let relay = home(app).relay()?;
    // Subscribed before the change is made, so its event cannot pass unseen.
    let mut mirrored = live.mirrored.subscribe();
    // Waits for a publish under way, so the change reaches the relay after it.
    let share = live.record.lock().await.share.clone();
    let (r, change) = (relay.clone(), change.clone());
    let answer = match blocking(move || r.change(&share, &change)).await {
        Err(Fail::Gone) => {
            let record = live.record.lock().await;
            stop(app, &record);
            return Err(Error::Invalid(format!("the share of {} has ended (expired, or unshared elsewhere); it is a local jelly again, so run that again", record.jelly)).into());
        }
        a => a.map_err(api)?,
    };
    let batch: Batch = serde_json::from_value(answer).map_err(|e| ApiError::Internal(format!("{}: unreadable answer: {e}", relay.url)))?;
    let seq = batch.events.last().map(|e| e.seq).ok_or_else(|| ApiError::Internal("the relay made no change".into()))?;
    let arrived = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match mirrored.recv().await {
                Ok((s, ev)) if s == seq => return Some(ev),
                Err(broadcast::error::RecvError::Closed) => return None,
                _ => {}
            }
        }
    });
    match arrived.await {
        Ok(Some(ev)) => Ok(Json(ev)),
        _ => Err(ApiError::Internal(format!("the relay made change {seq}, but its socket has not brought it here yet"))),
    }
}

/// Mints (or prints again) the link for `who`, sharing the jelly first if it is
/// not shared yet. `who` is "" for the view-only link.
pub(crate) async fn share(app: &Shared, name: &str, who: &str, days: u64) -> Result<Value, ApiError> {
    let relay = home(app).relay()?;
    let _one = app.sharing.lock().await;
    let live = live_or_publish(app, &relay, name).await?;
    let mut record = live.record.lock().await;
    let secret = match record.links.get(who) {
        Some(s) => s.clone(),
        None => relay::token(22)?,
    };
    let (r, share, s, w) = (relay.clone(), record.share.clone(), secret.clone(), who.to_string());
    let made = blocking(move || r.mint(&share, &s, &w, days)).await.map_err(api)?;
    record.links.insert(who.into(), secret.clone());
    record.expires_at = made["expires_at"].as_u64().or(record.expires_at);
    home(app).save(&record)?;
    Ok(json!({"jelly": name, "url": relay::link(relay.pages(&made), &record.share, &secret), "who": who, "expires_at": made["expires_at"]}))
}

/// Puts the jelly in the room of the relay's PIN, sharing it first if it is not
/// shared yet. With no PIN live the relay makes one; with one live, `pin` is
/// null and the jelly joins its room. People type the PIN at the relay's join
/// page with their email and get their own link, so they act as
/// `person:<email>` exactly as with `--to`; people already in the room open
/// the jelly from their page without it. `room` names the room's jellies.
pub(crate) async fn pin(app: &Shared, name: &str, hours: u64, days: u64) -> Result<Value, ApiError> {
    let relay = home(app).relay()?;
    let _one = app.sharing.lock().await;
    let live = live_or_publish(app, &relay, name).await?;
    let share = live.record.lock().await.share.clone();
    let r = relay.clone();
    let made = blocking(move || r.pin(&share, hours, days)).await.map_err(api)?;
    {
        let mut record = live.record.lock().await;
        record.expires_at = made["expires_at"].as_u64().or(record.expires_at);
        home(app).save(&record)?;
    }
    let room: Vec<&Value> = made["room"].as_array().into_iter().flatten().map(|j| &j["name"]).collect();
    Ok(json!({"jelly": name, "pin": made["pin"], "url": relay::join_page(relay.pages(&made)), "expires_at": made["expires_at"], "tries": made["tries"], "room": room}))
}

async fn live_or_publish(app: &Shared, relay: &Relay, name: &str) -> Result<Arc<Live>, ApiError> {
    match app.live(name) {
        Some(live) => Ok(live),
        None => publish(app, relay, name).await,
    }
}

/// Puts the jelly on the relay. Local changes wait for this to finish (they
/// need the record's lock), then go to the relay, so none falls in between.
async fn publish(app: &Shared, relay: &Relay, name: &str) -> Result<Arc<Live>, ApiError> {
    if name == PAGE {
        return Err(Error::Invalid("the jelly page cannot be shared; share one of its jellies".into()).into());
    }
    app.store.lock().expect("store lock").get(name)?;
    let record = Record { jelly: name.into(), share: relay::token(22)?, seq: 0, links: Default::default(), expires_at: None };
    let live = track(app, record);
    let record = live.record.lock().await;
    let (doc, assets) = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(name)?;
        let doc = json!({"name": j.name, "about": j.about, "view": j.view, "verbs": j.verbs, "state": j.state,
            "version": j.version, "view_rev": j.view_rev, "comments": j.comments, "asks": j.asks, "unsent": j.unsent});
        let dir = store.asset_dir(name);
        let files = store.try_assets(name).map_err(|e| ApiError::Internal(format!("could not read assets of {name}: {e}")))?;
        let assets: Vec<(String, Vec<u8>)> = files.into_iter()
            .map(|f| std::fs::read(dir.join(&f)).map(|b| (f, b)))
            .collect::<std::io::Result<_>>().map_err(|e| ApiError::Internal(format!("could not read assets of {name}: {e}")))?;
        (doc, assets)
    };
    let (r, share) = (relay.clone(), record.share.clone());
    let published = blocking(move || {
        r.publish(&share, &doc, &RUNTIME_JS)?;
        assets.iter().try_for_each(|(f, b)| r.put_asset(&share, f, b, content_type(f)))
    }).await;
    if let Err(e) = published {
        stop(app, &record);
        return Err(api(e));
    }
    home(app).save(&record)?;
    drop(record);
    run(app, &live);
    Ok(live)
}

/// Puts a file added beside a shared jelly on the relay too.
pub(crate) async fn put_asset(app: &Shared, live: &Live, file: &str, bytes: Vec<u8>) -> Result<(), ApiError> {
    let relay = home(app).relay()?;
    let (share, file) = (live.record.lock().await.share.clone(), file.to_string());
    blocking(move || relay.put_asset(&share, &file, &bytes, content_type(&file))).await.map_err(api)
}

pub(crate) async fn remove_asset(app: &Shared, live: &Live, file: &str) -> Result<(), ApiError> {
    let relay = home(app).relay()?;
    let (share, file) = (live.record.lock().await.share.clone(), file.to_string());
    blocking(move || relay.remove_asset(&share, &file)).await.map_err(api)
}

/// What `jelly unshare` stops.
/// Every shared jelly, for the jelly page: who holds a link, whether they
/// opened it, and when the share ends. With `here`, the relay is asked about
/// each share; without it, or when the relay cannot be reached, the people
/// come from the local record and `people_from` says so.
pub(crate) async fn list(app: &Shared, here: bool) -> Result<Value, ApiError> {
    let lives: Vec<Arc<Live>> = app.shares.lock().expect("shares lock").values().cloned().collect();
    if lives.is_empty() { return Ok(json!([])); }
    let relay = home(app).relay()?;
    let mut out = Vec::new();
    for live in lives {
        let (jelly, share, links, expires_at) = {
            let r = live.record.lock().await;
            (r.jelly.clone(), r.share.clone(), r.links.clone(), r.expires_at)
        };
        let (r, s) = (relay.clone(), share.clone());
        let asked = if here { blocking(move || r.people(&s)).await.ok() } else { None };
        let mut row = match asked {
            Some(mut v) => { v["people_from"] = json!("relay"); v }
            None => json!({"people_from": "record", "expires_at": expires_at, "people": links.keys().map(|who| json!({"who": who, "can_act": !who.is_empty()})).collect::<Vec<_>>()}),
        };
        row["jelly"] = json!(jelly);
        // The owner's own way in, when a link was minted here: links made with a PIN have none.
        // When the relay did not answer, these are on its own URL, which sends them on to where its pages are.
        let urls: std::collections::BTreeMap<_, _> = links.iter().map(|(who, secret)| (who.clone(), relay::link(relay.pages(&row), &share, secret))).collect();
        row["links"] = json!(urls);
        out.push(row);
    }
    out.sort_by(|a, b| a["jelly"].as_str().cmp(&b["jelly"].as_str()));
    Ok(json!(out))
}

pub(crate) enum Stop<'a> {
    /// The whole share; its last changes stay here.
    Share,
    /// Every link of this person's ("" is the view-only link), whether minted
    /// here or made by joining with the PIN, which the relay alone knows about.
    Link(&'a str),
    /// The PIN of the jelly's room, for every jelly in it: nobody new can
    /// join, and everyone in keeps their links.
    Pin,
}

pub(crate) async fn unshare(app: &Shared, name: &str, what: Stop<'_>) -> Result<Value, ApiError> {
    let _one = app.sharing.lock().await;
    let live = app.live(name).ok_or_else(|| Error::Invalid(format!("{name} is not shared")))?;
    let relay = home(app).relay()?;
    let mut record = live.record.lock().await;
    let share = record.share.clone();
    match what {
        Stop::Link(who) => {
            let w = who.to_string();
            blocking(move || relay.revoke(&share, &w)).await.map_err(api)?;
            if record.links.remove(who).is_some() {
                home(app).save(&record)?;
            }
            return Ok(json!({"jelly": name, "revoked": who}));
        }
        Stop::Pin => {
            blocking(move || relay.unpin(&share)).await.map_err(api)?;
            return Ok(json!({"jelly": name, "pin": "ended"}));
        }
        Stop::Share => {}
    }
    let since = record.seq;
    let last = match blocking(move || relay.end(&share, since)).await {
        Ok(v) => Some(v),
        Err(Fail::Gone) => None,
        Err(e) => return Err(api(e)),
    };
    let mut kept = 0;
    if let Some(batch) = last.map(serde_json::from_value::<Batch>).transpose().map_err(|e| ApiError::Internal(e.to_string()))? {
        kept = apply(app, &live, &mut record, &batch, true)?.map_or(0, |e| e.len());
    }
    stop(app, &record);
    Ok(json!({"jelly": name, "ended": true, "caught_up": kept}))
}

/// Keeps the daemon's socket to the relay open, catching up after every drop.
async fn sync(app: Shared, live: Arc<Live>) {
    let mut wait = 1;
    let mut said = String::new();
    loop {
        match connect(&app, &live).await {
            Ok(()) => { wait = 1; said.clear(); }
            Err(Fail::Gone) => {
                let record = live.record.lock().await;
                log(format_args!("the share of {} has ended (expired, or unshared elsewhere); it is a local jelly again", record.jelly));
                return stop(&app, &record);
            }
            // Say it once, not every retry.
            Err(e) => if e.to_string() != said {
                said = e.to_string();
                log(format_args!("{} is shared, but {said}; retrying", live.record.lock().await.jelly));
            },
        }
        tokio::time::sleep(Duration::from_secs(wait)).await;
        wait = (wait * 2).min(30);
    }
}

/// One connection: catch up, then mirror every change until it drops.
async fn connect(app: &Shared, live: &Live) -> Result<(), Fail> {
    let relay = home(app).relay().map_err(Fail::Other)?;
    let (share, since) = { let r = live.record.lock().await; (r.share.clone(), r.seq) };
    let mut req = relay.socket(&share, since).into_client_request().map_err(|e| Fail::Other(e.to_string()))?;
    req.headers_mut().insert("Authorization", format!("Bearer {}", relay.secret).parse().map_err(|_| Fail::Other("bad relay secret".into()))?);
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.map_err(|e| match e {
        tungstenite::Error::Http(r) if r.status() == 404 => Fail::Gone,
        tungstenite::Error::Http(r) => Fail::Other(format!("the relay answered {}", r.status())),
        e => Fail::Other(format!("cannot reach the relay at {} ({e})", relay.url)),
    })?;
    // The relay answers "ping" without waking the jelly. No answer within 75 s
    // means the connection is dead, as after the laptop slept.
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    // The first message is the catch-up, with the whole jelly.
    let mut whole = true;
    let mut told = 0;
    loop {
        tokio::select! {
            _ = ping.tick() => ws.send(Message::text("ping")).await.map_err(|e| Fail::Other(e.to_string()))?,
            msg = tokio::time::timeout(Duration::from_secs(75), ws.next()) => match msg {
                Err(_) => return Err(Fail::Other("the relay stopped answering".into())),
                Ok(None) => return Ok(()),
                Ok(Some(Err(e))) => return Err(Fail::Other(e.to_string())),
                Ok(Some(Ok(Message::Text(t)))) if t.as_str() == "pong" => {}
                Ok(Some(Ok(Message::Text(t)))) => {
                    let batch: Batch = serde_json::from_str(t.as_str()).map_err(|e| Fail::Other(format!("unreadable message from the relay: {e}")))?;
                    let mut record = live.record.lock().await;
                    let applied = apply(app, live, &mut record, &batch, std::mem::take(&mut whole));
                    drop(record);
                    // Only what is on disk: a daemon that restarts reads its position from there.
                    let saved = live.saved.load(Ordering::SeqCst);
                    if saved > told {
                        ws.send(Message::text(json!({"saved": saved}).to_string())).await.map_err(|e| Fail::Other(e.to_string()))?;
                        told = saved;
                    }
                    match applied {
                        Ok(Some(_)) => {}
                        Ok(None) => return Err(Fail::Other("missed some changes; catching up".into())),
                        Err(e) => return Err(Fail::Other(format!("could not mirror a change: {e}"))),
                    }
                }
                Ok(Some(Ok(Message::Close(Some(f))))) if u16::from(f.code) == 4404 => return Err(Fail::Gone),
                Ok(Some(Ok(Message::Close(_)))) => return Ok(()),
                Ok(Some(Ok(_))) => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::{Message as WsMessage, WebSocketUpgrade};
    use axum::routing::{get, post};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::mpsc;

    const SHARE: &str = "aaaaaaaaaaaaaaaaaaaaaa";
    const DAY: u64 = 86_400_000;
    use crate::server::now_ms;

    /// What `jelly hear` prints for `t:j`, as the inbox gives it.
    fn heard(app: &Shared) -> Vec<String> {
        let req = crate::store::InboxRequest { project: "t".into(), reader: crate::store::InboxReader::Named { name: "test".into() }, mine: None };
        app.store.lock().unwrap().read_inbox(&req).unwrap().events.into_iter().map(|e| e.text).collect()
    }

    fn event(seq: u64, by: &str) -> Value {
        json!({"seq": seq, "at_ms": 1, "kind": "do", "by": by, "verb": "pos", "args": {}, "version": seq + 1, "view_rev": 1})
    }

    /// A relay where each change from the daemon lands just after someone
    /// else's, as in a room on a call. It answers once the daemon has mirrored
    /// that other change, or after 5 s if the daemon never does.
    #[tokio::test(flavor = "multi_thread")]
    async fn people_keep_arriving_while_a_local_change_is_on_the_relay() {
        let dir = tempfile::tempdir().unwrap();
        let (events, _) = broadcast::channel(32);
        let app: Shared = Arc::new(App { store: Mutex::new(crate::store::Store::open(dir.path()).unwrap()), events, screens: Default::default(), trials: Default::default(), probes: Default::default(), shares: Default::default(), sharing: Default::default(), open: Default::default(), feeds: Default::default(), update: Default::default() });
        app.store.lock().unwrap().make("race", "".into(), "const verbs = { pos() {} };".into(), json!({}), "agent", false, "", "t").unwrap();

        let socket: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<String>>>> = Default::default();
        let seen = Arc::new(AtomicBool::new(false));
        let (to_socket, from_op, store, seen_by_relay) = (socket.clone(), socket.clone(), app.clone(), seen.clone());
        let fake = axum::Router::new()
            .route(&format!("/api/live/{SHARE}/ws"), get(move |up: WebSocketUpgrade| async move {
                up.on_upgrade(move |mut ws| async move {
                    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                    *to_socket.lock().await = Some(tx);
                    while let Some(m) = rx.recv().await {
                        if ws.send(WsMessage::Text(m.into())).await.is_err() { break; }
                    }
                })
            }))
            // Reads the op as the real relay does. Left unread, axum closes the
            // connection after answering, and on macOS ureq then fails to read
            // the answer with "Invalid argument (os error 22)".
            .route(&format!("/api/live/{SHARE}/op"), post(move |_op: Json<Value>| async move {
                let tx = from_op.lock().await.clone().unwrap();
                let both = json!({"other": 1, "mine": 1});
                tx.send(json!({"type": "events", "events": [event(1, "person:ana@example.com")], "doc": {"state": {"other": 1}}}).to_string()).unwrap();
                let mirrored = tokio::time::timeout(Duration::from_secs(5), async {
                    while store.store.lock().unwrap().get("t:race").unwrap().state != json!({"other": 1}) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await;
                seen_by_relay.store(mirrored.is_ok(), Ordering::SeqCst);
                tx.send(json!({"type": "events", "events": [event(2, "agent")], "doc": {"state": both}}).to_string()).unwrap();
                Json(json!({"events": [event(2, "agent")], "doc": {"state": both}}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
        relay::Home(dir.path().into()).set_relay(&url).unwrap();

        let live = track(&app, Record { jelly: "t:race".into(), share: SHARE.into(), seq: 0, links: Default::default(), expires_at: None });
        run(&app, &live);
        while socket.lock().await.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let ev = forward(&app, &live, &Change { by: "agent".into(), op: crate::op::Op::Do { verb: "pos".into(), args: Value::Null, source: None, quiet: false } }).await.map_err(|e| format!("{e:?}")).unwrap();
        assert!(seen.load(Ordering::SeqCst), "the other person's change waited for the local one");
        assert_eq!((ev.0.version, ev.0.by.as_str()), (3, "agent"));
        assert_eq!(app.store.lock().unwrap().get("t:race").unwrap().state, json!({"other": 1, "mine": 1}));
    }

    /// A daemon with an empty store, and the jelly `t:j` in it.
    fn app(dir: &std::path::Path) -> Shared {
        let (events, _) = broadcast::channel(32);
        let app: Shared = Arc::new(App { store: Mutex::new(crate::store::Store::open(dir).unwrap()), events, screens: Default::default(), trials: Default::default(), probes: Default::default(), shares: Default::default(), sharing: Default::default(), open: Default::default(), feeds: Default::default(), update: Default::default() });
        app.store.lock().unwrap().make("j", "".into(), "const verbs = { pos() {} };".into(), json!({}), "agent", false, "", "t").unwrap();
        app
    }

    /// What a fake relay holds of the share `SHARE`, as `relay/src/index.ts` keeps it.
    #[derive(Default)]
    struct Held {
        events: Vec<Value>,
        /// The whole jelly, as after the last event.
        doc: Value,
        /// The `since` of every socket the daemon opened.
        since: Vec<u64>,
        /// Every position a daemon said it saved.
        saved: Vec<u64>,
        sockets: Vec<mpsc::UnboundedSender<String>>,
    }
    type Fake = Arc<Mutex<Held>>;

    impl Held {
        fn new(state: Value) -> Fake {
            let doc = json!({"name": "t:j", "about": "", "view": "", "verbs": "const verbs = { pos() {} };", "state": state, "version": 1, "view_rev": 1, "comments": [], "asks": [], "unsent": 0});
            Arc::new(Mutex::new(Held { doc, ..Default::default() }))
        }

        /// The rule of `relay/src/trim.ts`: of the events up to `seq`, those
        /// older than a week go, but never the newest. It runs on every
        /// report; the relay runs it at most once an hour.
        fn trim(&mut self, seq: u64) {
            self.saved.push(seq);
            let newest = self.events.last().map_or(0, |e| e["seq"].as_u64().unwrap());
            let week = now_ms() - 7 * DAY;
            self.events.retain(|e| { let s = e["seq"].as_u64().unwrap(); s > seq || s == newest || e["at_ms"].as_u64().unwrap() >= week });
        }

        /// A person comments on the relay; the version stays.
        fn comment(&mut self, by: &str, text: &str) {
            let seq = self.events.last().map_or(0, |e| e["seq"].as_u64().unwrap()) + 1;
            let c = json!({"id": format!("c{seq}"), "anchor": null, "text": text, "by": by, "at_ms": now_ms(), "resolved": false, "replies": []});
            self.doc["comments"].as_array_mut().unwrap().push(c);
            let ev = json!({"seq": seq, "at_ms": now_ms(), "kind": "comment", "by": by, "args": {"text": text}, "version": self.doc["version"], "view_rev": 1, "unsent": 0});
            self.events.push(ev.clone());
            let msg = json!({"type": "events", "events": [ev], "doc": {"state": self.doc["state"], "comments": self.doc["comments"], "asks": [], "unsent": 0}}).to_string();
            self.sockets.retain(|s| s.send(msg.clone()).is_ok());
        }

        /// Every event so far was made `days` ago.
        fn age(&mut self, days: u64) {
            for e in &mut self.events {
                e["at_ms"] = json!(e["at_ms"].as_u64().unwrap() - days * DAY);
            }
        }

        /// A change on the relay that leaves the state and the version alone:
        /// `edit` makes it in the jelly.
        fn edit(&mut self, kind: &str, by: &str, edit: impl FnOnce(&mut Value)) {
            let seq = self.events.last().map_or(0, |e| e["seq"].as_u64().unwrap()) + 1;
            edit(&mut self.doc);
            let ev = json!({"seq": seq, "at_ms": now_ms(), "kind": kind, "by": by, "version": self.doc["version"], "view_rev": 1, "unsent": self.doc["unsent"]});
            self.events.push(ev.clone());
            let msg = json!({"type": "events", "events": [ev], "doc": {"state": self.doc["state"], "comments": self.doc["comments"], "asks": self.doc["asks"], "unsent": self.doc["unsent"]}}).to_string();
            self.sockets.retain(|s| s.send(msg.clone()).is_ok());
        }

        fn seqs(&self) -> Vec<u64> {
            self.events.iter().map(|e| e["seq"].as_u64().unwrap()).collect()
        }

        fn after(&self, since: u64) -> Value {
            json!({"type": "events", "events": self.events.iter().filter(|e| e["seq"].as_u64().unwrap() > since).collect::<Vec<_>>(), "doc": self.doc})
        }

        /// Someone changes the jelly on the relay: the event is kept and sent
        /// to the daemon's open socket with the state after it.
        fn change(&mut self, kind: &str, by: &str, state: Value) {
            let seq = self.events.last().map_or(0, |e| e["seq"].as_u64().unwrap()) + 1;
            self.doc["version"] = json!(self.doc["version"].as_u64().unwrap() + 1);
            self.doc["state"] = state.clone();
            let ev = json!({"seq": seq, "at_ms": now_ms(), "kind": kind, "by": by, "version": self.doc["version"], "view_rev": 1, "unsent": 0});
            self.events.push(ev.clone());
            let msg = json!({"type": "events", "events": [ev], "doc": {"state": state, "comments": [], "asks": [], "unsent": 0}}).to_string();
            self.sockets.retain(|s| s.send(msg.clone()).is_ok());
        }
    }

    type Q = axum::extract::Query<std::collections::HashMap<String, String>>;
    fn since(q: &Q) -> u64 {
        q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    /// Serves `held` as the relay of `dir`'s home; returns its URL.
    async fn serve(dir: &std::path::Path, held: &Fake) -> String {
        let (held, ended) = (held.clone(), held.clone());
        let fake = axum::Router::new().route(&format!("/api/live/{SHARE}"), axum::routing::delete(move |q: Q| async move {
            let mut h = ended.lock().unwrap();
            h.sockets.clear();
            Json(h.after(since(&q)))
        })).route(&format!("/api/live/{SHARE}/ws"), get(move |q: Q, up: WebSocketUpgrade| async move {
            let since = since(&q);
            up.on_upgrade(move |mut ws| async move {
                let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                let first = {
                    let mut h = held.lock().unwrap();
                    h.since.push(since);
                    h.sockets.push(tx);
                    h.after(since).to_string()
                };
                if ws.send(WsMessage::Text(first.into())).await.is_err() { return; }
                loop {
                    tokio::select! {
                        m = rx.recv() => match m {
                            Some(m) => if ws.send(WsMessage::Text(m.into())).await.is_err() { return },
                            None => return,
                        },
                        m = ws.recv() => match m {
                            Some(Ok(WsMessage::Text(t))) => if let Some(seq) = serde_json::from_str::<Value>(t.as_str()).ok().and_then(|v| v["saved"].as_u64()) {
                                held.lock().unwrap().trim(seq);
                            },
                            Some(Ok(_)) => {}
                            _ => return,
                        },
                    }
                }
            })
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, fake).await.unwrap() });
        relay::Home(dir.into()).set_relay(&url).unwrap();
        url
    }

    /// Waits up to 5 s for `t:j` to hold `state`.
    async fn arrives(app: &Shared, state: Value) {
        let landed = tokio::time::timeout(Duration::from_secs(5), async {
            while app.store.lock().unwrap().get("t:j").unwrap().state != state {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        assert!(landed.is_ok(), "{state} never arrived; the jelly holds {}", app.store.lock().unwrap().get("t:j").unwrap().state);
    }

    /// A newer relay may make a kind of change this daemon does not know. The
    /// mirror takes the jelly as the relay sent it and goes on, on the same socket.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_mirror_goes_on_past_a_kind_of_change_it_does_not_know() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let held = Held::new(json!({}));
        held.lock().unwrap().change("wave", "person:ana", json!({"waved": true}));
        serve(dir.path(), &held).await;
        let live = track(&app, Record { jelly: "t:j".into(), share: SHARE.into(), seq: 0, links: Default::default(), expires_at: None });
        run(&app, &live);
        arrives(&app, json!({"waved": true})).await;
        held.lock().unwrap().change("do", "person:ana", json!({"waved": true, "after": 1}));
        arrives(&app, json!({"waved": true, "after": 1})).await;
        assert_eq!(held.lock().unwrap().since, [0], "the daemon kept its first socket");
        assert_eq!(live.record.lock().await.seq, 2);
    }

    /// Waits up to 5 s for `what` to hold.
    async fn until(what: &str, mut ok: impl FnMut() -> bool) {
        let done = tokio::time::timeout(Duration::from_secs(5), async { while !ok() { tokio::time::sleep(Duration::from_millis(10)).await } }).await;
        assert!(done.is_ok(), "never: {what}");
    }

    /// Shares `t:j` from `app` as `SHARE` from relay seq `seq`, as `publish` leaves it.
    fn shared(app: &Shared, seq: u64) -> Arc<Live> {
        let record = Record { jelly: "t:j".into(), share: SHARE.into(), seq, links: Default::default(), expires_at: None };
        home(app).save(&record).unwrap();
        let live = track(app, record);
        run(app, &live);
        live
    }

    /// The daemon stops, as when its process ends: the record stays on disk.
    fn halt(live: &Live) {
        live.task.lock().unwrap().take().unwrap().abort();
    }

    /// A copy of a home, as a backup is.
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&target).unwrap();
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// A daemon on a fresh copy of `dir`, started as `serve` starts one.
    fn restart(dir: &std::path::Path) -> Shared {
        let (events, _) = broadcast::channel(32);
        let app: Shared = Arc::new(App { store: Mutex::new(crate::store::Store::open(dir).unwrap()), events, screens: Default::default(), trials: Default::default(), probes: Default::default(), shares: Default::default(), sharing: Default::default(), open: Default::default(), feeds: Default::default(), update: Default::default() });
        resume(&app);
        app
    }

    /// A home restored from a backup less than a week old finds every event
    /// it missed still on the relay, and hears each one.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_home_restored_from_a_recent_backup_hears_every_change() {
        let (dir, backup) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = app(dir.path());
        let held = Held::new(json!({}));
        serve(dir.path(), &held).await;
        let live = shared(&app, 0);
        held.lock().unwrap().change("do", "person:ana", json!({"n": 1}));
        until("the daemon says it saved 1", || held.lock().unwrap().saved.contains(&1)).await;
        copy(dir.path(), backup.path());
        held.lock().unwrap().change("do", "person:ana", json!({"n": 2}));
        held.lock().unwrap().change("send", "person:ana", json!({"n": 2}));
        held.lock().unwrap().change("do", "person:ana", json!({"n": 3}));
        until("the daemon says it saved 4", || held.lock().unwrap().saved.ends_with(&[4])).await;
        assert_eq!(held.lock().unwrap().seqs(), [1, 2, 3, 4], "nothing is a week old");
        halt(&live);

        let restored = restart(backup.path());
        arrives(&restored, json!({"n": 3})).await;
        let heard = heard(&restored);
        assert_eq!(heard.len(), 4, "{heard:?}");
        assert!(heard[2].starts_with("ana sent"), "the Send is heard: {heard:?}");
    }

    /// One more than a week behind finds the events it missed gone. It
    /// catches up from the whole jelly on its first socket, keeps that socket,
    /// and the agent hears what happened.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_home_restored_from_an_old_backup_catches_up_and_says_so() {
        let (dir, backup) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = app(dir.path());
        let held = Held::new(json!({}));
        serve(dir.path(), &held).await;
        let live = shared(&app, 0);
        held.lock().unwrap().change("do", "person:ana", json!({"n": 1}));
        until("the daemon says it saved 1", || held.lock().unwrap().saved.contains(&1)).await;
        copy(dir.path(), backup.path());
        held.lock().unwrap().change("do", "person:ana", json!({"n": 2}));
        held.lock().unwrap().change("send", "person:ana", json!({"n": 2}));
        held.lock().unwrap().change("do", "person:ana", json!({"n": 3}));
        arrives(&app, json!({"n": 3})).await;
        held.lock().unwrap().age(8);
        held.lock().unwrap().change("do", "person:ana", json!({"n": 4}));
        until("the relay deletes what is saved and a week old", || held.lock().unwrap().seqs() == [5]).await;
        halt(&live);

        let restored = restart(backup.path());
        arrives(&restored, json!({"n": 4})).await;
        until("the restored home says it saved 5", || held.lock().unwrap().saved.ends_with(&[5])).await;
        held.lock().unwrap().change("do", "person:bo", json!({"n": 5}));
        arrives(&restored, json!({"n": 5})).await;
        assert_eq!(held.lock().unwrap().since, [0, 1], "one socket each, no loop");
        assert_eq!(home(&restored).records().unwrap()[0].seq, 6);
        let heard = heard(&restored);
        assert!(heard.iter().any(|l| l == "caught up from the relay: 3 changes on the shared jelly were not heard one by one (the relay keeps a week of them); `jelly state t:j` has the jelly as it is now, and `jelly look t:j` what people did"), "{heard:?}");
        assert!(heard.last().unwrap().starts_with("bo "), "{heard:?}");
        let store = restored.store.lock().unwrap();
        assert_eq!(store.get("t:j").unwrap().version, 7);
        assert!(store.at_version("t:j", 3).is_err(), "v3 was never in this home");
    }

    /// A record that fails to save is not told to the relay, which so keeps
    /// the events after the last saved one. A restart reads the record from
    /// disk and replays them; nothing is missing.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_save_then_a_restart_replays_from_what_is_on_disk() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let held = Held::new(json!({}));
        serve(dir.path(), &held).await;
        let live = shared(&app, 0);
        held.lock().unwrap().change("do", "person:ana", json!({"n": 1}));
        until("the daemon says it saved 1", || held.lock().unwrap().saved == [1]).await;
        let shares = dir.path().join("shares");
        std::fs::set_permissions(&shares, std::fs::Permissions::from_mode(0o555)).unwrap();
        held.lock().unwrap().change("do", "person:ana", json!({"n": 2}));
        held.lock().unwrap().change("do", "person:ana", json!({"n": 3}));
        arrives(&app, json!({"n": 3})).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(held.lock().unwrap().saved, [1], "the daemon tells only what is on disk");
        assert_eq!(held.lock().unwrap().seqs(), [1, 2, 3]);
        halt(&live);

        std::fs::set_permissions(&shares, std::fs::Permissions::from_mode(0o755)).unwrap();
        let again = restart(dir.path());
        until("the restarted daemon says it saved 3", || held.lock().unwrap().saved.ends_with(&[3])).await;
        assert_eq!(held.lock().unwrap().since, [0, 1]);
        assert_eq!(again.store.lock().unwrap().get("t:j").unwrap().state, json!({"n": 3}));
        assert_eq!(held.lock().unwrap().seqs(), [1, 2, 3], "saved, but not a week old");
    }

    /// Two homes with one share record (a home copied to a second machine)
    /// both mirror the share and both tell the relay what they saved. The one
    /// that was away catches up from the whole jelly when it comes back.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_daemons_with_one_share_record_both_keep_up() {
        let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = app(a_dir.path());
        let held = Held::new(json!({}));
        let url = serve(a_dir.path(), &held).await;
        home(&a).save(&Record { jelly: "t:j".into(), share: SHARE.into(), seq: 0, links: Default::default(), expires_at: None }).unwrap();
        copy(a_dir.path(), b_dir.path());
        relay::Home(b_dir.path().into()).set_relay(&url).unwrap();
        let a_live = shared(&a, 0);
        let b = restart(b_dir.path());
        for n in 1..=2 {
            held.lock().unwrap().change("do", "person:ana", json!({"n": n}));
        }
        arrives(&a, json!({"n": 2})).await;
        arrives(&b, json!({"n": 2})).await;
        until("both say they saved 2", || held.lock().unwrap().saved.iter().filter(|s| **s == 2).count() == 2).await;
        halt(&b.live("t:j").unwrap());

        for n in 3..=4 {
            held.lock().unwrap().change("do", "person:ana", json!({"n": n}));
        }
        arrives(&a, json!({"n": 4})).await;
        // B stays away more than a week.
        held.lock().unwrap().age(8);
        held.lock().unwrap().change("do", "person:ana", json!({"n": 5}));
        until("the relay deletes what A saved a week ago", || held.lock().unwrap().seqs() == [5]).await;
        let b = restart(b_dir.path());
        arrives(&b, json!({"n": 5})).await;
        held.lock().unwrap().change("do", "person:bo", json!({"n": 6}));
        arrives(&a, json!({"n": 6})).await;
        arrives(&b, json!({"n": 6})).await;
        let mut since = held.lock().unwrap().since.clone();
        since.sort();
        assert_eq!(since, [0, 0, 2], "each daemon opened one socket per start");
        halt(&a_live);
    }

    /// Unsharing from a home far behind the relay still keeps the last of the
    /// jelly: the relay's answer carries all of it.
    #[tokio::test(flavor = "multi_thread")]
    async fn unsharing_from_a_home_behind_the_relay_keeps_the_jelly() {
        let (dir, backup) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = app(dir.path());
        let held = Held::new(json!({}));
        serve(dir.path(), &held).await;
        home(&app).save(&Record { jelly: "t:j".into(), share: SHARE.into(), seq: 0, links: Default::default(), expires_at: None }).unwrap();
        copy(dir.path(), backup.path());
        let live = shared(&app, 0);
        for n in 1..=2 {
            held.lock().unwrap().change("do", "person:ana", json!({"n": n}));
        }
        arrives(&app, json!({"n": 2})).await;
        held.lock().unwrap().age(8);
        held.lock().unwrap().change("do", "person:ana", json!({"n": 3}));
        until("the relay deletes what the daemon saved a week ago", || held.lock().unwrap().seqs() == [3]).await;
        halt(&live);

        let (events, _) = broadcast::channel(32);
        let restored: Shared = Arc::new(App { store: Mutex::new(crate::store::Store::open(backup.path()).unwrap()), events, screens: Default::default(), trials: Default::default(), probes: Default::default(), shares: Default::default(), sharing: Default::default(), open: Default::default(), feeds: Default::default(), update: Default::default() });
        // Shared, but its socket never got to catch up.
        track(&restored, home(&restored).records().unwrap().remove(0));
        let ended = unshare(&restored, "t:j", Stop::Share).await.map_err(|e| format!("{e:?}")).unwrap();
        assert_eq!(ended["caught_up"], 2, "the last event, and the note that two were missed");
        assert_eq!(restored.store.lock().unwrap().get("t:j").unwrap().state, json!({"n": 3}));
        assert!(restored.live("t:j").is_none() && home(&restored).records().unwrap().is_empty());
        // No undo step jumps the gap: undoing now would take back changes this home never saw.
        let mut store = restored.store.lock().unwrap();
        assert_eq!(store.undo("t:j", "agent").unwrap_err().to_string(), "t:j has nothing to undo");
        assert_eq!(store.get("t:j").unwrap().state, json!({"n": 3}));
        assert!(store.at_version("t:j", 2).is_err(), "look --compare cannot rebuild a version this home never had");
        assert_eq!(store.at_version("t:j", 4).unwrap().state, json!({"n": 3}));
    }

    /// A home whose jelly file is older than its share record (the jelly came
    /// back from a backup, the record did not) takes the jelly the catch-up
    /// carries, though no event is new, and says so.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_jelly_older_than_its_share_record_catches_up_with_no_new_events() {
        let (dir, old) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = app(dir.path());
        let held = Held::new(json!({}));
        serve(dir.path(), &held).await;
        let live = shared(&app, 0);
        copy(&dir.path().join("jellies"), old.path());
        held.lock().unwrap().change("do", "person:ana", json!({"n": 1}));
        held.lock().unwrap().comment("person:ana", "Nice");
        until("the daemon says it saved 2", || held.lock().unwrap().saved.ends_with(&[2])).await;
        halt(&live);
        copy(old.path(), &dir.path().join("jellies"));

        let again = restart(dir.path());
        arrives(&again, json!({"n": 1})).await;
        assert_eq!(again.store.lock().unwrap().get("t:j").unwrap().comments.len(), 1);
        assert_eq!(heard(&again).last().unwrap(), "caught up from the relay: this home was behind its share (restored from a backup?), so it took the jelly as the relay has it; `jelly state t:j` shows it, and `jelly look t:j` what people did");
    }

    /// The same when only an answer and a reply are missing, which change
    /// neither the version nor how many asks and comments there are. A daemon
    /// that starts with nothing missing records nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_jelly_missing_only_an_answer_and_a_reply_catches_up() {
        let (dir, old) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = app(dir.path());
        let held = Held::new(json!({}));
        serve(dir.path(), &held).await;
        let live = shared(&app, 0);
        held.lock().unwrap().edit("ask", "agent", |d| d["asks"] = json!([{"id": "a1", "title": "Which?", "by": "agent", "at_ms": 1}]));
        held.lock().unwrap().edit("comment", "person:ana", |d| d["comments"] = json!([{"id": "c1", "anchor": null, "text": "Nice", "by": "person:ana", "at_ms": 1, "resolved": false, "replies": []}]));
        until("the daemon says it saved 2", || held.lock().unwrap().saved.ends_with(&[2])).await;
        copy(&dir.path().join("jellies"), old.path());
        held.lock().unwrap().edit("answer", "person:ana", |d| d["asks"][0]["answer"] = json!({"option": null, "text": "The first", "by": "person:ana", "at_ms": 2}));
        held.lock().unwrap().edit("reply", "agent", |d| d["comments"][0]["replies"] = json!([{"text": "Thanks", "by": "agent", "at_ms": 3}]));
        until("the daemon says it saved 4", || held.lock().unwrap().saved.ends_with(&[4])).await;
        halt(&live);
        copy(old.path(), &dir.path().join("jellies"));

        let again = restart(dir.path());
        until("the answer and the reply land", || {
            let store = again.store.lock().unwrap();
            let j = store.get("t:j").unwrap();
            j.asks[0].answer.as_ref().is_some_and(|a| a.text == "The first") && j.comments[0].replies.len() == 1
        }).await;
        halt(&again.live("t:j").unwrap());

        let third = restart(dir.path());
        // Its catch-up carries no event; the change after it shows the catch-up was read.
        until("the third daemon connects", || held.lock().unwrap().since == [0, 4, 4]).await;
        held.lock().unwrap().edit("do", "person:ana", |d| { d["state"] = json!({"n": 1}); d["version"] = json!(2); });
        arrives(&third, json!({"n": 1})).await;
        let log = std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
        assert_eq!(log.matches(r#""kind":"missed""#).count(), 1, "once for the restored jelly, and not again by a daemon that missed nothing");
    }
}
