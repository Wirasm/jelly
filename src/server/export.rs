//! `jelly share` without flags: one read-only HTML file with state and assets baked in.

use super::*;

/// One file you can send anyone: the view with its state baked in, assets
/// inlined, and a read-only `jelly` in place of the live runtime.
pub(super) async fn share(State(app): State<Shared>, Path(name): Path<String>) -> Result<Html<String>, ApiError> {
    use base64::Engine as _;
    use lol_html::html_content::UserData as _;
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
    let mut marker_bytes = [0; 16];
    getrandom::fill(&mut marker_bytes).map_err(|e| ApiError::Internal(format!("could not prepare shared roots: {e}")))?;
    let marker = format!("jelly-snapshot-{}", marker_bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
    let boot = json!({"name": j.name, "about": j.about, "state": j.state, "asks": j.asks, "assets": assets, "rootMarker": marker})
        .to_string().replace("</", "<\\/");
    let inject = format!("<script>window.__JELLY__={boot};</script><script>{SNAPSHOT_JS}</script>");
    let mut base_href = None;
    lol_html::rewrite_str(&j.view, lol_html::RewriteStrSettings {
        element_content_handlers: vec![
            lol_html::element!("template base", |element| { element.set_user_data(true); Ok(()) }),
            lol_html::element!("base[href]", |element| {
                if base_href.is_none() && element.namespace_uri() == "http://www.w3.org/1999/xhtml"
                    && element.user_data().downcast_ref::<bool>() != Some(&true) {
                    base_href = element.get_attribute("href");
                }
                Ok(())
            }),
        ],
        ..Default::default()
    }).map_err(|e| ApiError::Internal(format!("could not read shared view base: {e}")))?;
    let html = lol_html::rewrite_str(&j.view, lol_html::RewriteStrSettings {
        element_content_handlers: vec![
            lol_html::element!("template object", |element| {
                element.set_user_data(true);
                Ok(())
            }),
            lol_html::element!("object[data]", |element| {
                let value = element.get_attribute("data").expect("matched data attribute");
                if let Some(uri) = shared_static_asset(&value, &assets, base_href.as_deref()) {
                    // Only the browser knows whether a declarative template becomes a root.
                    // Its existing cleanup restores this attribute if the template stays inert.
                    if element.user_data().downcast_ref::<bool>() == Some(&true) {
                        element.set_attribute(&format!("data-{marker}-object"), &value)?;
                    }
                    // Objects can commit their missing-file fallback before view scripts run.
                    element.set_attribute("data", &uri)?;
                }
                Ok(())
            }),
            lol_html::element!("template", |element| {
                if element.get_attribute("shadowrootmode").is_some_and(|mode| mode.eq_ignore_ascii_case("open") || mode.eq_ignore_ascii_case("closed")) {
                    element.prepend(&format!("<{marker} hidden></{marker}>"), lol_html::html_content::ContentType::Html);
                }
                element.after(&format!("<script data-{marker}>window.__JELLY__.finishTemplate()</script>"), lol_html::html_content::ContentType::Html);
                Ok(())
            }),
        ],
        ..Default::default()
    }).map_err(|e| ApiError::Internal(format!("could not prepare shared view: {e}")))?;
    Ok(Html(inject_head(&html, &inject)))
}

pub(super) fn shared_static_asset(value: &str, assets: &[(String, String)], base_href: Option<&str>) -> Option<String> {
    let mut embedded = None;
    // The output directory is unknown. Requiring the same asset relative to two
    // disjoint directories excludes absolute paths and traversal that escapes
    // the document directory then happens to return to a synthetic location.
    let locations = if cfg!(windows) {
        ["file:///C:/jelly-shared/", "file:///C:/elsewhere/output/"]
    } else {
        ["file:///jelly-shared/", "file:///elsewhere/output/"]
    };
    for location in locations {
        let document = url::Url::parse(location).expect("valid view URL");
        let base = base_href.and_then(|href| document.join(href).ok())
            .filter(|url| !matches!(url.scheme(), "data" | "javascript"))
            .unwrap_or_else(|| document.clone());
        let directory = document.join("assets/").ok()?.to_file_path().ok()?;
        let url = base.join(value).ok()?;
        let path = url.to_file_path().ok()?;
        let file = path.strip_prefix(directory).ok()?.to_str()?;
        let (_, uri) = assets.iter().find(|(name, _)| name == file)?;
        let uri = format!("{uri}{}", url.fragment().map(|hash| format!("#{hash}")).unwrap_or_default());
        if embedded.as_ref().is_some_and(|prior| prior != &uri) { return None; }
        embedded = Some(uri);
    }
    embedded
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::store::PERSON;
    use base64::Engine as _;

    fn fixture(view: &str) -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        store.make("test:share", view.into(), "const verbs = {};".into(),
            json!({"picture": "assets/dot.png", "file": "dot.png", "textFile": "hello.txt", "label": "Label: assets/dot.png", "closing": "</script>"}),
            "agent", false, "Shared </script> jelly", "").unwrap();
        for (anchor, title) in [("#pictures", "open"), ("#pictures", "answered"), ("#other", "unasked"), ("#other", "other")] {
            store.ask("test:share", json!({"seen": true, "title": title, "anchor": {"selector": anchor}}), "agent").unwrap();
        }
        store.answer("test:share", "a2", None, "captured answer", PERSON).unwrap();
        store.unask("test:share", "a3", "agent").unwrap();
        let (events, _) = broadcast::channel(16);
        (dir, Arc::new(App { store: Mutex::new(store), events, screens: Mutex::default(), trials: Mutex::default(), probes: Mutex::default(), shares: Mutex::default(), sharing: Default::default(), open: Mutex::default() }))
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
        assert_eq!(boot["asks"], expected["asks"]);
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
    async fn share_embeds_parsed_objects_before_browser_loads() {
        let inert = "<object data='assets/green.svg'></object>";
        let view = format!("<head></head><object data='assets/green.svg'></object><template>{inert}</template><script>const markup = \"{inert}\";</script>");
        let (_dir, app) = fixture(&view);
        app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
        let html = exported(app).await;
        assert!(html.contains("<object data=\"data:image/svg+xml;base64,c3Zn\"></object>"));
        let marker = boot(&html)["rootMarker"].as_str().unwrap().to_owned();
        assert!(html.contains(&format!("data-{marker}-object=\"assets/green.svg\"")));
        assert!(html.contains(&format!("const markup = \"{inert}\";")));
    }

    #[tokio::test]
    async fn share_static_objects_follow_the_document_base() {
        for base in ["<base href='https://example.com/'>", "<base href='../'>"] {
            for placement in [true, false] {
                let object = "<object data='assets/green.svg'></object>";
                let view = if placement { format!("<head>{base}</head>{object}") } else { format!("<head></head>{object}{base}") };
                let (_dir, app) = fixture(&view);
                app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
                assert!(exported(app).await.contains(object));
            }
        }
        for path in ["assets/%67reen.svg", r"assets\%67reen.svg", r"assets\t/../%67reen.svg", "././assets/%67reen.svg", "assets/sub/../%67reen.svg?query#part"] {
            let (_dir, app) = fixture(&format!("<head><template><base href='https://example.com/'></template><base href='./'></head><object data='{path}'></object>"));
            app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
            let html = exported(app).await;
            let fragment = if path.ends_with("#part") { "#part" } else { "" };
            assert!(html.contains(&format!("<object data=\"data:image/svg+xml;base64,c3Zn{fragment}\"></object>")), "{path}");
        }
    }

    #[tokio::test]
    async fn share_static_objects_use_only_html_document_bases() {
        let object = "<object data='assets/green.svg'></object>";
        for base in [
            "<svg><base href='https://example.com/'/></svg>",
            "<math><base href='https://example.com/'/></math>",
            "<base href='data:text/html,ignored'>",
            "<base href='javascript:void(0)'>",
            "<base href='data:text/html,ignored'><base href='https://example.com/'>",
            "<base href='javascript:void(0)'><base href='https://example.com/'>",
            "<base href=''><base href='https://example.com/'>",
            "<base href='  '>",
            "<template><base href='https://example.com/'></template>",
            "<div><template shadowrootmode='open'><base href='https://example.com/'></template></div>",
            "<noscript><base href='https://example.com/'></noscript>",
            "<!-- <base href='https://example.com/'> -->",
            "<script>const html = `<base href='https://example.com/'>`;</script>",
        ] {
            let (_dir, app) = fixture(&format!("<head></head>{base}{object}"));
            app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
            assert!(exported(app).await.contains("<object data=\"data:image/svg+xml;base64,c3Zn\"></object>"), "{base}");
        }
        for base in [
            "<svg><foreignObject><base href='https://example.com/'></foreignObject></svg>",
            "<math><mtext><base href='https://example.com/'></mtext></math>",
        ] {
            let (_dir, app) = fixture(&format!("<head></head>{base}{object}"));
            app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
            assert!(exported(app).await.contains(object), "{base}");
        }
    }

    #[tokio::test]
    async fn share_static_objects_do_not_depend_on_the_export_location() {
        for (base, path) in [
            ("/jelly-shared/", "assets/green.svg"),
            ("../jelly-shared/", "assets/green.svg"),
            ("file:///jelly-shared/", "assets/green.svg"),
            ("/elsewhere/output/", "assets/green.svg"),
            ("../output/", "assets/green.svg"),
            ("./", "/jelly-shared/assets/green.svg"),
            ("./", "../jelly-shared/assets/green.svg"),
        ] {
            let object = format!("<object data='{path}'></object>");
            let (_dir, app) = fixture(&format!("<head><base href='{base}'></head>{object}"));
            app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
            assert!(exported(app).await.contains(&object), "base {base}, data {path}");
        }
        let (_dir, app) = fixture("<head><base href='nested/'></head><object data='../assets/green.svg'></object>");
        app.store.lock().unwrap().put_asset("test:share", "green.svg", b"svg", "agent").unwrap();
        assert!(exported(app).await.contains("<object data=\"data:image/svg+xml;base64,c3Zn\"></object>"));
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
        let view = include_str!("../../tests/fixtures/share/view.html")
            .replace("__GC_ROOTS__", &["open", "closed"].into_iter().map(|mode| {
                format!("<div class=discard-root><template shadowrootmode={mode}><share-gc-probe></share-gc-probe></template></div>").repeat(25)
            }).collect::<String>())
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
            for file in ["link-unknown", "document-default"] {
                let css = format!("#encoding-{file}.café {{ background-image: url(dot.png); }} #encoding-{file}.café::after {{ content: 'café'; }}");
                store.put_asset("test:share", &format!("{file}.css"), css.as_bytes(), "agent").unwrap();
            }
            let link_selected = b"#encoding-link-selected.caf\xe9 { background-image: url(dot.png); } #encoding-link-selected.caf\xe9::after { content: 'caf\xe9'; }";
            store.put_asset("test:share", "link-selected.css", link_selected, "agent").unwrap();
            let collision = String::from_utf8_lossy(link_selected);
            store.put_asset("test:share", "link-collision.css", collision.as_bytes(), "agent").unwrap();
            store.put_asset("test:share", "link-imported.css", b"@import 'link-child.css';", "agent").unwrap();
            store.put_asset("test:share", "link-child.css", b"#encoding-link-imported.caf\xe9 { background-image: url(dot.png); } #encoding-link-imported.caf\xe9::after { content: 'caf\xe9'; }", "agent").unwrap();
            store.put_asset("test:share", "inherited.css", b"@charset \"windows-1252\"; @import 'child.css';", "agent").unwrap();
            store.put_asset("test:share", "child.css", b"#encoding-inherited.caf\xe9 { background-image: url(dot.png); } #encoding-inherited.caf\xe9::after { content: 'caf\xe9'; }", "agent").unwrap();
        }
        let html = exported(app).await;
        let path = dir.path().join("shared.html");
        std::fs::write(&path, html).unwrap();
        // Chrome's updater can inherit stderr and hold a pipe open after Chrome exits.
        let log = dir.path().join("browser.log");
        let stderr = std::fs::File::create(&log).unwrap();
        let mut browser = tokio::process::Command::new(browser)
            .args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--enable-automation", "--disable-background-timer-throttling", "--disable-component-update", "--js-flags=--expose-gc"])
            .arg(format!("--user-data-dir={}", dir.path().join("chrome").display()))
            .arg(format!("file://{}", path.display()))
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .kill_on_drop(true).spawn().expect("browser could not start");
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), receive.recv()).await;
        server.abort();
        browser.kill().await.expect("browser could not stop");
        browser.wait().await.expect("browser could not exit");
        let result = result.unwrap_or_else(|_| panic!("browser did not report a result within 30 seconds: {}", std::fs::read_to_string(log).unwrap()))
            .expect("browser result channel closed");
        assert_eq!(result["complete"], true, "{result}");
        assert_eq!(result["ok"], true, "{result}");
        println!("{result}");
    }
}
