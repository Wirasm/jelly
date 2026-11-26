//! What an agent sees of a jelly: snapshots from the browser, `jelly look`, `--compare` and `jelly try`.

use super::*;

// ---- the agent view -------------------------------------------------------

/// What a browser shows of a jelly, as text, posted by the runtime for `jelly look`.
#[derive(Deserialize, Clone)]
pub(super) struct Screen {
    pub(super) owner: String,
    pub(super) text: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) version: u64,
    #[serde(default)]
    pub(super) at_ms: u64,
    /// Errors the page threw or logged.
    #[serde(default)]
    pub(super) errors: Vec<String>,
    /// Dialogs the page opened (`alert`, `confirm`, `prompt`) in an agent render,
    /// where nobody could close them: `alert "Saved"`.
    #[serde(default)]
    pub(super) dialogs: Vec<String>,
    #[serde(default)]
    pub(super) page_height: u32,
    /// Every open ask and how the page shows it.
    #[serde(default)]
    pub(super) asks: Vec<AskShown>,
    /// Rendered with `prefers-color-scheme: dark` for `jelly look`'s contrast check.
    #[serde(default)]
    pub(super) dark: bool,
    /// Text below WCAG AA contrast against what is painted behind it.
    #[serde(default)]
    pub(super) contrast: Vec<Contrast>,
    /// Other measured problems by kind. Only `jelly look`'s renders measure them, so
    /// `None` is a snapshot from a person's browser.
    #[serde(default)]
    pub(super) problems: Option<Vec<Problem>>,
}

#[derive(Deserialize, Clone, PartialEq)]
pub(super) struct Contrast {
    /// A selector for the element, for `jelly try` and `ask --at`.
    pub(super) at: String,
    pub(super) text: String,
    /// The text and background colors as the page paints them, `#rrggbb`.
    pub(super) fg: String,
    pub(super) bg: String,
    pub(super) ratio: f64,
}

#[derive(Deserialize, Clone)]
pub(super) struct Problem {
    pub(super) kind: String,
    pub(super) items: Vec<String>,
}

pub(super) struct ViewScreen {
    pub(super) owner: String,
    pub(super) view_rev: u64,
    pub(super) screen: Option<Screen>,
    pub(super) dark: Option<Screen>,
}

