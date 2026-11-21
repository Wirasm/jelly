//! Asks: one question on a jelly or a round on an ask page, answers, and how a page shows each.

use super::*;

/// How a page shows one open ask.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct AskShown {
    pub(super) id: String,
    /// The view draws it (an element with `data-ask`); otherwise it is a card in
    /// jelly's stack of questions.
    pub(super) drawn: bool,
    pub(super) anchor: AskAnchor,
    /// The anchor's selector, "" without one.
    #[serde(default)]
    pub(super) at: String,
}

/// Whether the element an ask is about is on the page.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AskAnchor {
    /// The ask names no element.
    None,
    Matched,
    Missing,
    /// Its selector is not valid CSS.
    Invalid,
}

#[derive(Deserialize)]
pub(super) struct AskReq {
    #[serde(flatten)]
    pub(super) spec: Value,
}

pub(super) async fn ask(State(app): State<Shared>, Path(name): Path<String>, headers: axum::http::HeaderMap, Json(r): Json<AskReq>) -> Result<Json<Value>, ApiError> {
    let by = r.spec["by"].as_str().map(String::from);
    let asked = now_ms();
    let Json(ev) = match app.live(&name) {
        Some(live) => {
            let by = self::by(by)?;
            let item = crate::store::ask_item(&r.spec, &by)?;
            share::forward(&app, &live, json!({"op": "ask", "item": item, "by": by})).await?
        }
        None => app.change(|s| s.ask(&name, r.spec, &self::by(by)?))?,
    };
    let mut out = json!(ev);
    out["visible"] = json!(app.visible(&name));
    out["notified"] = json!(app.notifies());
    // Tell the asker how the ask shows. The ask is made either way, and the
    // event that listeners and the log get is unchanged.
    let args = ev.args.unwrap_or_default();
    if args["anchor"]["selector"].is_string() {
        match ask_shown(&app, port_of(&headers), &name, args["id"].as_str().unwrap_or(""), ev.version, asked).await {
            Ok(shown) => {
                out["anchor"] = json!(shown.anchor);
                out["drawn"] = json!(shown.drawn);
            }
            Err(why) => out["anchor_unchecked"] = json!(why),
        }
    }
    Ok(Json(out))
}

/// How long an ask waits for a browser before it answers "not checked".
pub(super) const ANCHOR_CHECK: std::time::Duration = std::time::Duration::from_secs(8);

/// How the page shows an ask, from a fresh browser snapshot; failure to render does not undo the ask.
pub(super) async fn ask_shown(app: &App, port: u16, name: &str, item: &str, version: u64, asked: u64) -> Result<AskShown, String> {
    let owner = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(name).map_err(|e| e.to_string())?;
        app.snapshot_owner(name, j.view_rev).map_err(|_| "could not issue snapshot owner".to_string())?
    };
    let render = render(port, name, None, 1280, 900, None, false, ANCHOR_CHECK);
    tokio::pin!(render);
    // The browser often posts its snapshot and then lingers, so answer as soon
    // as the snapshot is in; dropping the render stops the browser.
    let mut rendered: Option<(Result<(), String>, std::time::Instant)> = None;
    loop {
        let snap = app.screen(name, &owner).map_err(|_| "the jelly changed during the anchor check".to_string())?
            .filter(|s| s.version >= version && s.at_ms >= asked);
        if let Some(shown) = snap.and_then(|s| s.asks.into_iter().find(|a| a.id == item)) {
            return Ok(shown);
        }
        match &rendered {
            // The browser is gone: give its snapshot a moment to land, then give up.
            Some((r, at)) if at.elapsed() > std::time::Duration::from_secs(2) => {
                return Err(r.clone().err().unwrap_or_else(|| "the page did not report this question".into()));
            }
            Some(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            None => tokio::select! {
                r = &mut render => rendered = Some((r, std::time::Instant::now())),
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            },
        }
    }
}

#[derive(Deserialize)]
pub(super) struct AnswerReq {
    pub(super) option: Option<String>,
    #[serde(default)]
    pub(super) text: String,
    pub(super) by: Option<String>,
}

pub(super) async fn answer(State(app): State<Shared>, Path((name, id)): Path<(String, String)>, Json(r): Json<AnswerReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "answer", "id": id, "option": r.option, "text": r.text, "by": by(r.by)?})).await;
    }
    app.change(|s| s.answer(&name, &id, r.option.as_deref(), &r.text, &by(r.by)?))
}

