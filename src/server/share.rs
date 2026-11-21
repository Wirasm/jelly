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
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};

/// A shared jelly in the daemon: its record, and the task that mirrors it.
pub(crate) struct Live {
    /// Held while changes are mirrored, so they apply once and in order.
    record: tokio::sync::Mutex<Record>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
    let live = Arc::new(Live { record: tokio::sync::Mutex::new(record), task: Mutex::new(None) });
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
fn apply(app: &App, record: &mut Record, batch: &Batch) -> Result<Option<Vec<(u64, Event)>>, Error> {
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
    if let Err(e) = home(app).save(record) {
        log(format_args!("{e}"));
    }
    failed.map_or(Ok(Some(out)), Err)
}

/// Makes one change on the relay and returns its event as mirrored here.
pub(crate) async fn forward(app: &Shared, live: &Live, op: Value) -> Result<Json<Event>, ApiError> {
    let relay = home(app).relay()?;
    let mut record = live.record.lock().await;
    let share = record.share.clone();
    let r = relay.clone();
    let answer = match blocking(move || r.change(&share, &op)).await {
        Err(Fail::Gone) => {
            stop(app, &record);
            return Err(Error::Invalid(format!("the share of {} has ended (expired, or unshared elsewhere); it is a local jelly again, so run that again", record.jelly)).into());
        }
        a => a.map_err(api)?,
    };
    let batch: Batch = serde_json::from_value(answer).map_err(|e| ApiError::Internal(format!("{}: unreadable answer: {e}", relay.url)))?;
    let seq = batch.events.last().map(|e| e.seq).ok_or_else(|| ApiError::Internal("the relay made no change".into()))?;
    let mut done = apply(app, &mut record, &batch)?;
    if done.is_none() {
        // Someone acted on the relay just before this; its socket has not told us yet.
        let (share, since) = (record.share.clone(), record.seq);
        let caught: Batch = serde_json::from_value(blocking(move || relay.since(&share, since)).await.map_err(api)?)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        done = apply(app, &mut record, &caught)?;
    }
    done.into_iter().flatten().find(|(s, _)| *s == seq).map(|(_, ev)| Json(ev))
        .ok_or_else(|| ApiError::Internal(format!("the relay's change {seq} did not arrive")))
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
    let expires_at = blocking(move || r.mint(&share, &s, &w, days)).await.map_err(api)?;
    record.links.insert(who.into(), secret.clone());
    home(app).save(&record)?;
    Ok(json!({"jelly": name, "url": relay.link(&record.share, &secret), "who": who, "expires_at": expires_at}))
}

/// Makes a new PIN for a room, sharing the jelly first if it is not shared yet.
/// People type it at the relay's join page with their email and get their own
/// link, so they act as `person:<email>` exactly as with `--to`.
pub(crate) async fn pin(app: &Shared, name: &str, hours: u64, days: u64) -> Result<Value, ApiError> {
    let relay = home(app).relay()?;
    let _one = app.sharing.lock().await;
    let live = live_or_publish(app, &relay, name).await?;
    let share = live.record.lock().await.share.clone();
    let r = relay.clone();
    let made = blocking(move || r.pin(&share, hours, days)).await.map_err(api)?;
    Ok(json!({"jelly": name, "pin": made["pin"], "url": relay.join_page(), "expires_at": made["expires_at"], "tries": made["tries"]}))
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
        r.publish(&share, &doc, RUNTIME_JS)?;
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
/// opened it, and when the share ends. What the relay knows is asked for each
/// time; when it cannot be reached, the people come from the local record and
/// `people_from` says so.
pub(crate) async fn list(app: &Shared) -> Result<Value, ApiError> {
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
        let mut row = match blocking(move || r.people(&s)).await {
            Ok(mut v) => { v["people_from"] = json!("relay"); v }
            Err(_) => json!({"people_from": "record", "people": links.keys().map(|who| json!({"who": who, "can_act": !who.is_empty()})).collect::<Vec<_>>()}),
        };
        row["jelly"] = json!(jelly);
        // The owner's own way in, when a link was minted here: links made with a PIN have none.
        row["links"] = json!(links.iter().map(|(who, secret)| (who.clone(), relay.link(&share, secret))).collect::<std::collections::BTreeMap<_, _>>());
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
    /// The PIN: nobody new can join, and everyone in stays.
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
        kept = apply(app, &mut record, &batch)?.map_or(0, |e| e.len());
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
                    match apply(app, &mut record, &batch) {
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