pub(super) async fn put_screen(State(app): State<Shared>, Path(name): Path<String>, Json(mut sc): Json<Screen>) -> Result<StatusCode, ApiError> {
    let store = app.store.lock().expect("store lock");
    let j = store.get(&name)?;
    let mut screens = app.screens.lock().expect("screens lock");
    let entry = screens.get_mut(&name).filter(|s| s.owner == sc.owner && s.view_rev == j.view_rev)
        .ok_or_else(snapshot_changed)?;
    sc.at_ms = now_ms();
    if sc.dark { entry.dark = Some(sc) } else { entry.screen = Some(sc) }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct NoteReq {
    pub(super) text: String,
    pub(super) by: Option<String>,
}

pub(super) async fn note(State(app): State<Shared>, Path(name): Path<String>, Json(r): Json<NoteReq>) -> Result<Json<Event>, ApiError> {
    app.change(|s| s.note(&name, &r.text, &by(r.by)?))
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A browser the daemon can drive headless: JELLY_CHROME, or the usual installs.
pub(super) fn chrome() -> Option<String> {
    let mut candidates = vec![
        std::env::var("JELLY_CHROME").unwrap_or_default(),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into(),
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".into(),
        "/usr/bin/google-chrome".into(),
        "/usr/bin/chromium".into(),
        "/usr/bin/chromium-browser".into(),
    ];
    // Windows installs go under one of the program folders, which the environment names.
    for (folder, install) in [
        ("ProgramFiles", r"Google\Chrome\Application\chrome.exe"),
        ("ProgramFiles(x86)", r"Google\Chrome\Application\chrome.exe"),
        ("LOCALAPPDATA", r"Google\Chrome\Application\chrome.exe"),
        ("LOCALAPPDATA", r"Chromium\Application\chrome.exe"),
        ("ProgramFiles(x86)", r"Microsoft\Edge\Application\msedge.exe"),
        ("ProgramFiles", r"BraveSoftware\Brave-Browser\Application\brave.exe"),
    ] {
        if let Some(root) = std::env::var_os(folder) {
            candidates.push(std::path::Path::new(&root).join(install).to_string_lossy().into_owned());
        }
    }
    candidates.into_iter().find(|c| !c.is_empty() && std::path::Path::new(c).exists())
}

/// How long `jelly look` waits for the browser.
pub(super) const RENDER_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);

/// The `failed` line for a render that did not finish: what went unchecked, why,
/// and what to do. Agents fix the page for every `!` line; this one says
/// nothing either way about the page, only another render can.
pub(super) fn render_failed(unchecked: &str, why: &str, command: &str) -> String {
    format!("failed   {unchecked}: {why}. Run `jelly {command}` again; if it keeps failing, report it")
}

/// Runs headless Chrome on a jelly at a given size. With `png`, writes a
/// screenshot there; either way the page's runtime posts a fresh snapshot.
/// With `trial`, the page runs that `jelly try`'s steps. With `dark`, the page
/// prefers the dark color scheme; otherwise light, whatever the machine uses.
#[allow(clippy::too_many_arguments)]
pub(super) async fn render(port: u16, name: &str, trial: Option<&str>, width: u32, height: u32, png: Option<&std::path::Path>, dark: bool, limit: std::time::Duration) -> Result<(), String> {
    let bin = chrome().ok_or("no Chrome, Chromium, Edge or Brave found; set JELLY_CHROME to a browser binary")?;
    run_browser(&bin, port, name, trial, width, height, png, dark, limit).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_browser(bin: &str, port: u16, name: &str, trial: Option<&str>, width: u32, height: u32, png: Option<&std::path::Path>, dark: bool, limit: std::time::Duration) -> Result<(), String> {
    // Trial tokens are URL-safe base64, so they go into the URL as they are.
    // `&dark` tells the runtime its snapshot is the dark one.
    let trial = format!("{}{}", trial.map(|t| format!("&try={t}")).unwrap_or_default(), if dark { "&dark=1" } else { "" });
    // Headless Chrome will not make a window narrower than 500px, so phone
    // widths render the jelly in an iframe of exactly that size.
    let url = if width < 500 {
        format!("http://127.0.0.1:{port}/look-frame/{name}?w={width}&h={height}{trial}")
    } else {
        format!("http://127.0.0.1:{port}/j/{name}/?agent=1{trial}")
    };
    // The phone frame is scaled up to 500px wide, so the window grows with it.
    let height = if width < 500 { (height as f64 * 500.0 / width.max(1) as f64).ceil() as u32 } else { height };
    run_browser_at(bin, &url, width.max(500), height, png, dark, limit).await
}

/// Runs headless Chrome on `url` in a window of this size, light or dark. With
/// `png`, writes a screenshot of the window there; either way the page posts its snapshot.
/// Every jelly daemon on the machine shares a few browser slots. Without a cap,
/// several agents (or the test suite) starting light and dark renders at once
/// starve each other until renders hit their time limit. Each slot is a lock
/// file in the temp dir, so the cap holds across processes; a crashed daemon's
/// lock goes with it.
pub(super) async fn browser_slot() -> Result<std::fs::File, String> {
    let slots = std::thread::available_parallelism().map_or(4, |n| n.get()).div_ceil(2).max(2);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        for i in 0..slots {
            let path = std::env::temp_dir().join(format!("jelly-browser-slot-{i}.lock"));
            if let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path) {
                if file.try_lock().is_ok() {
                    return Ok(file);
                }
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("waited 60 s for a free browser: too many renders are running on this machine".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

pub(super) async fn run_browser_at(bin: &str, url: &str, width: u32, height: u32, png: Option<&std::path::Path>, dark: bool, limit: std::time::Duration) -> Result<(), String> {
    // The render's own time limit starts once it has a browser slot.
    let _slot = browser_slot().await?;
    // Chrome refuses a profile another Chrome is using (exit 21), so each render
    // gets its own. A fresh profile starts as fast as a used one.
    static RENDERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RENDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let profile = std::env::temp_dir().join(format!("jelly-chrome-{}-{n}", std::process::id()));
    let mut cmd = tokio::process::Command::new(bin);
    // WebGL views get the machine's GPU, or SwiftShader, Chrome's software GPU,
    // where there is none. Forcing SwiftShader (`--use-angle=swiftshader`) holds
    // the page's clock back a second or more per render, and long enough under
    // load for renders to hit their time limit. `--disable-gpu` would leave
    // WebGL views with no context at all.
    cmd.args(["--headless=new", "--enable-unsafe-swiftshader", "--hide-scrollbars", "--no-first-run", "--no-default-browser-check", "--mute-audio"])
        .arg(format!("--user-data-dir={}", profile.display()))
        // Headless Chrome follows the machine's appearance unless told; 0 is dark, 1 light.
        .arg(format!("--blink-settings=preferredColorScheme={}", if dark { 0 } else { 1 }));
    if cfg!(windows) {
        // On Windows `--window-size` is the outer size, so the page comes out narrower than
        // asked and its snapshot is refused; a fullscreen window on a screen of that size is exact.
        cmd.arg(format!("--screen-info={{{width}x{height}}}")).arg("--start-fullscreen");
    } else {
        cmd.arg(format!("--window-size={width},{height}"));
    }
    // Port 0: Chrome picks a free port and writes it to the profile.
    cmd.args(["--remote-debugging-port=0", "about:blank"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let result = match cmd.spawn() {
        Ok(mut child) => {
            let result = tokio::time::timeout(limit, drive(&mut child, &profile, url, png)).await
                .unwrap_or_else(|_| Err(format!("the browser took longer than {} s", limit.as_secs())));
            let _ = child.kill().await;
            result
        }
        Err(e) => Err(format!("could not run the browser: {e}")),
    };
    // Best effort: a leftover folder in the temp dir harms nothing.
    let _ = tokio::fs::remove_dir_all(&profile).await;
    result
}

/// Virtual time a render gives the page: its timers run this far ahead, and
/// the render ends when they have.
const BUDGET_MS: u64 = 2500;
/// How long, in real time, a render waits for the page's network to go quiet.
/// A page that keeps a connection open (an EventSource, a streaming fetch, an
/// app in an iframe that does either) never goes quiet.
const NETWORK_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
/// A page that never goes quiet gets its virtual time in steps this long, one
/// per as much real time, so its requests still get answers in between.
const STEP_MS: u64 = 500;

/// Loads `url` in the browser over the DevTools protocol, runs the page's clock
/// for `BUDGET_MS`, and with `png` writes a screenshot there.
///
/// The clock is Chrome's virtual time, as `--virtual-time-budget` uses: it
/// stops while requests are pending, so the page gets its data before its
/// timers run out. A request that never ends would stop it for good, so after
/// `NETWORK_WAIT` the clock runs at about real speed whatever the network does.
async fn drive(child: &mut tokio::process::Child, profile: &std::path::Path, url: &str, png: Option<&std::path::Path>) -> Result<(), String> {
    // The file holds the port, then the browser's socket path.
    let socket = loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the browser failed ({status})"));
        }
        if let Some((port, path)) = tokio::fs::read_to_string(profile.join("DevToolsActivePort")).await.ok().as_deref().and_then(|s| s.split_once('\n')) {
            if !path.trim().is_empty() { break format!("ws://127.0.0.1:{port}{}", path.trim()); }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let mut cdp = Devtools::connect(&socket).await?;
    let targets = cdp.call("Target.getTargets", json!({})).await?;
    let page = targets["targetInfos"].as_array().into_iter().flatten().find(|t| t["type"] == "page")
        .and_then(|t| t["targetId"].as_str()).ok_or("the browser opened no page")?.to_string();
    let attached = cdp.call("Target.attachToTarget", json!({ "targetId": page, "flatten": true })).await?;
    cdp.session = attached["sessionId"].as_str().map(String::from);
    // What `--virtual-time-budget` does (Chrome's headless_command.js): load
    // the page, then give it the budget.
    cdp.call("Page.enable", json!({})).await?;
    let nav = cdp.call("Page.navigate", json!({ "url": url })).await?;
    if let Some(e) = nav["errorText"].as_str().filter(|e| !e.is_empty()) {
        return Err(format!("the browser could not load the page: {e}"));
    }
    // A frame that streams forever never finishes loading; go on without it.
    if let Ok(loaded) = tokio::time::timeout(NETWORK_WAIT, cdp.event("Page.loadEventFired")).await { loaded?; }
    cdp.call("Emulation.setVirtualTimePolicy", json!({ "policy": "pauseIfNetworkFetchesPending", "budget": BUDGET_MS, "maxVirtualTimeTaskStarvationCount": 9999 })).await?;
    if tokio::time::timeout(NETWORK_WAIT, cdp.event(BUDGET_SPENT)).await.is_err() {
        // A connection is still open. Run the clock in steps at about real
        // speed, so the page's own requests still get answers between steps.
        let mut left = BUDGET_MS;
        while left > 0 {
            let step = STEP_MS.min(left);
            cdp.call("Emulation.setVirtualTimePolicy", json!({ "policy": "advance", "budget": step })).await?;
            cdp.event(BUDGET_SPENT).await?;
            tokio::time::sleep(std::time::Duration::from_millis(step)).await;
            left -= step;
        }
    }
    if let Some(p) = png {
        use base64::Engine as _;
        // A spent budget stops the clock, and a stopped page may not paint the
        // frame a screenshot waits for.
        cdp.call("Emulation.setVirtualTimePolicy", json!({ "policy": "advance" })).await?;
        let shot = cdp.call("Page.captureScreenshot", json!({ "format": "png" })).await?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(shot["data"].as_str().unwrap_or_default())
            .map_err(|e| format!("unreadable screenshot from the browser: {e}"))?;
        tokio::fs::write(p, bytes).await.map_err(|e| format!("could not write {}: {e}", p.display()))?;
    }
    Ok(())
}

const BUDGET_SPENT: &str = "Emulation.virtualTimeBudgetExpired";

/// One connection to a browser's DevTools socket, attached to one page once `session` is set.
struct Devtools {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    id: u64,
    session: Option<String>,
    /// Events that came while waiting for an answer, by method.
    events: Vec<String>,
}

impl Devtools {
    async fn connect(socket: &str) -> Result<Self, String> {
        // A full-page screenshot comes back as one message of several megabytes.
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default().max_message_size(None).max_frame_size(None);
        let (ws, _) = tokio_tungstenite::connect_async_with_config(socket, Some(config), false).await
            .map_err(|e| format!("could not reach the browser: {e}"))?;
        Ok(Devtools { ws, id: 0, session: None, events: Vec::new() })
    }

    /// Sends one command and waits for its answer.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        use futures_util::SinkExt;
        self.id += 1;
        let mut msg = json!({ "id": self.id, "method": method, "params": params });
        if let Some(s) = &self.session { msg["sessionId"] = json!(s); }
        self.ws.send(tokio_tungstenite::tungstenite::Message::text(msg.to_string())).await.map_err(|e| format!("lost the browser: {e}"))?;
        loop {
            let m = self.next().await?;
            if m["id"] == self.id {
                if let Some(e) = m.get("error") { return Err(format!("the browser refused {method}: {}", e["message"])); }
                return Ok(m["result"].clone());
            }
            if let Some(event) = m["method"].as_str() { self.events.push(event.into()); }
        }
    }

    /// Waits for the event `method`, or takes it if it already came.
    async fn event(&mut self, method: &str) -> Result<(), String> {
        if let Some(i) = self.events.iter().position(|e| e == method) {
            self.events.remove(i);
            return Ok(());
        }
        while self.next().await?["method"] != method {}
        Ok(())
    }

    /// The next message from the browser: an answer or an event.
    async fn next(&mut self) -> Result<Value, String> {
        use tokio_tungstenite::tungstenite::Message;
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    let m: Value = serde_json::from_str(t.as_str()).map_err(|e| format!("unreadable message from the browser: {e}"))?;
                    return Ok(m);
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(format!("lost the browser: {e}")),
                None => return Err("the browser closed".into()),
            }
        }
    }
}

#[derive(Deserialize)]
pub(super) struct LookQ {
    /// Render only this width. Without it, desktop and phone side by side.
    pub(super) width: Option<u32>,
    pub(super) height: Option<u32>,
    /// With `width`: also take a screenshot.
    #[serde(default)]
    pub(super) png: bool,
    /// The whole page, not just the first screen.
    #[serde(default)]
    pub(super) full: bool,
    /// Answer with JSON instead of text.
    #[serde(default)]
    pub(super) json: bool,
    /// Compare with an earlier version (`v12`) or one saved with `save`.
    pub(super) compare: Option<String>,
    /// Keep this version under a name to compare with later.
    pub(super) save: Option<String>,
}

/// `jelly look`: everything an agent needs to pick up a jelly, as plain text:
/// what it is, what the person did, what waits on them, and what is on screen.
/// At desktop and phone width by default, at one `width`, or compared with an
/// earlier version.
pub(super) async fn look(State(app): State<Shared>, Path(name): Path<String>, Query(mut q): Query<LookQ>, req_headers: axum::http::HeaderMap) -> Result<Response, ApiError> {
    let port = port_of(&req_headers);
    // Read what to compare with before saving, so `--compare x --save x` compares with the old x.
    let before = q.compare.as_deref().map(|c| past(&app, &name, c)).transpose()?;
    if let Some(label) = q.save.take() { save(&app, &name, &label)?; }
    if let Some(before) = before {
        return compare(&app, port, &name, before, &q).await;
    }
    if q.width.is_none() {
        return look_both(&app, port, &name, q.json).await;
    }
    let render_name = name.clone();
    look_at_width(app, name, q, move |width, height, png: Option<std::path::PathBuf>, dark| {
        let name = render_name.clone();
        async move { render(port, &name, None, width, height, png.as_deref(), dark, RENDER_LIMIT).await }
    }).await
}

/// One width: a light and a dark render at once. The light one takes the png,
/// unless `full` needs a third render at the height the light one measured.
pub(super) async fn look_at_width<F, Fut>(app: Shared, name: String, q: LookQ, mut render_page: F) -> Result<Response, ApiError>
where
    F: FnMut(u32, u32, Option<std::path::PathBuf>, bool) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let (j, home, owner) = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(&name)?;
        (j.clone(), store.home().to_path_buf(), app.snapshot_owner(&name, j.view_rev)?)
    };
    let width = q.width.unwrap_or(1280);
    let height = q.height.unwrap_or(if width <= 500 { 844 } else { 900 });
    // Only look's own renders measure problems, so a person's snapshot does not
    // count. A snapshot from before the last change of any kind (an ask added
    // since, say) has not measured it.
    let fresh = |app: &App, dark: bool| -> Result<Option<Screen>, ApiError> {
        Ok(app.screen_in(&name, &owner, dark)?.filter(|s| s.version == j.version && s.at_ms >= j.updated_ms && s.width == width && s.problems.is_some()))
    };
    let mut notes: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut png_path = q.png.then(|| {
        let dir = home.join("renders");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{}-{width}.png", crate::disk::name(&name)))
    });
    let mut png = None;
    let mut snap = fresh(&app, false)?;
    // The dark render only checks contrast. It runs beside the light one, so it
    // costs a second browser but little extra time.
    let mut dark = fresh(&app, true)?;
    if snap.is_none() || dark.is_none() {
        let asked = now_ms();
        let shot = png_path.clone().filter(|_| !q.full);
        let light_run = snap.is_none().then(|| render_page(width, height, shot.clone(), false));
        let dark_run = dark.is_none().then(|| render_page(width, height, None, true));
        let (light_run, dark_run) = tokio::join!(maybe(light_run), maybe(dark_run));
        // A light render that takes the png can fail at the screenshot after the
        // page sent its snapshot, so that snapshot is still waited for.
        let light_err = light_run.as_ref().and_then(|r| r.as_ref().err().cloned());
        let wait_light = match &light_run {
            Some(Err(_)) => shot.is_some(),
            Some(Ok(())) => { png = shot.clone(); true }
            None => false,
        };
        let wait_dark = match dark_run {
            Some(Err(e)) => { failed.push(render_failed(&format!("dark mode at {width}px was not checked"), &e, "look")); false }
            ran => ran.is_some(),
        };
        for _ in 0..30 {
            if wait_light && snap.is_none() { snap = fresh(&app, false)?.filter(|s| s.at_ms >= asked); }
            if wait_dark && dark.is_none() { dark = fresh(&app, true)?.filter(|s| s.at_ms >= asked); }
            if (!wait_light || snap.is_some()) && (!wait_dark || dark.is_some()) { break; }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        match light_err {
            Some(e) if snap.is_some() => { failed.push(render_failed("no screenshot", &e, "look")); png_path = None; }
            Some(e) => failed.push(render_failed(&format!("light mode at {width}px was not checked"), &e, "look")),
            None if wait_light && snap.is_none() => notes.push("The page rendered but sent no snapshot.".into()),
            None => {}
        }
        if wait_dark && dark.is_none() { notes.push("The dark mode render sent no snapshot.".into()); }
    }
    if let Some(path) = png_path.filter(|_| png.is_none()) {
        // The full page: as tall as the page said it was, within reason.
        let h = if q.full { snap.as_ref().map(|s| s.page_height).filter(|h| *h > height).unwrap_or(height).min(12000) } else { height };
        match render_page(width, h, Some(path.clone()), false).await {
            Ok(()) => png = Some(path),
            Err(e) => failed.push(render_failed("no screenshot", &e, "look")),
        }
    }

    app.validate_render(&name, &owner, j.version)?;
    let mut out = summary(&app, &j);
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    if let Some(p) = &png { line(&mut out, format!("png      {}", p.display())); }
    // Everything from here to the outline is a problem line.
    let problems_from = out.len();
    if let Some(sc) = &snap {
        for p in screen_problems(sc) { line(&mut out, p); }
        if sc.page_height > sc.height && !q.full && png.is_some() { line(&mut out, format!("!        the page is {}px tall and the png shows the first {}px; --full for all of it", sc.page_height, sc.height)); }
    }
    for p in problem_lines(snap.as_ref(), dark.as_ref()) { line(&mut out, format!("!        {p}")); }
    for n in notes { line(&mut out, format!("!        {n}")); }
    for f in failed { line(&mut out, f); }
    if q.json {
        return Ok(Json(json!({
            "jelly": j.name, "version": j.version,
            "summary": &out[..problems_from],
            "problems": out[problems_from..].lines().collect::<Vec<_>>(),
            "png": png,
            "outline": snap.map(|s| s.text),
        })).into_response());
    }
    if let Some(s) = snap {
        line(&mut out, format!("\non screen at {}×{} (v{}):", s.width, s.height, s.version));
        out.push_str(&s.text);
        out.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], out).into_response())
}

/// The part of `jelly look` that needs no browser: what the jelly is, who made
/// it, its verbs and state, what waits on the person and what they did lately.
pub(super) fn summary(app: &App, j: &crate::store::Jelly) -> String {
    let mut out = String::new();
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    let (last, mine) = app.store.lock().expect("store lock").recent_events(&j.name);
    // A trim can drop the newest event while an older one of the person's stays:
    // only the event that made this version says who made the last change.
    let by = last.filter(|e| e.version == j.version).map(|e| format!(" by {}", e.by)).unwrap_or_default();
    let last = format!(" · last change {}{by}", ago(j.updated_ms));
    let shaped = j.shaped_by.as_ref().filter(|s| **s != j.made_by).map(|s| format!(" · shaped by {s}")).unwrap_or_default();
    line(&mut out, format!("{} · {} · v{} · made by {}{shaped}{last}{}{}", j.name, if j.about.is_empty() { "(no about line)" } else { &j.about }, j.version, j.made_by,
        if j.kept { " · kept" } else { "" }, if j.archived { " · archived" } else { "" }));
    line(&mut out, if app.visible(&j.name) > 0 { "screen   someone has it open, in front".to_string() } else { "screen   nobody has it open right now".to_string() });
    if j.unsent > 0 { line(&mut out, format!("unsent   {} change{} by the person since their last Send", j.unsent, if j.unsent == 1 { "" } else { "s" })); }
    if let Some(feed) = feed::line(app, app.store.lock().expect("store lock").home(), &j.name) { line(&mut out, feed); }
    if let Some(w) = &j.working { line(&mut out, format!("working  {} is working on it: {}", w["by"].as_str().unwrap_or("?"), w["what"].as_str().unwrap_or(""))); }
    for n in &j.notes { line(&mut out, format!("note     {}: {}", n["by"].as_str().unwrap_or("?"), n["text"].as_str().unwrap_or(""))); }
    let verbs = crate::verbs::check(&j.verbs).unwrap_or_default();
    line(&mut out, format!("verbs    {}", if verbs.is_empty() { "none (read-only)".to_string() } else { verbs.iter().map(|v| verb_sig(&j.verbs, v)).collect::<Vec<_>>().join(" · ") }));
    line(&mut out, format!("state    {}", state_shape(&j.state)));
    let assets = app.store.lock().expect("store lock").assets(&j.name);
    if !assets.is_empty() { line(&mut out, format!("assets   {}", assets.join(", "))); }
    for a in j.asks.iter().filter(|a| a.is_open()) {
        line(&mut out, format!("waits    {}", a.summary()));
    }
    for a in j.asks.iter().filter(|a| a.answer.is_some()).rev().take(5) {
        let ans = a.answer.as_ref().expect("filtered");
        let pick = ans.option.as_deref().unwrap_or(if a.seen { "seen" } else { "" });
        line(&mut out, format!("answered {} {:?} → {pick}{}{}", a.id, a.title, if ans.text.is_empty() { String::new() } else { format!(" {:?}", ans.text) }, named(&ans.by)));
    }
    for f in j.comments.iter().filter(|f| !f.resolved) {
        line(&mut out, format!("comment  {} on {}{}: {:?}{}", f.id, crate::store::comment_place(&f.anchor), named(&f.by), f.text, if f.replies.is_empty() { "" } else { " (thread)" }));
    }
    for e in &mine {
        line(&mut out, format!("person   {} {}{}", ago(e.at_ms), viewer(&e.by).map(|n| format!("{n} ")).unwrap_or_default(), e.describe()));
    }
    let project = j.project();
    if !project.is_empty() && app.store.lock().expect("store lock").project_dir(project).is_ok_and(|d| d.join("DESIGN.md").exists()) {
        line(&mut out, format!("design   project {project} has a DESIGN.md: jelly design · CSS variables at /p/{project}/design.css"));
    }
    out
}

/// Errors the page threw, dialogs it opened and asks about an element that is
/// not there, as `error`, `dialog` and `!` lines.
pub(super) fn screen_problems(sc: &Screen) -> Vec<String> {
    let mut out = Vec::new();
    for e in &sc.errors { out.push(format!("error    {e}")); }
    for d in &sc.dialogs { out.push(format!("dialog   {d}")); }
    for a in &sc.asks {
        let why = match a.anchor {
            AskAnchor::Missing => "matches nothing on the page",
            AskAnchor::Invalid => "has invalid CSS selector syntax",
            AskAnchor::None | AskAnchor::Matched => continue,
        };
        out.push(format!("!        {} asks about {}, which {why}{}", a.id, a.at, if a.drawn { "" } else { "; it has no pin" }));
    }
    out
}

/// Awaits a render that may not have been started.
pub(super) async fn maybe<F: std::future::Future>(run: Option<F>) -> Option<F::Output> {
    match run { Some(f) => Some(f.await), None => None }
}

/// How many of each kind of problem `jelly look` prints; the rest are counted.
pub(super) const PROBLEMS_SHOWN: usize = 3;

/// The measured problems as `kind: item` lines, a few of each kind. Unreadable
/// text says which color scheme it is unreadable in when it is only one of them,
/// and gets one line per color pair, worst first: the fix is usually one color.
pub(super) fn problem_lines(light: Option<&Screen>, dark: Option<&Screen>) -> Vec<String> {
    let none = Vec::new();
    // Without a dark render there is nothing to tell the schemes apart by.
    let l = light.map_or(&none, |s| &s.contrast);
    let d = dark.map_or(l, |s| &s.contrast);
    let mut out = Vec::new();
    let mut add = |kind: &str, items: Vec<String>, unit: &str| {
        out.extend(items.iter().take(PROBLEMS_SHOWN).map(|item| format!("{kind}: {item}")));
        if items.len() > PROBLEMS_SHOWN { out.push(format!("{kind}: {} more{unit}", items.len() - PROBLEMS_SHOWN)); }
    };
    for (kind, found) in [
        ("unreadable text", l.iter().filter(|c| d.contains(c)).collect::<Vec<_>>()),
        ("unreadable text in light mode", l.iter().filter(|c| !d.contains(c)).collect()),
        ("unreadable text in dark mode", d.iter().filter(|c| !l.contains(c)).collect()),
    ] {
        let mut pairs: Vec<(&Contrast, usize)> = Vec::new();
        for c in found {
            match pairs.iter_mut().find(|(p, _)| p.fg == c.fg && p.bg == c.bg) {
                Some((_, more)) => *more += 1,
                None => pairs.push((c, 0)),
            }
        }
        pairs.sort_by(|a, b| a.0.ratio.total_cmp(&b.0.ratio));
        add(kind, pairs.iter().map(|(c, more)| format!("{} on {} is {:.1}:1 at {} {:?}{}", c.fg, c.bg, c.ratio, c.at, c.text,
            if *more > 0 { format!(" and {more} more like it") } else { String::new() })).collect(), " color pairs");
    }
    for p in light.and_then(|s| s.problems.as_ref()).into_iter().flatten() { add(&p.kind, p.items.clone(), ""); }
    out
}

// ---- jelly look ----------------------------------------------------------------

/// One page in a `jelly look` render at two widths or two versions. Its snapshot goes
/// here, not into the jelly's shared one, so pages at several widths or
/// versions can render at once.
pub(super) struct Probe {
    pub(super) name: String,
    pub(super) width: u32,
    pub(super) height: u32,
    /// Shown above the page in the side-by-side screenshot.
    pub(super) label: String,
    pub(super) dark: bool,
    /// An earlier version to show instead of the current one. Agent renders
    /// cannot call verbs, so this only ever shows the past.
    pub(super) past: Option<Past>,
    pub(super) screen: Option<Screen>,
}

impl App {
    pub(super) fn start_probe(&self, name: &str, (width, height): (u32, u32), label: &str, dark: bool, past: Option<&Past>) -> Result<String, ApiError> {
        let token = random_token()?;
        let probe = Probe { name: name.into(), width, height, label: label.into(), dark, past: past.cloned(), screen: None };
        self.probes.lock().expect("probes lock").insert(token.clone(), probe);
        Ok(token)
    }
}

/// Removes the probes when dropped, so a cancelled request leaves none behind.
pub(super) struct Probes<'a>(&'a App, Vec<String>);