pub(super) async fn unask(State(app): State<Shared>, Path((name, id)): Path<(String, String)>, Json(r): Json<ByReq>) -> Result<Json<Event>, ApiError> {
    if let Some(live) = app.live(&name) {
        return share::forward(&app, &live, json!({"op": "unask", "id": id, "by": by(r.by)?})).await;
    }
    app.change(|s| s.unask(&name, &id, &by(r.by)?))
}

#[derive(Deserialize)]
pub(super) struct ProjectQ {
    pub(super) project: Option<String>,
    /// List answered items instead of open ones: the decision record.
    #[serde(default)]
    pub(super) answered: bool,
    /// Only jellies this agent owns (`Jelly::owner`).
    pub(super) mine: Option<String>,
}

/// Everything open for the person, newest first; with `answered=true`, every
/// answered item instead: what was asked, what was picked, by whom, and when.
pub(super) async fn asks_list(State(app): State<Shared>, Query(q): Query<ProjectQ>) -> Json<Value> {
    let store = app.store.lock().expect("store lock");
    let project = q.project.filter(|p| !p.is_empty());
    let mut items: Vec<Value> = Vec::new();
    let wanted = |j: &&crate::store::Jelly| j.name != PAGE && project.as_deref().is_none_or(|p| j.project() == p)
        && q.mine.as_deref().is_none_or(|m| j.owner() == m);
    for j in store.list().filter(wanted) {
        for a in j.asks.iter().filter(|a| if q.answered { a.answer.is_some() } else { a.is_open() }) {
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

#[derive(Deserialize)]
pub(super) struct RoundReq {
    pub(super) spec: Value,
    #[serde(default)]
    pub(super) project: String,
    pub(super) by: Option<String>,
}

/// A round of questions on an ask page, made or updated in one step.
pub(super) async fn ask_round(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<RoundReq>) -> Result<Json<Value>, ApiError> {
    let who = by(r.by)?;
    let id = if r.project.is_empty() || name.contains(':') { name.clone() } else { format!("{}:{name}", r.project) };
    app.not_shared(&id, "start a new round of questions")?;
    app.change_many(|s| s.ask_round(&name, &r.spec, ASK_VIEW, ASK_VERBS, &r.project, &who))
}

#[cfg(test)]
mod ask_shown_tests {
    use super::*;

    #[tokio::test]
    async fn an_ask_answers_with_how_the_page_shows_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.make("shown", "<p>Test</p>".into(), "const verbs = {};".into(), json!({}), "test", false, "", "").unwrap();
        let (events, _) = broadcast::channel(16);
        let app = Arc::new(App { store: Mutex::new(store), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default(), feeds: Mutex::default() });
        for (drawn, anchor, line) in [
            (false, "matched", "it shows in jelly's stack of questions, with a pin on #tile"),
            (false, "missing", "#tile matches nothing on the page, so it has no pin; it shows in jelly's stack of questions"),
            (true, "missing", "the view draws it; #tile matches nothing on the page"),
        ] {
            let mut received = app.events.subscribe();
            let post_snapshot = async {
                let ev = received.recv().await.unwrap();
                let owner = {
                    let store = app.store.lock().unwrap();
                    app.snapshot_owner("shown", store.get("shown").unwrap().view_rev).unwrap()
                };
                let before: Screen = serde_json::from_value(json!({"owner": owner, "text": "", "width": 1280, "height": 900, "version": ev.version})).unwrap();
                // A snapshot that does not know the ask yet must not answer for it.
                put_screen(State(app.clone()), Path("shown".into()), Json(before.clone())).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let mut after = before;
                after.asks = serde_json::from_value(json!([{"id": ev.args.as_ref().unwrap()["id"], "drawn": drawn, "anchor": anchor, "at": "#tile"}])).unwrap();
                put_screen(State(app.clone()), Path("shown".into()), Json(after)).await.unwrap();
            };
            let request = ask(State(app.clone()), Path("shown".into()), axum::http::HeaderMap::new(), Json(AskReq {
                spec: json!({"title": "Choose", "by": "test", "anchor": {"selector": "#tile"}})
            }));
            let (response, ()) = tokio::join!(request, post_snapshot);
            let Json(response) = response.unwrap_or_else(|_| panic!("ask failed"));
            assert_eq!((&response["anchor"], &response["drawn"]), (&json!(anchor), &json!(drawn)));
            assert_eq!(crate::shown(&response), line);
        }
        let log = std::fs::read_to_string(app.store.lock().unwrap().events_path()).unwrap();
        assert!(!log.contains("drawn"), "how an ask shows is not part of its event");
    }
}
