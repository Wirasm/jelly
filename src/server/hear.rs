//! Hearing the person: the inbox, and waiting for the next change (`jelly hear`).

use super::*;

/// `GET /api/wait`, which is what `jelly hear --wait` asks.
#[derive(Deserialize, Clone)]
pub(super) struct WaitQ {
    /// Return the first matching event after this one. Without it the wait
    /// starts at the newest event when it begins, and reads no history.
    pub(super) since: Option<u64>,
    pub(super) by: String,
    pub(super) jelly: Option<String>,
    pub(super) timeout: Option<u64>,
    /// Return on any change. Without it only a `send` ends the wait: the
    /// person works through a jelly and says when they are done.
    #[serde(default)]
    pub(super) every: bool,
}

/// The person hands the turn back (the view's Send button).
pub(super) async fn send(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "send", "by": by(r.by)?})).await;
    }
    app.change(|s| s.send(&name, &by(r.by)?))
}

pub(super) async fn wait(State(app): State<Shared>, Query(q): Query<WaitQ>) -> Result<Response, ApiError> {
    by(Some(q.by.clone()))?;
    match wait_since(&app, &q).await? {
        Some(ev) => Ok(Json(ev).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

pub(super) fn history_error(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("could not read event history: {e}"))
}

pub(super) fn recorded_change(reader: &mut impl BufRead, q: &WaitQ) -> Result<Option<Event>, Error> {
    let mut line = String::new();
    while reader.read_line(&mut line).map_err(history_error)? != 0 {
        let ev: Event = serde_json::from_str(&line).map_err(history_error)?;
        // A wait sets `since` before it reads (`wait_since_at_handoff`).
        if q.since.is_none_or(|since| ev.seq > since) && ev.by != q.by && (q.every || ev.kind == crate::store::EventKind::Send)
            && q.jelly.as_ref().map_or(ev.jelly != PAGE, |j| *j == ev.jelly)
        {
            return Ok(Some(ev));
        }
        line.clear();
    }
    Ok(None)
}

/// `events.jsonl` as an open wait reads it. A trim replaces the file, so the
/// wait first reads the rest of the old file, whose tail the trim may have
/// dropped, then reopens and reads the new one from the start. That cannot
/// report an event twice, because the wait returns on the first event that matches.
pub(super) struct History {
    pub(super) reader: Option<BufReader<File>>,
    pub(super) trims: u64,
}

impl History {
    pub(super) fn open(store: &Store) -> Result<History, Error> {
        let reader = match File::open(store.events_path()) {
            Ok(file) => Some(BufReader::new(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && store.sequence() == 0 => None,
            Err(e) => return Err(history_error(e)),
        };
        Ok(History { reader, trims: store.trims() })
    }

    /// The log from its end: what is recorded after `store.sequence()`.
    pub(super) fn at_end(store: &Store) -> Result<History, Error> {
        let mut history = History::open(store)?;
        if let Some(reader) = &mut history.reader {
            std::io::Seek::seek(reader, std::io::SeekFrom::End(0)).map_err(history_error)?;
        }
        Ok(history)
    }

    pub(super) fn next_change(&mut self, store: &Store, q: &WaitQ) -> Result<Option<Event>, Error> {
        if self.trims != store.trims() {
            if let Some(reader) = &mut self.reader {
                if let Some(ev) = recorded_change(reader, q)? { return Ok(Some(ev)); }
            }
            *self = History::open(store)?;
        } else if self.reader.is_none() {
            *self = History::open(store)?;
        }
        match &mut self.reader {
            Some(reader) => recorded_change(reader, q),
            None => Ok(None),
        }
    }
}

pub(super) async fn wait_since(app: &App, q: &WaitQ) -> Result<Option<Event>, Error> {
    wait_since_at_handoff(app, q, |_| {}).await
}

pub(super) async fn wait_since_at_handoff(app: &App, q: &WaitQ, after_scan: impl FnOnce(&mut Store)) -> Result<Option<Event>, Error> {
    let mut q = q.clone();
    let rx = app.events.subscribe();
    let deadline = q.timeout.map(|t| Instant::now().checked_add(Duration::from_secs(t))
        .ok_or_else(|| Error::Invalid("timeout is too large".into()))).transpose()?;
    let history = {
        let mut store = app.store.lock().expect("store lock");
        let history = match q.since {
            Some(_) => {
                let mut history = History::open(&store)?;
                if let Some(ev) = history.next_change(&store, &q)? { return Ok(Some(ev)); }
                history
            }
            // Now, under the lock every event is numbered and logged under:
            // the log ends at this sequence, and the wait is already subscribed.
            None => {
                q.since = Some(store.sequence());
                History::at_end(&store)?
            }
        };
        if let Some(name) = &q.jelly { store.get(name)?; }
        after_scan(&mut store);
        history
    };
    wait_for_record(app, &q, history, rx, deadline).await
}

pub(super) async fn wait_for_record(
    app: &App,
    q: &WaitQ,
    mut history: History,
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
        let store = app.store.lock().expect("store lock");
        if let Some(ev) = history.next_change(&store, q)? { return Ok(Some(ev)); }
        if matches!(notification, Err(broadcast::error::RecvError::Closed)) {
            return Err(Error::Invalid("the event stream ended".into()));
        }
    }
}

/// What the person did that this reader has not heard yet, each with a line of
/// text. It consumes nothing: the CLI commits `through` once it has printed them.
pub(super) async fn read_inbox(State(app): State<Shared>, Json(req): Json<InboxRequest>) -> Result<Json<InboxBatch>, ApiError> {
    Ok(Json(app.store.lock().expect("store lock").read_inbox(&req)?))
}

pub(super) async fn commit_inbox(State(app): State<Shared>, Json(req): Json<InboxCommit>) -> Result<Json<Value>, ApiError> {
    app.store.lock().expect("store lock").commit_inbox(req)?;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    fn fixture(capacity: usize) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        for name in ["j", "other", PAGE] {
            store.make(name, "".into(), "const verbs = {};".into(), json!({}), "agent", false, "", "").unwrap();
        }
        let (events, _) = broadcast::channel(capacity);
        (dir, App { store: Mutex::new(store), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default(), update: Mutex::default() })
    }

    fn query(jelly: Option<&str>, since: u64) -> WaitQ {
        WaitQ { since: Some(since), by: "agent".into(), jelly: jelly.map(String::from), timeout: Some(0), every: true }
    }

    fn note(app: &App, jelly: &str, who: &str) -> Event {
        app.change(|s| s.note(jelly, "changed", who)).unwrap().0
    }

    #[tokio::test]
    async fn fresh_history_waits_for_its_first_event_or_times_out() {
        for (first_event, since) in [(false, 0), (true, 0), (false, u64::MAX)] {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(dir.path()).unwrap();
            let path = store.events_path();
            let (events, _) = broadcast::channel(1);
            let app = App { store: Mutex::new(store), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default(), update: Mutex::default() };
            let q = WaitQ { timeout: Some(1), ..query(None, since) };
            let waiting = wait_since(&app, &q);
            tokio::pin!(waiting);
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            }).await;
            assert!(!path.exists(), "waiting must not create an event log");
            if first_event {
                let expected = app.change(|s| s.make("first", "".into(), "const verbs = {};".into(), json!({}), "person", false, "", "")).unwrap().0;
                let actual = waiting.await.unwrap().unwrap();
                assert_eq!(actual.seq, expected.seq);
                assert_eq!(actual.jelly, "first");
                assert!(actual.state.is_none());
            } else {
                assert!(waiting.await.unwrap().is_none());
                assert!(!path.exists());
            }
        }
    }

    #[tokio::test]
    async fn history_is_exclusive_ordered_and_filtered_by_author_and_jelly() {
        let (_dir, app) = fixture(16);
        let start = note(&app, "j", "person");
        note(&app, "j", "agent");
        let page = note(&app, PAGE, "person");
        let other = note(&app, "other", "helper");
        let first = note(&app, "j", "helper");
        note(&app, "j", "person");
        assert_eq!(wait_since(&app, &query(Some("j"), start.seq)).await.unwrap().unwrap().seq, first.seq);
        assert_eq!(wait_since(&app, &query(None, start.seq)).await.unwrap().unwrap().seq, other.seq);
        assert_eq!(wait_since(&app, &query(Some(PAGE), start.seq)).await.unwrap().unwrap().seq, page.seq);
        assert!(wait_since(&app, &query(None, u64::MAX)).await.unwrap().is_none());
        assert!(matches!(wait_since(&app, &query(Some("missing"), 0)).await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn a_wait_ends_on_send_and_counts_what_the_person_did() {
        let (_dir, app) = fixture(16);
        let start = note(&app, "j", "agent");
        let send = {
            let mut store = app.store.lock().unwrap();
            store.comment("j", json!(null), "first", "person").unwrap();
            store.comment("j", json!(null), "second", "person").unwrap();
            store.note("j", "not the person", "helper").unwrap();
            assert_eq!(store.get("j").unwrap().unsent, 2);
            let send = store.send("j", "person").unwrap();
            assert_eq!(store.get("j").unwrap().unsent, 0);
            send
        };
        let on_send = WaitQ { every: false, ..query(Some("j"), start.seq) };
        let woke = wait_since(&app, &on_send).await.unwrap().unwrap();
        assert_eq!((woke.seq, woke.args), (send.seq, Some(json!(2))));
        assert!(wait_since(&app, &WaitQ { every: false, ..query(Some("j"), send.seq) }).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_wait_without_since_hears_the_next_send_and_not_an_earlier_one() {
        let (_dir, app) = fixture(8);
        let _ = app.change(|s| s.send("j", "person")).unwrap();
        let q = WaitQ { since: None, every: false, timeout: Some(5), ..query(Some("j"), 0) };
        let waiting = wait_since(&app, &q);
        tokio::pin!(waiting);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        }).await;
        let send = app.change(|s| s.send("j", "person")).unwrap().0;
        assert_eq!(waiting.await.unwrap().unwrap().seq, send.seq);
    }

    #[tokio::test]
    async fn history_survives_replacement_removal_and_restart() {
        let (dir, mut app) = fixture(16);
        let start = app.change(|s| s.set_state("j", "/n", Some(json!(1)), "agent")).unwrap().0;
        let replacement = app.change(|s| s.make("j", "".into(), "const verbs = {};".into(), json!({}), "person", true, "", "")).unwrap().0;
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
            let ask = app.change(|s| s.ask("j", json!({"title": "Which?"}), "agent")).unwrap().0;
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
        let mut history = History::open(&app.store.lock().unwrap()).unwrap();
        assert!(history.next_change(&app.store.lock().unwrap(), &q).unwrap().is_none());
        let (first, second) = {
            let mut s = app.store.lock().unwrap();
            (s.note("j", "first", "helper").unwrap(), s.note("j", "second", "person").unwrap())
        };
        app.events.send(second).unwrap();
        let result = wait_for_record(&app, &q, history, rx, None).await.unwrap().unwrap();
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
    async fn an_open_wait_hears_events_after_the_log_is_trimmed() {
        let (_dir, app) = fixture(8);
        let start = note(&app, "j", "person");
        let q = WaitQ { timeout: Some(5), ..query(Some("j"), start.seq) };
        let future = wait_since(&app, &q);
        tokio::pin!(future);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut future).await.is_err());
        note(&app, "j", "agent");
        app.store.lock().unwrap().trim_events(1, 1 << 20).unwrap();
        let expected = note(&app, "j", "helper");
        assert_eq!(tokio::time::timeout(Duration::from_secs(1), future).await.unwrap().unwrap().unwrap().seq, expected.seq);
    }

    #[tokio::test]
    async fn an_open_wait_hears_an_unread_event_the_trim_dropped() {
        let (_dir, app) = fixture(8);
        let start = note(&app, "j", "person");
        let q = WaitQ { timeout: Some(5), ..query(Some("j"), start.seq) };
        let future = wait_since(&app, &q);
        tokio::pin!(future);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut future).await.is_err());
        let expected = note(&app, "j", "helper");
        note(&app, "j", "agent");
        app.store.lock().unwrap().trim_events(1, 0).unwrap();
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
        assert_eq!(last.kind, crate::store::EventKind::Ask);
        assert_eq!(summary["state"]["round"], 1);
    }

    #[test]
    fn log_failures_leave_answers_and_removals_uncommitted() {
        for failure in ["directory", "readonly", "partial"] {
            let (dir, app) = fixture(8);
            let _ = app.change(|s| s.ask("j", json!({"title": "Which?"}), "agent")).unwrap();
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
                let ApiError::Store(error) = result.unwrap_err() else { panic!("expected store error") };
                let error = error.to_string();
                assert!(error.contains("could not record event"), "{error}");
                assert!(rx.try_recv().is_err(), "failed changes must not be broadcast as accepted");
                if failure == "readonly" {
                    let reopened = Store::open(dir.path()).unwrap();
                    assert!(reopened.get("j").unwrap().asks[0].answer.is_none());
                } else {
                    assert!(Store::open(dir.path()).is_err(), "damaged logs must prevent startup");
                }
                assert!(app.store.lock().unwrap().get("j").unwrap().asks[0].answer.is_none());
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
                    let _ = app.change(|s| s.shape("sharing", Some("different view".into()), None, None, "agent")).unwrap();
                }
                let path = app.store.lock().unwrap().events_path();
                let head = std::fs::read_to_string(&path).unwrap().lines().last()
                    .map(|line| serde_json::from_str::<Event>(line).unwrap().seq).unwrap();
                let owner = if existing {
                    let view_rev = app.store.lock().unwrap().get("sharing").unwrap().view_rev;
                    Some(app.snapshot_owner("sharing", view_rev).unwrap())
                } else { None };
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
                if let Some(owner) = owner {
                    if successful == 0 {
                        assert!(app.screen("sharing", &owner).is_ok());
                    } else {
                        assert!(!app.screens.lock().unwrap().contains_key("sharing"), "a committed shape invalidates the old screen even when the batch fails");
                        assert!(app.screen("sharing", &owner).is_err());
                    }
                }
                let response = error.into_response();
                let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                let message = body["error"].as_str().unwrap();
                assert!(message.contains("could not record event"), "{message}");
                if successful > 0 {
                    assert!(message.contains(&format!("committed {successful} changes to sharing through seq:{}", head + successful)), "{message}");
                    assert!(message.contains("jelly state sharing"), "{message}");
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
        let ask = app.change(|s| s.ask("j", json!({"title": "Which?"}), "agent")).unwrap().0;
        let path = app.store.lock().unwrap().events_path();
        let original = std::fs::read(&path).unwrap();
        app.store.lock().unwrap().log_failure = Some((ask.seq + 1, 10));
        assert!(app.change(|s| s.answer("j", "a1", None, "yes", "person")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(app.store.lock().unwrap().get("j").unwrap().asks[0].answer.is_none());
        assert!(Store::open(dir.path()).unwrap().get("j").unwrap().asks[0].answer.is_none());
        app.store.lock().unwrap().log_failure = None;
        let answer = app.change(|s| s.answer("j", "a1", None, "yes", "person")).unwrap().0;
        assert_eq!(wait_since(&app, &query(Some("j"), ask.seq)).await.unwrap().unwrap().seq, answer.seq);
    }

    #[tokio::test]
    async fn missing_committed_history_is_an_error() {
        for restart in [false, true] {
            let (dir, mut app) = fixture(4);
            let ask = app.change(|s| s.ask("j", json!({"title": "Which?"}), "agent")).unwrap().0;
            let _ = app.change(|s| s.answer("j", "a1", None, "yes", "person")).unwrap();
            if restart {
                app.store = Mutex::new(Store::open(dir.path()).unwrap());
            }
            let path = app.store.lock().unwrap().events_path();
            std::fs::remove_file(&path).unwrap();
            for since in [0, ask.seq, u64::MAX] {
                let error = wait_since(&app, &query(Some("j"), since)).await.unwrap_err();
                assert!(error.to_string().contains("event history"));
            }
            assert!(!path.exists());
        }
    }

    #[tokio::test]
    async fn missing_history_after_the_first_notification_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let path = store.events_path();
        let (events, _) = broadcast::channel(1);
        let app = App { store: Mutex::new(store), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default(), update: Mutex::default() };
        let q = WaitQ { timeout: Some(1), ..query(None, 0) };
        let waiting = wait_since(&app, &q);
        tokio::pin!(waiting);
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        }).await;
        let _ = app.change(|s| s.make("first", "".into(), "const verbs = {};".into(), json!({}), "person", false, "", "")).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(waiting.await.unwrap_err().to_string().contains("event history"));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn unreadable_or_malformed_history_is_an_error() {
        let (_dir, app) = fixture(4);
        let path = app.store.lock().unwrap().events_path();
        std::fs::write(&path, "invalid json\n").unwrap();
        assert!(wait_since(&app, &query(None, 0)).await.unwrap_err().to_string().contains("event history"));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(wait_since(&app, &query(None, 0)).await.unwrap_err().to_string().contains("event history"));
    }
}