impl Drop for Probes<'_> {
    fn drop(&mut self) {
        let mut probes = self.0.probes.lock().expect("probes lock");
        for t in &self.1 { probes.remove(t); }
    }
}

pub(super) const FRAME_PAD: u32 = 16;
pub(super) const FRAME_LABEL: u32 = 24;

/// Renders these probes side by side in one browser and waits for each page's
/// snapshot. With `png`, the screenshot holds them all, each under its label.
pub(super) async fn render_probes(app: &App, port: u16, tokens: &[String], png: Option<&std::path::Path>) -> Result<Vec<Option<Screen>>, String> {
    let bin = chrome().ok_or("no Chrome, Chromium, Edge or Brave found; set JELLY_CHROME to a browser binary")?;
    let (width, height, dark) = {
        let probes = app.probes.lock().expect("probes lock");
        let sizes: Vec<_> = tokens.iter().filter_map(|t| probes.get(t)).map(|p| (p.width, p.height, p.dark)).collect();
        (FRAME_PAD + sizes.iter().map(|s| s.0 + FRAME_PAD).sum::<u32>(),
         FRAME_LABEL + sizes.iter().map(|s| s.1).max().unwrap_or(0) + FRAME_PAD,
         sizes.iter().any(|s| s.2))
    };
    let url = format!("http://127.0.0.1:{port}/probe-frame?t={}", tokens.join(","));
    run_browser_at(&bin, &url, width.max(500), height, png, dark, RENDER_LIMIT).await?;
    // The page posts its snapshot shortly before the browser exits; give it a moment to land.
    let taken = || -> Vec<Option<Screen>> {
        let probes = app.probes.lock().expect("probes lock");
        tokens.iter().map(|t| probes.get(t).and_then(|p| p.screen.clone())).collect()
    };
    for _ in 0..30 {
        if taken().iter().all(Option::is_some) { break; }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(taken())
}

#[derive(Deserialize)]
pub(super) struct ProbeFrameQ {
    /// Probe tokens, comma-separated, in the order they show left to right.
    pub(super) t: String,
}

/// The page a probe render loads: each probe's jelly in a frame of its exact
/// size, side by side, labelled. Phone widths need the frame anyway, because
/// headless Chrome will not make a window narrower than 500px.
pub(super) async fn probe_frame(State(app): State<Shared>, Query(q): Query<ProbeFrameQ>) -> Html<String> {
    let probes = app.probes.lock().expect("probes lock");
    let frames: String = q.t.split(',').filter_map(|t| probes.get(t).map(|p| (t, p))).map(|(t, p)| {
        // Only tokens the daemon issued get here, and they are URL-safe base64.
        format!("<div><div class=\"l\">{}</div><iframe src=\"/j/{}/?agent=1&probe={t}{}\" style=\"width:{}px;height:{}px\"></iframe></div>",
            p.label, p.name, if p.dark { "&dark=1" } else { "" }, p.width, p.height)
    }).collect();
    Html(format!("<!doctype html><meta charset=\"utf-8\"><style>body{{margin:0;padding:0 {FRAME_PAD}px;display:flex;gap:{FRAME_PAD}px;align-items:flex-start;background:#c9c9d1}}\
        .l{{height:{FRAME_LABEL}px;line-height:{FRAME_LABEL}px;font:600 13px system-ui,sans-serif;color:#222}}\
        iframe{{display:block;border:0;background:#fff}}</style><body>{frames}</body>"))
}

/// A probe page's snapshot.
pub(super) async fn put_probe_screen(State(app): State<Shared>, Path(token): Path<String>, Json(mut sc): Json<Screen>) -> Result<StatusCode, ApiError> {
    let mut probes = app.probes.lock().expect("probes lock");
    let probe = probes.get_mut(&token).ok_or_else(|| ApiError::Store(Error::Invalid("this render is over".into())))?;
    sc.at_ms = now_ms();
    probe.screen = Some(sc);
    Ok(StatusCode::NO_CONTENT)
}

/// How outline `to` differs from outline `from`, by line. A line whose `#id`
/// is on both sides changed; so did a line without an id replaced by one of the
/// same kind (`text`, `button`, …) in the same place. Other lines were added or
/// removed.
#[derive(Serialize, Default)]
pub(super) struct OutlineDiff {
    pub(super) added: Vec<String>,
    pub(super) removed: Vec<String>,
    pub(super) changed: Vec<Changed>,
}

#[derive(Serialize)]
pub(super) struct Changed {
    pub(super) from: String,
    pub(super) to: String,
}

pub(super) fn outline_diff(from: &str, to: &str) -> OutlineDiff {
    let (a, b): (Vec<&str>, Vec<&str>) = (from.lines().collect(), to.lines().collect());
    // An outline line is `  - button "Go" #go`; the indent and dash are only layout.
    let item = |l: &str| { let l = l.trim_start(); l.strip_prefix("- ").unwrap_or(l).to_string() };
    let id = |l: &str| l.rsplit_once(" #").map(|(_, id)| id.to_string()).filter(|id| !id.contains([' ', '"']));
    let kind = |l: &str| l.split([' ', ':']).next().unwrap_or("").to_string();
    // Each side's lines that differ, with the diff hunk they are in.
    let (mut old, mut new) = (Vec::<(usize, Option<String>)>::new(), Vec::<(usize, Option<String>)>::new());
    for (hunk, op) in similar::capture_diff_slices(similar::Algorithm::Myers, &a, &b).iter().enumerate() {
        if matches!(op, similar::DiffOp::Equal { .. }) { continue; }
        old.extend(a[op.old_range()].iter().map(|l| (hunk, Some(item(l)))));
        new.extend(b[op.new_range()].iter().map(|l| (hunk, Some(item(l)))));
    }
    let mut d = OutlineDiff::default();
    for (oh, o) in old.iter_mut() {
        let Some(line) = o.as_ref() else { continue };
        let found = new.iter_mut().find(|(nh, n)| n.as_ref().is_some_and(|n| match id(line) {
            Some(i) => id(n).as_ref() == Some(&i),
            // Without ids, only a same-kind line in the same hunk is the same element.
            None => nh == oh && id(n).is_none() && kind(n) == kind(line),
        }));
        if let Some((_, n)) = found {
            d.changed.push(Changed { from: o.take().expect("checked"), to: n.take().expect("matched") });
        }
    }
    d.removed = old.into_iter().filter_map(|(_, l)| l).collect();
    d.added = new.into_iter().filter_map(|(_, l)| l).collect();
    d
}

/// How many outline differences `look` prints; the rest are counted.
pub(super) const DIFF_SHOWN: usize = 40;

impl OutlineDiff {
    pub(super) fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    /// `changed`, `removed` and `added` lines, at most `DIFF_SHOWN` of them.
    pub(super) fn lines(&self) -> Vec<String> {
        let all: Vec<String> = self.changed.iter().map(|c| format!("changed  {} → {}", c.from, c.to))
            .chain(self.removed.iter().map(|l| format!("removed  {l}")))
            .chain(self.added.iter().map(|l| format!("added    {l}")))
            .collect();
        let more = all.len().saturating_sub(DIFF_SHOWN);
        let mut out: Vec<String> = all.into_iter().take(DIFF_SHOWN).collect();
        if more > 0 { out.push(format!("…        {more} more; --json has them all")); }
        out
    }
}

/// Every `error` and `!` line for one width: what the page threw, questions
/// that did not place, and what the light and dark renders measured.
pub(super) fn measured_problems(light: &Option<Screen>, dark: &Option<Screen>) -> Vec<String> {
    light.as_ref().map(screen_problems).unwrap_or_default().into_iter()
        .chain(problem_lines(light.as_ref(), dark.as_ref()).into_iter().map(|p| format!("!        {p}")))
        .collect()
}

pub(super) const DESKTOP: (u32, u32) = (1280, 900);
pub(super) const PHONE: (u32, u32) = (390, 844);


/// The jelly at desktop and phone width in one call. The desktop
/// outline, what differs on a phone, the measured problems at both widths, and
/// one screenshot with both side by side. Four pages render in three browsers
/// at once (the two light pages share the screenshot), so it takes about as
/// long as one render.
pub(super) async fn look_both(app: &Shared, port: u16, name: &str, json: bool) -> Result<Response, ApiError> {
    let (j, home) = {
        let store = app.store.lock().expect("store lock");
        (store.get(name)?.clone(), store.home().to_path_buf())
    };
    let label = |(w, h): (u32, u32), what: &str| format!("{what} {w}×{h}");
    // Desktop and phone in light mode, then each in dark mode.
    let probes = Probes(app, vec![
        app.start_probe(name, DESKTOP, &label(DESKTOP, "desktop"), false, None)?,
        app.start_probe(name, PHONE, &label(PHONE, "phone"), false, None)?,
        app.start_probe(name, DESKTOP, "", true, None)?,
        app.start_probe(name, PHONE, "", true, None)?,
    ]);
    let dir = home.join("renders");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{}-look.png", crate::disk::name(name)));
    // A screenshot left from an earlier look must not pass for this one.
    let _ = std::fs::remove_file(&path);
    let (light, dd, pd) = tokio::join!(
        render_probes(app, port, &probes.1[..2], Some(&path)),
        render_probes(app, port, &probes.1[2..3], None),
        render_probes(app, port, &probes.1[3..], None),
    );
    let (mut notes, mut failed): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let light_ran = light.is_ok();
    let mut got = |r: Result<Vec<Option<Screen>>, String>, n: usize, unchecked: &str| match r {
        Ok(s) => s,
        Err(e) => { failed.push(render_failed(unchecked, &e, "look")); vec![None; n] }
    };
    let mut light = got(light, 2, "light mode at desktop and phone width was not checked").into_iter();
    let (desk_sc, phone_sc) = (light.next().flatten(), light.next().flatten());
    let desk_dark_sc = got(dd, 1, "dark mode at desktop width was not checked").pop().flatten();
    let phone_dark_sc = got(pd, 1, "dark mode at phone width was not checked").pop().flatten();
    if light_ran && desk_sc.is_none() { notes.push("The page sent no snapshot at desktop width.".into()); }
    if light_ran && phone_sc.is_none() { notes.push("The page sent no snapshot at phone width.".into()); }
    let png = path.exists().then_some(path);

    let mut out = summary(app, &j);
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    if let Some(p) = &png { line(&mut out, format!("png      {} · desktop on the left, phone on the right", p.display())); }
    let problems_from = out.len();
    let desk_problems = measured_problems(&desk_sc, &desk_dark_sc);
    // Only the problems desktop does not have, so nothing is said twice.
    let phone_problems: Vec<String> = measured_problems(&phone_sc, &phone_dark_sc).into_iter()
        .filter(|p| !desk_problems.contains(p))
        .map(|p| match p.split_once(' ') { Some((tag, rest)) => format!("{tag:<8} at phone width: {}", rest.trim_start()), None => p })
        .collect();
    for p in desk_problems.iter().chain(&phone_problems) { line(&mut out, p.clone()); }
    for n in notes { line(&mut out, format!("!        {n}")); }
    for f in failed { line(&mut out, f); }
    let diff = match (&desk_sc, &phone_sc) {
        (Some(d), Some(p)) => Some(outline_diff(&d.text, &p.text)),
        _ => None,
    };
    if json {
        return Ok(Json(json!({
            "jelly": j.name, "version": j.version,
            "summary": &out[..problems_from],
            "problems": out[problems_from..].lines().collect::<Vec<_>>(),
            "png": png,
            "outline": desk_sc.map(|s| s.text),
            "phone": diff.map(|d| json!({"width": PHONE.0, "height": PHONE.1, "added": d.added, "removed": d.removed, "changed": d.changed})),
        })).into_response());
    }
    if let Some(s) = &desk_sc {
        line(&mut out, format!("\non screen at {}×{} (v{}):", s.width, s.height, s.version));
        out.push_str(&s.text);
        out.push('\n');
    }
    if let Some(d) = diff {
        line(&mut out, format!("\nat phone width {}×{}, compared with desktop:", PHONE.0, PHONE.1));
        if d.is_empty() { line(&mut out, "same     the outline is the same as on desktop".into()); }
        for l in d.lines() { line(&mut out, l); }
    }
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], out).into_response())
}

