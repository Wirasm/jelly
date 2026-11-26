//! Live share links (#108). While a jelly is shared, the copy on your relay is
//! the live one: its verbs run there, people act through their links, and the
//! daemon forwards local changes to it. Every change the relay makes comes back
//! over one WebSocket per shared jelly and is mirrored into the local store, so
//! `inbox`, `wait` and `look` work as before. When the laptop sleeps the relay
//! keeps going; on reconnecting the daemon catches up from the last event it saw.

use super::{content_type, ApiError, App, Shared, RUNTIME_JS};
use crate::relay::{self, Fail, Record, Relay};
use crate::store::{Error, Event, Mirrored, MirroredDoc, PAGE};
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;
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
    let live = Arc::new(Live { record: tokio::sync::Mutex::new(record), task: Mutex::new(None), mirrored: broadcast::channel(1024).0 });
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
/// returns them with the relay's seq. None when events before them are missing.
fn apply(app: &App, live: &Live, record: &mut Record, batch: &Batch) -> Result<Option<Vec<(u64, Event)>>, Error> {
    let fresh: Vec<&Mirrored> = batch.events.iter().filter(|e| e.seq > record.seq).collect();
    let Some(last) = fresh.last() else { return Ok(Some(Vec::new())) };
    if fresh[0].seq != record.seq + 1 {
        return Ok(None);
    }
    let mut store = app.store.lock().expect("store lock");
    let mut out = Vec::new();
    let mut failed = None;
    for e in &fresh {
        let doc = (e.seq == last.seq).then_some(&batch.doc);
        match store.mirror(&record.jelly, e, doc) {
            Ok(ev) => { record.seq = e.seq; out.push((e.seq, ev)); }
            Err(err) => { failed = Some(err); break; }
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
    if let Err(e) = home(app).save(record) {
        log(format_args!("{e}"));
    }
    failed.map_or(Ok(Some(out)), Err)
}

/// Makes one change on the relay and returns its event as mirrored here. The
/// socket mirrors every change, this one too, so people's changes keep coming
/// in however many local ones are waiting on the relay.
pub(crate) async fn forward(app: &Shared, live: &Live, op: Value) -> Result<Json<Event>, ApiError> {
    let relay = home(app).relay()?;
    // Subscribed before the change is made, so its event cannot pass unseen.
    let mut mirrored = live.mirrored.subscribe();
    // Waits for a publish under way, so the change reaches the relay after it.
    let share = live.record.lock().await.share.clone();
    let r = relay.clone();
    let answer = match blocking(move || r.change(&share, &op)).await {
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
    let record = Record { jelly: name.into(), share: relay::token(22)?, seq: 0, links: Default::default() };
    let live = track(app, record);
    let record = live.record.lock().await;
    let (doc, assets) = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(name)?;
        let doc = json!({"name": j.name, "about": j.about, "view": j.view, "verbs": j.verbs, "state": j.state,
            "version": j.version, "view_rev": j.view_rev, "comments": j.comments, "asks": j.asks});
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
        let (jelly, share, links) = {
            let r = live.record.lock().await;
            (r.jelly.clone(), r.share.clone(), r.links.clone())
        };
        let (r, s) = (relay.clone(), share.clone());
        let asked = if here { blocking(move || r.people(&s)).await.ok() } else { None };
        let mut row = match asked {
            Some(mut v) => { v["people_from"] = json!("relay"); v }
            None => json!({"people_from": "record", "people": links.keys().map(|who| json!({"who": who, "can_act": !who.is_empty()})).collect::<Vec<_>>()}),
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
        kept = apply(app, &live, &mut record, &batch)?.map_or(0, |e| e.len());
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
                    match apply(app, live, &mut record, &batch) {
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

        let live = track(&app, Record { jelly: "t:race".into(), share: SHARE.into(), seq: 0, links: Default::default() });
        run(&app, &live);
        while socket.lock().await.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let ev = forward(&app, &live, json!({"op": "do", "verb": "pos", "by": "agent"})).await.map_err(|e| format!("{e:?}")).unwrap();
        assert!(seen.load(Ordering::SeqCst), "the other person's change waited for the local one");
        assert_eq!((ev.0.version, ev.0.by.as_str()), (3, "agent"));
        assert_eq!(app.store.lock().unwrap().get("t:race").unwrap().state, json!({"other": 1, "mine": 1}));
    }
}