// ---- jelly look --compare ------------------------------------------------------

/// `v12` names version 12.
pub(super) fn version_of(s: &str) -> Option<u64> {
    s.strip_prefix('v').filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).and_then(|n| n.parse().ok())
}

/// Where `look --save LABEL` keeps a jelly's view and state.
pub(super) fn saved_path(app: &App, name: &str, label: &str) -> Result<std::path::PathBuf, ApiError> {
    let ok = (1..=40).contains(&label.len()) && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !ok || version_of(label).is_some() {
        return Err(Error::Invalid(format!("{label:?} cannot name a saved version: use letters, digits, - and _ (v and a number is a version)")).into());
    }
    let home = app.store.lock().expect("store lock").home().to_path_buf();
    Ok(home.join("renders").join("saved").join(crate::disk::name(name)).join(format!("{label}.json")))
}

/// Keeps the jelly's view and state as they are now under `label`, outside the
/// jelly: nothing about the jelly changes.
pub(super) fn save(app: &App, name: &str, label: &str) -> Result<(), ApiError> {
    let path = saved_path(app, name, label)?;
    let past = {
        let store = app.store.lock().expect("store lock");
        let j = store.get(name)?;
        Past { view: j.view.clone(), state: j.state.clone(), version: j.version }
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().expect("saved_path has a folder"))?;
        std::fs::write(&path, serde_json::to_vec(&past).expect("a past version serializes"))
    };
    write().map_err(|e| ApiError::Internal(format!("could not save {name} as {label}: {e}")))
}

/// What `--compare` means by `against`, with how to name it: an earlier
/// version rebuilt from history, or one saved with `--save`.
pub(super) fn past(app: &App, name: &str, against: &str) -> Result<(String, Past), ApiError> {
    if let Some(v) = version_of(against) {
        return Ok((format!("v{v}"), app.store.lock().expect("store lock").at_version(name, v)?));
    }
    let path = saved_path(app, name, against)?;
    let bytes = std::fs::read(&path).map_err(|_| Error::Invalid(format!("nothing is saved as {against} for {name}; save it with: jelly look {name} --save {against}")))?;
    let past: Past = serde_json::from_slice(&bytes).map_err(|e| ApiError::Internal(format!("{}: {e}", path.display())))?;
    Ok((format!("{against} (v{})", past.version), past))
}

/// `jelly look --compare`: what changed since an earlier version. Both versions
/// render at once, side by side in one screenshot, each also in dark mode, and
/// the answer is the outline difference and the problems that are new or fixed.
pub(super) async fn compare(app: &Shared, port: u16, name: &str, (against, before): (String, Past), q: &LookQ) -> Result<Response, ApiError> {
    let (j, home) = {
        let store = app.store.lock().expect("store lock");
        (store.get(name)?.clone(), store.home().to_path_buf())
    };
    let width = q.width.unwrap_or(DESKTOP.0);
    let size = (width, q.height.unwrap_or(if width <= 500 { PHONE.1 } else { DESKTOP.1 }));
    let now = format!("now v{}", j.version);
    // Before and now in light mode, then each in dark mode.
    let probes = Probes(app, vec![
        app.start_probe(name, size, &against, false, Some(&before))?,
        app.start_probe(name, size, &now, false, None)?,
        app.start_probe(name, size, "", true, Some(&before))?,
        app.start_probe(name, size, "", true, None)?,
    ]);
    let dir = home.join("renders");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{}-compare.png", crate::disk::name(name)));
    let _ = std::fs::remove_file(&path);
    let (light, bd, nd) = tokio::join!(
        render_probes(app, port, &probes.1[..2], Some(&path)),
        render_probes(app, port, &probes.1[2..3], None),
        render_probes(app, port, &probes.1[3..], None),
    );
    let (mut notes, mut failed): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let light_ran = light.is_ok();
    let mut got = |r: Result<Vec<Option<Screen>>, String>, n: usize, unchecked: &str| match r {
        Ok(s) => s,
        Err(e) => { failed.push(render_failed(unchecked, &e, "look")); vec![None; n] }
    };
    let mut light = got(light, 2, &format!("light mode of {against} and now was not checked")).into_iter();
    let (before_sc, now_sc) = (light.next().flatten(), light.next().flatten());
    let before_dark = got(bd, 1, &format!("dark mode of {against} was not checked")).pop().flatten();
    let now_dark = got(nd, 1, "dark mode now was not checked").pop().flatten();
    if light_ran && before_sc.is_none() { notes.push(format!("{against} sent no snapshot.")); }
    if light_ran && now_sc.is_none() { notes.push("The page sent no snapshot.".into()); }
    let png = path.exists().then_some(path);

    let was = measured_problems(&before_sc, &before_dark);
    let is = measured_problems(&now_sc, &now_dark);
    // A side that was not checked has no problems to tell apart: nothing is new or fixed.
    let checked = before_sc.is_some() && now_sc.is_some() && before_dark.is_some() && now_dark.is_some();
    let new: Vec<&String> = if checked { is.iter().filter(|p| !was.contains(p)).collect() } else { Vec::new() };
    let fixed: Vec<&String> = if checked { was.iter().filter(|p| !is.contains(p)).collect() } else { Vec::new() };
    let diff = match (&before_sc, &now_sc) {
        (Some(b), Some(n)) => Some(outline_diff(&b.text, &n.text)),
        _ => None,
    };

    let mut out = summary(app, &j);
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    if let Some(p) = &png { line(&mut out, format!("png      {} · {against} on the left, {now} on the right", p.display())); }
    let problems_from = out.len();
    for p in &is {
        // New problems say so, so the agent sees what its change broke.
        let p = if new.contains(&p) { p.split_once(' ').map(|(tag, rest)| format!("{tag:<8} new: {}", rest.trim_start())).unwrap_or(p.clone()) } else { p.clone() };
        line(&mut out, p);
    }
    for n in notes { line(&mut out, format!("!        {n}")); }
    for f in failed { line(&mut out, f); }
    if q.json {
        let diff = diff.unwrap_or_default();
        return Ok(Json(json!({
            "jelly": j.name, "version": j.version, "against": against, "against_version": before.version,
            "summary": &out[..problems_from],
            "problems": out[problems_from..].lines().collect::<Vec<_>>(),
            // null, not [], when a side was not checked.
            "new_problems": checked.then_some(new), "fixed_problems": checked.then_some(fixed),
            "png": png,
            "added": diff.added, "removed": diff.removed, "changed": diff.changed,
        })).into_response());
    }
    line(&mut out, format!("\ncompared with {against}:"));
    if let Some(d) = diff {
        if d.is_empty() { line(&mut out, "same     the outline did not change".into()); }
        for l in d.lines() { line(&mut out, l); }
    }
    // `fixed  unreadable text …`, without the `!`; an error keeps its word.
    for f in fixed {
        line(&mut out, match f.split_once(' ') {
            Some(("!", rest)) => format!("fixed    {}", rest.trim_start()),
            Some((tag, rest)) => format!("fixed    {tag}: {}", rest.trim_start()),
            None => format!("fixed    {f}"),
        });
    }
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], out).into_response())
}

// ---- jelly try ---------------------------------------------------------------

/// One thing `jelly try` does in the page, as a person would.
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "do", rename_all = "lowercase")]
pub(super) enum TryStep {
    Click { at: String },
    /// Replaces the field's value, like a person selecting it all and typing.
    Type { at: String, text: String },
    Press { at: String, key: String },
}

impl std::fmt::Display for TryStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Click { at } => write!(f, "click {at}"),
            Self::Type { at, text } => write!(f, "type {at} {text:?}"),
            Self::Press { at, key } => write!(f, "press {at} {key}"),
        }
    }
}

/// A `jelly try` render in progress. The page's verb calls run against `state`,
/// which starts as the jelly's and is thrown away at the end: nothing is saved,
/// logged or undoable.
pub(super) struct Trial {
    pub(super) name: String,
    pub(super) verbs: String,
    pub(super) state: Value,
    pub(super) version: u64,
    pub(super) steps: Vec<TryStep>,
    pub(super) calls: Vec<TrialCall>,
    pub(super) result: Option<TrialResult>,
}

/// One verb call the page made under `jelly try`.
#[derive(Serialize)]
pub(super) struct TrialCall {
    pub(super) verb: String,
    pub(super) args: Value,
    /// JSON pointers into the state that the call touched.
    pub(super) changed: Vec<String>,
    /// Why the verb refused, when it did; then nothing changed.
    pub(super) refused: Option<String>,
    /// What `jelly try` prints for it.
    #[serde(skip)]
    pub(super) line: String,
}

/// What the page reports once its steps ran.
#[derive(Deserialize)]
pub(super) struct TrialResult {
    pub(super) text: String,
    pub(super) width: u32,
    pub(super) height: u32,
    #[serde(default)]
    pub(super) errors: Vec<String>,
    /// Dialogs the page opened during the steps: `alert "a tea needs a name"`.
    #[serde(default)]
    pub(super) dialogs: Vec<String>,
    /// The step that could not be done and why; the steps after it did not run.
    #[serde(default)]
    pub(super) failed: Option<TrialFailure>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct TrialFailure {
    pub(super) step: usize,
    pub(super) error: String,
}

#[derive(Deserialize)]
pub(super) struct TryReq {
    pub(super) steps: Vec<TryStep>,
    pub(super) width: Option<u32>,
    #[serde(default)]
    pub(super) png: bool,
    /// Answer with JSON instead of text.
    #[serde(default)]
    pub(super) json: bool,
}

impl App {
    pub(super) fn start_trial(&self, name: &str, steps: &[TryStep]) -> Result<String, ApiError> {
        let store = self.store.lock().expect("store lock");
        let j = store.get(name)?;
        let token = random_token()?;
        let trial = Trial { name: j.name.clone(), verbs: j.verbs.clone(), state: j.state.clone(), version: j.version, steps: steps.to_vec(), calls: Vec::new(), result: None };
        self.trials.lock().expect("trials lock").insert(token.clone(), trial);
        Ok(token)
    }
}

pub(super) fn random_token() -> Result<String, ApiError> {
    use base64::Engine as _;
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|e| ApiError::Internal(format!("could not issue a token: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// `jelly try`: renders the jelly, does the steps in it as a person would, and
/// says what happened. The page's verb calls go to `trial_call`, so the jelly
/// itself is never changed.
pub(super) async fn try_steps(State(app): State<Shared>, Path(name): Path<String>, req_headers: axum::http::HeaderMap, Json(r): Json<TryReq>) -> Result<Response, ApiError> {
    let port = port_of(&req_headers);
    let width = r.width.unwrap_or(1280);
    let height = if width <= 500 { 844 } else { 900 };
    let (version, home) = {
        let store = app.store.lock().expect("store lock");
        (store.get(&name)?.version, store.home().to_path_buf())
    };
    let (mut notes, mut failed): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let token = app.start_trial(&name, &r.steps)?;
    // The screenshot is taken in the same render, once the steps have run.
    let shot = r.png.then(|| {
        let dir = home.join("renders");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{}-try-{width}.png", crate::disk::name(&name)))
    });
    let mut png = None;
    match render(port, &name, Some(&token), width, height, shot.as_deref(), false, RENDER_LIMIT).await {
        Ok(()) => {
            png = shot;
            for _ in 0..30 {
                if app.trials.lock().expect("trials lock").get(&token).is_some_and(|t| t.result.is_some()) { break; }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        Err(e) => failed.push(render_failed("the steps were not tried", &e, "try")),
    }
    let trial = app.trials.lock().expect("trials lock").remove(&token).expect("only this handler removes its trial");

    let mut out = String::new();
    let line = |out: &mut String, s: String| { out.push_str(&s); out.push('\n'); };
    line(&mut out, format!("{} · tried {} step{} on v{version} · nothing was saved", trial.name, trial.steps.len(), if trial.steps.len() == 1 { "" } else { "s" }));
    for (i, s) in trial.steps.iter().enumerate() { line(&mut out, format!("step     {} {s}", i + 1)); }
    if trial.calls.is_empty() { line(&mut out, "call     none: no verb ran".into()); }
    for c in &trial.calls { line(&mut out, c.line.clone()); }
    if let Some(res) = &trial.result {
        for e in &res.errors { line(&mut out, format!("error    {e}")); }
        for d in &res.dialogs { line(&mut out, format!("dialog   {d}")); }
        if let Some(f) = &res.failed {
            let step = trial.steps.get(f.step).map(|s| s.to_string()).unwrap_or_default();
            line(&mut out, format!("!        step {} ({step}) failed: {}; the steps after it did not run", f.step + 1, f.error));
        }
    } else if failed.is_empty() {
        notes.push("The page rendered but did not finish the steps.".into());
    }
    if r.json {
        let res = trial.result.as_ref();
        return Ok(Json(json!({
            "jelly": trial.name, "version": version, "steps": trial.steps, "calls": trial.calls,
            "errors": res.map(|r| &r.errors[..]).unwrap_or_default(),
            "dialogs": res.map(|r| &r.dialogs[..]).unwrap_or_default(),
            // `step` counts from 0 here: it indexes `steps`.
            "failed": res.and_then(|r| r.failed.as_ref()),
            "problems": notes.iter().cloned().chain(failed.iter().cloned()).collect::<Vec<_>>(), "png": png, "state": trial.state,
            "outline": res.map(|r| &r.text),
        })).into_response());
    }
    for n in notes { line(&mut out, format!("!        {n}")); }
    for f in failed { line(&mut out, f); }
    if let Some(p) = &png { line(&mut out, format!("png      {}", p.display())); }
    line(&mut out, format!("state    {}", state_shape(&trial.state)));
    if let Some(res) = trial.result {
        line(&mut out, format!("\non screen at {}×{} after the steps:", res.width, res.height));
        out.push_str(&res.text);
        out.push('\n');
    }
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], out).into_response())
}

pub(super) async fn put_trial_result(State(app): State<Shared>, Path(token): Path<String>, Json(res): Json<TrialResult>) -> Result<StatusCode, ApiError> {
    let mut trials = app.trials.lock().expect("trials lock");
    let trial = trials.get_mut(&token).ok_or_else(trial_over)?;
    trial.result = Some(res);
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn trial_over() -> ApiError {
    Error::NotFound("this jelly try is over".into()).into()
}

/// A verb call from a page under `jelly try`: runs against the trial's state and
/// answers like a real call, but saves nothing and records no event.
pub(super) async fn trial_call(State(app): State<Shared>, Path(token): Path<String>, Json(r): Json<CallReq>) -> Result<Json<Value>, ApiError> {
    let mut trials = app.trials.lock().expect("trials lock");
    let trial = trials.get_mut(&token).ok_or_else(trial_over)?;
    let args = clip(&r.args.to_string(), 120);
    // A real page's calls come from the person (named, if the page set a name),
    // so the verbs see the same. Only person identities are accepted here.
    let by = r.by.filter(|b| crate::store::is_person(b)).unwrap_or_else(|| "person".into());
    let next = match crate::verbs::run(&trial.verbs, &r.verb, &trial.state, &r.args, &by, now_ms()) {
        Ok(next) => next,
        Err(e) => {
            trial.calls.push(TrialCall { line: format!("refused  {} {args}: {e}", r.verb), verb: r.verb, args: r.args, changed: Vec::new(), refused: Some(e.clone()) });
            return Err(Error::Invalid(e).into());
        }
    };
    let mut changed = Vec::new();
    crate::store::diff(&trial.state, &next, &mut String::new(), &mut changed);
    let what = if changed.is_empty() { "changed nothing".to_string() } else {
        let shown: Vec<String> = changed.iter().take(5).map(|p| match next.pointer(p) {
            Some(v) => format!("{p} = {}", clip(&v.to_string(), 60)),
            None => format!("{p} removed"),
        }).collect();
        let more = if changed.len() > 5 { format!(" · {} more", changed.len() - 5) } else { String::new() };
        shown.join(" · ") + &more
    };
    trial.calls.push(TrialCall { line: format!("call     {} {args} → {what}", r.verb), verb: r.verb.clone(), args: r.args.clone(), changed: changed.clone(), refused: None });
    trial.state = next;
    trial.version += 1;
    Ok(Json(json!({"seq": 0, "at_ms": now_ms(), "jelly": trial.name, "kind": "do", "by": by, "verb": r.verb, "args": r.args,
        "changed": changed, "version": trial.version, "state": trial.state})))
}

pub(super) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s.to_string() }
}

pub(crate) fn ago(ms: u64) -> String {
    let s = now_ms().saturating_sub(ms) / 1000;
    match s { 0..=59 => "just now".into(), 60..=3599 => format!("{}m ago", s / 60), 3600..=86399 => format!("{}h ago", s / 3600), _ => format!("{}d ago", s / 86400) }
}

/// `pick({id, value})`: the verb's name and its second parameter, which holds
/// the args (destructured keys, or the parameter's name).
pub(super) fn verb_sig(src: &str, name: &str) -> String {
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
pub(super) fn state_shape(v: &Value) -> String {
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

#[derive(Deserialize)]
pub(super) struct FrameQ {
    pub(super) w: u32,
    pub(super) h: u32,
    #[serde(rename = "try")]
    pub(super) trial: Option<String>,
    /// `jelly look`'s dark render.
    pub(super) dark: Option<String>,
}

pub(super) async fn look_frame(Path(name): Path<String>, Query(q): Query<FrameQ>) -> Result<Html<String>, ApiError> {
    // The name goes into the page, so only a valid jelly name may (#66).
    crate::store::valid_name(&name)?;
    // Scaled up to fill the 500px window, so the screenshot is the phone layout and nothing else.
    let k = 500.0 / q.w.max(1) as f64;
    // Only a URL-safe token passes through, so it cannot break out of the attribute.
    let trial = q.trial.filter(|t| t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).map(|t| format!("&try={t}")).unwrap_or_default()
        + if q.dark.is_some() { "&dark=1" } else { "" };
    Ok(Html(format!("<!doctype html><body style=\"margin:0;overflow:hidden\"><iframe src=\"/j/{name}/?agent=1{trial}\" style=\"display:block;border:0;width:{w}px;height:{h}px;background:#fff;transform:scale({k});transform-origin:0 0\"></iframe></body>", w = q.w, h = q.h)))
}

#[cfg(test)]
mod problem_tests {
    use super::*;

    fn measured(contrast: Value, problems: Value) -> Screen {
        serde_json::from_value(json!({"owner": "test", "text": "", "width": 1280, "height": 900, "version": 1, "contrast": contrast, "problems": problems})).unwrap()
    }

    /// Unreadable text is one line per color pair, worst first, and says which
    /// scheme it is in; every kind shows a few items and counts the rest.
    #[test]
    fn problems_print_a_few_lines_per_kind() {
        let text = |at: &str, fg: &str, ratio: f64| json!({"at": at, "text": "t", "fg": fg, "bg": "#ffffff", "ratio": ratio});
        let light = measured(json!([text("#a", "#cccccc", 1.6), text("#b", "#cccccc", 1.6), text("#c", "#999999", 2.8), text("#d", "#eeeeee", 1.1)]),
            json!([{"kind": "image without alt", "items": ["#1", "#2", "#3", "#4", "#5"]}]));
        let dark = measured(json!([text("#d", "#eeeeee", 1.1), text("#e", "#ffffff", 1.0)]), json!([]));
        assert_eq!(problem_lines(Some(&light), Some(&dark)), [
            r##"unreadable text: #eeeeee on #ffffff is 1.1:1 at #d "t""##,
            r##"unreadable text in light mode: #cccccc on #ffffff is 1.6:1 at #a "t" and 1 more like it"##,
            r##"unreadable text in light mode: #999999 on #ffffff is 2.8:1 at #c "t""##,
            r##"unreadable text in dark mode: #ffffff on #ffffff is 1.0:1 at #e "t""##,
            "image without alt: #1", "image without alt: #2", "image without alt: #3", "image without alt: 2 more",
        ]);
        // Without a dark render, nothing is said about either scheme.
        assert!(problem_lines(Some(&light), None).iter().all(|l| !l.contains(" mode")));
    }

    /// A line replaced by one of the same kind is a change; a button replaced by
    /// a heading is one removed and one added. Ids pair lines first.
    #[test]
    fn outline_diff_pairs_changes_by_id_then_kind() {
        let d = outline_diff("- heading \"A\" [h1]\n  - text \"3 items\" #n\n  - button \"Go\"\n- link \"x\"",
            "- heading \"A\" [h1]\n  - text \"4 items\" #n\n  - heading \"Gone\" [h2]\n- link \"x\"\n- img \"new\"");
        assert_eq!(serde_json::to_value(&d).unwrap(), json!({
            "added": ["heading \"Gone\" [h2]", "img \"new\""],
            "removed": ["button \"Go\""],
            "changed": [{"from": "text \"3 items\" #n", "to": "text \"4 items\" #n"}],
        }));
        // A line with an id pairs with its id, not with whatever took its place.
        let d = outline_diff("- text \"3 items\" #count", "- text \"Faint\" #faint\n- text \"4 items\" #count");
        assert_eq!(serde_json::to_value(&d).unwrap(), json!({
            "added": ["text \"Faint\" #faint"], "removed": [],
            "changed": [{"from": "text \"3 items\" #count", "to": "text \"4 items\" #count"}],
        }));
    }
}
