//! Headless UI tests: the real `LimeApp` is driven frame by frame with injected
//! pointer/keyboard events (no window), against a local mock of the Modrinth API.
//! Widgets register their rectangles in `ui::REGISTRY`, so tests click by label.

use super::*;
use eframe::egui::{Color32, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};
use std::sync::atomic::AtomicUsize;

use crate::testsupport::{Mock, route};

// ------------------------------------------------------------------ fixtures

fn hit(id: &str, title: &str) -> String {
    format!(
        r#"{{"project_id":"{id}","project_type":"mod","slug":"{id}","author":"someone","title":"{title}","description":"{title} description","downloads":1234,"follows":5,"icon_url":null,"versions":["1.21.1","26.3"]}}"#
    )
}

fn search_json(hits: &[(&str, &str)]) -> String {
    let items: Vec<String> = hits.iter().map(|(i, t)| hit(i, t)).collect();
    format!(r#"{{"hits":[{}],"total_hits":{}}}"#, items.join(","), hits.len())
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    hex::encode(Sha1::digest(bytes))
}

fn version_json(id: &str, project: &str, file_url: &str, filename: &str, bytes: &[u8], deps: &str) -> String {
    format!(
        r#"{{"id":"{id}","project_id":"{project}","name":"{filename}","version_number":"1.0.0","version_type":"release","game_versions":["26.3"],"loaders":["fabric"],"files":[{{"url":"{file_url}","filename":"{filename}","primary":true,"hashes":{{"sha1":"{}"}}}}],"dependencies":[{deps}]}}"#,
        sha1_hex(bytes)
    )
}

fn fabric_instance(name: &str) -> Instance {
    Instance {
        name: name.to_string(),
        game_version: "26.3".into(),
        loader: "fabric".into(),
        loader_version: "0.20.1".into(),
        version_name: Some("fabric-loader-0.20.1.json".into()),
        created: "1".into(),
        ..Default::default()
    }
}

// ------------------------------------------------------------------ harness

/// All UI tests share the global widget registry and process env: run them one at a time.
static SERIAL: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct H {
    ctx: egui::Context,
    app: LimeApp,
    root: PathBuf,
    mock: Mock,
    t: f64,
    size: egui::Vec2,
}

impl H {
    fn new(instances: Vec<Instance>) -> H {
        let root = std::env::temp_dir().join(format!(
            "lc-ui-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut st = LauncherState::default();
        st.instances = instances;
        st.ram_mb = 4096;
        instance::save_state(&root, &st).unwrap();

        let mock = Mock::start();
        // SAFETY: tests that touch the environment hold SERIAL.
        unsafe {
            std::env::set_var("LIMECRAFT_MODRINTH_API", mock.url());
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
            std::env::set_var("no_proxy", "127.0.0.1,localhost");
        }
        let ctx = egui::Context::default();
        theme::apply(&ctx, true, crate::core::prefs::DEFAULT_ACCENT, 1.0, false, true);
        let app = LimeApp::with_root(&ctx, root.clone(), None);
        let mut h = H { ctx, app, root, mock, t: 0.0, size: egui::vec2(1280.0, 800.0) };
        h.frame(vec![]);
        h.frame(vec![]);
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        ui::REGISTRY.lock().unwrap().clear();
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        self.t += 0.05;
        let app = &mut self.app;
        let _ = self.ctx.run_ui(raw, |ui| app.frame_ui(ui));
    }

    fn rects(&self, label: &str) -> Vec<Rect> {
        ui::REGISTRY.lock().unwrap().iter().filter(|(l, _)| l == label).map(|(_, r)| *r).collect()
    }

    fn has(&self, label: &str) -> bool {
        !self.rects(label).is_empty()
    }

    fn labels(&self) -> Vec<String> {
        ui::REGISTRY.lock().unwrap().iter().map(|(l, _)| l.clone()).collect()
    }

    fn click_at(&mut self, pos: Pos2) {
        let m = Modifiers::NONE;
        // hover for two frames first, like a real mouse arriving at the widget
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: m }]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: m }]);
        self.frame(vec![]);
    }

    /// Click the `nth` widget registered under `label`.
    fn click_nth(&mut self, label: &str, nth: usize) {
        let rects = self.rects(label);
        let r = *rects
            .get(nth)
            .unwrap_or_else(|| panic!("no widget #{nth} labelled {label:?}; on screen: {:?}", self.labels()));
        self.click_at(r.center());
    }

    fn click(&mut self, label: &str) {
        self.click_nth(label, 0);
    }

    fn type_text(&mut self, text: &str) {
        self.frame(vec![Event::Text(text.to_string())]);
    }

    fn press(&mut self, key: Key) {
        let ev = |pressed| Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: Modifiers::NONE };
        self.frame(vec![ev(true)]);
        self.frame(vec![ev(false)]);
    }

    /// Run frames until `cond` holds (background jobs finish) or fail after `secs`.
    fn wait(&mut self, what: &str, secs: u64, cond: impl Fn(&LimeApp) -> bool) {
        let start = std::time::Instant::now();
        while !cond(&self.app) {
            if start.elapsed().as_secs() >= secs {
                panic!("timed out waiting for: {what}; screen: {:?}; toast: {:?}", self.labels(), self.app.toast);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.frame(vec![]);
        }
        self.frame(vec![]);
    }

    fn searches(&self) -> Vec<String> {
        self.mock.requests().into_iter().filter(|r| r.starts_with("GET /search")).collect()
    }
}

impl Drop for H {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Percent-decoded request line (the launcher encodes `_` and `.` too).
fn decoded(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string()
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------------ tests

#[test]
fn catalog_opens_with_popular_results_and_install_buttons() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    h.mock.set(vec![route("/search", 200, search_json(&[("AAA", "Sodium"), ("BBB", "Lithium")]))]);

    h.click("Каталог");
    h.wait("auto search finished", 10, |a| a.mod_searched);
    assert_eq!(h.app.mod_results.len(), 2, "results should appear without pressing anything");
    assert_eq!(h.rects("Установить").len(), 2, "one install button per card; screen: {:?}", h.labels());

    // the request must carry the instance's compatibility filters
    let s: Vec<String> = h.searches().iter().map(|r| decoded(r)).collect();
    assert_eq!(s.len(), 1, "exactly one automatic search, got {s:?}");
    assert!(s[0].contains("versions:26.3"), "game version facet missing: {}", s[0]);
    assert!(s[0].contains("categories:fabric"), "loader facet missing: {}", s[0]);
    assert!(s[0].contains("project_type:mod"), "type facet missing: {}", s[0]);
}

#[test]
fn typing_a_query_and_pressing_enter_searches() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    h.mock.set(vec![route("/search", 200, search_json(&[("AAA", "Sodium")]))]);
    h.click("Каталог");
    h.wait("first search", 10, |a| a.mod_searched);

    h.click("field:mods-search");
    h.type_text("sodium");
    assert_eq!(h.app.mod_search_query, "sodium", "text must reach the search field");
    h.press(Key::Enter);
    h.wait("second search", 10, |a| a.mod_search_job.is_none());
    let s = h.searches();
    assert_eq!(s.len(), 2, "Enter must start a search: {s:?}");
    assert!(s[1].contains("query=sodium"), "{}", s[1]);
}

#[test]
fn search_button_works_too() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.mock.set(vec![route("/search", 200, search_json(&[("AAA", "Sodium")]))]);
    h.click("Каталог");
    h.wait("first search", 10, |a| a.mod_searched);
    h.click("field:mods-search");
    h.type_text("iris");
    h.click("Найти");
    h.wait("button search", 10, |a| a.mod_search_job.is_none());
    assert!(h.searches().iter().any(|s| s.contains("query=iris")), "{:?}", h.searches());
}

#[test]
fn network_error_is_shown_inline_with_retry() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    h.mock.set(vec![route("/search", 500, "{}")]);
    h.click("Каталог");
    h.wait("error", 20, |a| a.mod_search_error.is_some());
    assert!(h.has("Повторить"), "error banner with retry expected; screen: {:?}", h.labels());

    // server recovers -> Retry works
    h.mock.set(vec![route("/search", 200, search_json(&[("AAA", "Sodium")]))]);
    h.click("Повторить");
    h.wait("recovered", 10, |a| a.mod_searched && a.mod_search_error.is_none());
    assert_eq!(h.app.mod_results.len(), 1);
    assert!(!h.has("Повторить"));
}

#[test]
fn empty_result_says_nothing_found() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    h.mock.set(vec![route("/search", 200, search_json(&[]))]);
    h.click("Каталог");
    h.wait("search", 10, |a| a.mod_searched);
    assert!(h.app.mod_results.is_empty());
    assert!(h.app.mod_search_error.is_none());
}

#[test]
fn modpack_tab_searches_and_offers_install() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.mock.set(vec![route("/search", 200, search_json(&[("PPP", "Fabulously Optimized"), ("QQQ", "Cobblemon")]))]);
    h.click("Каталог");
    h.wait("catalog search", 10, |a| a.mod_searched);
    h.click("Модпаки");
    h.wait("modpack search", 10, |a| a.pack_searched);
    assert_eq!(h.app.pack_results.len(), 2);
    assert!(h.searches().iter().any(|s| decoded(s).contains("project_type:modpack")), "{:?}", h.searches());

    h.click_nth("Установить", 0);
    assert_eq!(h.app.selected_pack, Some(0));
    assert!(h.has("Скачать и установить"), "install panel expected; screen: {:?}", h.labels());
}

#[test]
fn modpack_search_by_enter() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.mock.set(vec![route("/search", 200, search_json(&[("PPP", "Fabulously Optimized")]))]);
    h.click("Каталог");
    h.wait("catalog search", 10, |a| a.mod_searched);
    h.click("Модпаки");
    h.wait("modpack search", 10, |a| a.pack_searched);
    h.click("field:packs-search");
    h.type_text("skyblock");
    h.press(Key::Enter);
    h.wait("enter search", 10, |a| a.pack_search_job.is_none());
    assert!(h.searches().iter().any(|s| s.contains("query=skyblock")), "{:?}", h.searches());
}

#[test]
fn one_click_install_downloads_the_mod_into_the_instance() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    let jar = b"PK-fake-jar-bytes".to_vec();
    let file_url = format!("{}/files/sodium.jar", h.mock.url());
    h.mock.set(vec![
        route("/search", 200, search_json(&[("AAA", "Sodium")])),
        route("/project/AAA/version", 200, format!("[{}]", version_json("v1", "AAA", &file_url, "sodium.jar", &jar, ""))),
        route("/version_files", 200, "{}"),
        route("/files/sodium.jar", 200, jar.clone()),
    ]);
    h.click("Каталог");
    h.wait("search", 10, |a| a.mod_searched);
    h.click("Установить");
    h.wait("install finished", 20, |a| a.mod_install_job.is_none() && a.mod_install_report.is_some());

    let installed = instance::instance_mines(&h.root, "26.3 fabric").join("mods/sodium.jar");
    assert_eq!(std::fs::read(&installed).expect("jar must be downloaded"), jar);
    let reqs = h.mock.requests();
    let ver = reqs.iter().find(|r| r.contains("/project/AAA/version")).map(|r| decoded(r)).expect("versions requested");
    assert!(ver.contains("game_versions") && ver.contains("26.3"), "{ver}");
    assert!(ver.contains("fabric"), "{ver}");
}

#[test]
fn install_pulls_required_dependency() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    let (jar_a, jar_b) = (b"main-mod".to_vec(), b"dependency-mod".to_vec());
    let (url_a, url_b) = (format!("{}/files/a.jar", h.mock.url()), format!("{}/files/dep.jar", h.mock.url()));
    let dep = r#"{"project_id":"DEP","version_id":null,"dependency_type":"required"}"#;
    h.mock.set(vec![
        route("/search", 200, search_json(&[("AAA", "Main Mod")])),
        route("/project/AAA/version", 200, format!("[{}]", version_json("v1", "AAA", &url_a, "a.jar", &jar_a, dep))),
        route("/project/DEP/version", 200, format!("[{}]", version_json("v2", "DEP", &url_b, "dep.jar", &jar_b, ""))),
        route("/version_files", 200, "{}"),
        route("/files/a.jar", 200, jar_a.clone()),
        route("/files/dep.jar", 200, jar_b.clone()),
    ]);
    h.click("Каталог");
    h.wait("search", 10, |a| a.mod_searched);
    h.click("Установить");
    h.wait("install finished", 20, |a| a.mod_install_job.is_none() && a.mod_install_report.is_some());
    let mods = instance::instance_mines(&h.root, "26.3 fabric").join("mods");
    assert!(mods.join("a.jar").exists() && mods.join("dep.jar").exists(), "both files expected in {}", mods.display());
}

#[test]
fn install_without_loader_is_refused_with_explanation() {
    let _g = lock();
    let vanilla = Instance { name: "plain".into(), game_version: "26.3".into(), created: "1".into(), ..Default::default() };
    let mut h = H::new(vec![vanilla]);
    let jar = b"x".to_vec();
    let url = format!("{}/files/x.jar", h.mock.url());
    h.mock.set(vec![
        route("/search", 200, search_json(&[("AAA", "Some Mod")])),
        route("/project/AAA/version", 200, format!("[{}]", version_json("v1", "AAA", &url, "x.jar", &jar, ""))),
        route("/version_files", 200, "{}"),
        route("/files/x.jar", 200, jar),
    ]);
    h.click("Каталог");
    h.wait("search", 10, |a| a.mod_searched);
    h.click("Установить");
    h.wait("install result", 20, |a| a.mod_install_job.is_none());
    let toast = h.app.toast.clone().unwrap_or_default();
    assert!(toast.contains("нет загрузчика"), "expected a clear message, got {toast:?}");
}

#[test]
fn every_page_renders_at_several_window_sizes() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric"), fabric_instance("Second")]);
    h.mock.set(vec![route("/search", 200, search_json(&[("AAA", "Sodium")]))]);
    for size in [egui::vec2(800.0, 600.0), egui::vec2(1280.0, 800.0), egui::vec2(2560.0, 1440.0), egui::vec2(480.0, 700.0)] {
        h.size = size;
        h.frame(vec![]);
        h.frame(vec![]); // the centered tabs move with the window
        h.click("Библиотека");
        assert!(h.has("card:26.3 fabric"), "library cards at {size:?}: {:?}", h.labels());
        h.click("card:26.3 fabric");
        assert_eq!(h.app.page, Page::Instance);
        for tab in ["Журнал", "Настройки", "Содержимое"] {
            let last = h.rects(tab).len() - 1; // the top-bar tab is registered first
            h.click_nth(tab, last);
        }
        h.click("Каталог");
        h.wait("catalog", 10, |a| a.mod_searched || a.mod_search_error.is_some());
        for tab in ["Установленные", "Модпаки", "Каталог"] {
            let last = h.rects(tab).len() - 1; // "Каталог" is also a top-bar tab
            h.click_nth(tab, last);
            h.frame(vec![]);
        }
        h.click("Настройки");
        assert_eq!(h.app.page, Page::Other);
    }
}

#[test]
fn library_search_field_filters_cards() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("Alpha"), fabric_instance("Beta")]);
    assert!(h.has("card:Alpha") && h.has("card:Beta"));
    h.click("field:library-search");
    h.type_text("bet");
    h.frame(vec![]);
    assert!(!h.has("card:Alpha"), "Alpha must be filtered out: {:?}", h.labels());
    assert!(h.has("card:Beta"));
}


// ------------------------------------------------------------------ Settings / account

fn open_settings(h: &mut H) {
    h.click("Настройки"); // the rail gear is registered first
    assert_eq!(h.app.page, Page::Other);
}

#[test]
fn offline_nickname_is_validated_saved_and_used_for_launch() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.size = egui::vec2(1280.0, 3000.0); // the whole settings page on one screen
    h.frame(vec![]);
    open_settings(&mut h);
    h.frame(vec![]);
    h.frame(vec![]); // let the layout settle after the size change
    assert!(h.has("field:nickname"), "{:?}", h.labels());

    h.app.username_input = "ab".into();
    h.frame(vec![]); // the "invalid nickname" hint appears and moves the button down
    h.click("Сохранить");
    assert!(h.app.toast.clone().unwrap_or_default().contains("Ник"), "{:?}", h.app.toast);
    assert_eq!(h.app.state.account.as_ref().unwrap().username, "Player");

    h.app.username_input.clear();
    h.click("field:nickname");
    h.type_text("Steve_01");
    h.click("Сохранить");
    let acc = h.app.state.account.clone().unwrap();
    assert_eq!(acc.username, "Steve_01");
    let auth = account::launch_auth(&acc).unwrap();
    assert_eq!((auth.token.as_str(), auth.uuid.clone()), ("0", account::offline_uuid("Steve_01")));
    // saved to disk
    assert_eq!(instance::load_state(&h.root).account.unwrap().username, "Steve_01");
}

#[test]
fn there_is_no_microsoft_login_but_there_is_an_offline_skins_tab() {
    let _g = lock();
    let mut h = H::new(vec![]);
    assert!(h.has("Скины"), "{:?}", h.labels());
    open_settings(&mut h);
    let labels = h.labels();
    assert!(!labels.iter().any(|l| l.contains("Microsoft") || l.contains("Client ID")), "{labels:?}");
}

#[test]
fn network_check_lists_hosts() {
    let _g = lock();
    let mut h = H::new(vec![]);
    open_settings(&mut h);
    assert!(h.has("Проверить сеть"), "{:?}", h.labels());
    // results can be shown without touching the real network
    h.app.net_results = vec![
        netcheck::CheckResult { name: "Modrinth API".into(), url: "u".into(), purpose: "поиск".into(), reachable: true, status: Some(200), millis: 120, detail: String::new() },
        netcheck::CheckResult { name: "Fabric Maven".into(), url: "u".into(), purpose: "Fabric".into(), reachable: false, status: None, millis: 8000, detail: "operation timed out".into() },
    ];
    h.frame(vec![]);
}

#[test]
fn account_chip_opens_settings() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("A")]);
    h.click("Библиотека");
    assert!(h.has("Аккаунт"), "{:?}", h.labels());
    h.click("Аккаунт");
    assert_eq!(h.app.page, Page::Other);
}


#[test]
fn disabled_buttons_ignore_clicks() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    // install buttons are disabled while an install is running
    h.mock.set(vec![route("/search", 200, search_json(&[("AAA", "Sodium")]))]);
    h.click("Каталог");
    h.wait("search", 10, |a| a.mod_searched);
    let running: Job<mods::InstallReport> = Arc::new(Mutex::new(None));
    h.app.mod_install_job = Some(running.clone());
    h.frame(vec![]);
    h.click("Установить");
    assert!(Arc::ptr_eq(h.app.mod_install_job.as_ref().unwrap(), &running), "a second install must not start");
    assert!(h.app.mod_install_prog.is_none());
}


#[test]
fn catalog_shows_project_icons() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 50, 50, 255])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let icon = format!("{}/icons/sodium.png", h.mock.url());
    let hits = format!(
        r#"{{"hits":[{{"project_id":"AAA","project_type":"mod","slug":"sodium","author":"a","title":"Sodium","description":"d","downloads":1,"follows":1,"icon_url":"{icon}","versions":["26.3"]}}],"total_hits":1}}"#
    );
    h.mock.set(vec![route("/search", 200, hits), route("/icons/sodium.png", 200, png)]);
    h.click("Каталог");
    h.wait("search", 10, |a| a.mod_searched);
    h.wait("icon loaded", 10, |a| matches!(a.pics.get(&format!("u:{icon}")), Some(Some(_))));
    assert!(h.mock.requests().iter().any(|r| r.contains("/icons/sodium.png")));
    // cached on disk for the next start
    assert!(images::icon_cache_path(&h.root.join("cache").join("icons"), &icon, 96).exists());
}

// ------------------------------------------------------------------ icons, rainbow, picture colours, import

#[test]
fn every_icon_has_a_glyph_in_the_bundled_font() {
    let _g = lock();
    let ctx = egui::Context::default();
    setup_fonts(&ctx, prefs::FontChoice::Builtin);
    let raw = || RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(300.0, 300.0))), ..Default::default() };
    let _ = ctx.run_ui(raw(), |_| {}); // fonts take effect on the next pass
    let mut missing = Vec::new();
    let _ = ctx.run_ui(raw(), |ui| {
        for &icon in ui::ALL_ICONS {
            let (ch, fill) = icon.glyph();
            let family = egui::FontFamily::Name(if fill { ui::ICON_FONT_FILL } else { ui::ICON_FONT }.into());
            let galley = ui.painter().layout_no_wrap(ch.to_string(), egui::FontId::new(24.0, family), Color32::WHITE);
            let ok = galley.rows.first().and_then(|r| r.glyphs.first()).map(|g| !g.uv_rect.is_nothing() && g.uv_rect.size.x > 1.0).unwrap_or(false);
            if !ok {
                missing.push(format!("{icon:?}"));
            }
        }
    });
    assert!(missing.is_empty(), "icons without a glyph: {missing:?}");
}

#[test]
fn rainbow_accent_can_be_switched_on_and_off() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.size = egui::vec2(1280.0, 3000.0);
    h.frame(vec![]);
    open_settings(&mut h);
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(h.has("accent:Радуга"), "{:?}", h.labels());
    h.click("accent:Радуга");
    assert!(h.app.state.ui.rainbow);
    let before = theme::green();
    for _ in 0..80 {
        h.frame(vec![]); // 4 simulated seconds
    }
    assert_ne!(theme::green(), before, "the accent must move while the rainbow is on");
    assert!(instance::load_state(&h.root).ui.rainbow, "saved");

    h.click("accent:Небо");
    assert!(!h.app.state.ui.rainbow);
    assert_eq!(theme::green(), Color32::from_rgb(0x4a, 0x9b, 0xf0));
    for _ in 0..40 {
        h.frame(vec![]);
    }
    assert_eq!(theme::green(), Color32::from_rgb(0x4a, 0x9b, 0xf0), "no more movement");
}

#[test]
fn text_colour_follows_the_background_picture() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.size = egui::vec2(1280.0, 3000.0);
    h.frame(vec![]);
    open_settings(&mut h);
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(theme::palette().dark, "dark theme by default");

    let dir = h.root.clone();
    let shade = move |v: u8| {
        let f = dir.join(format!("bg-{v}.png"));
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([v, v, v]))).save(&f).unwrap();
        f
    };
    let (white, black) = (shade(250), shade(10));
    h.click("Своя картинка");
    h.frame(vec![]);
    h.app.pick_override = Some(white);
    h.click("Выбрать картинку…");
    h.frame(vec![]);
    assert!(h.app.state.ui.bg_luma.unwrap_or(0.0) > 0.9, "{:?}", h.app.state.ui.bg_luma);
    assert!(!theme::palette().dark, "a bright picture gets dark text");
    assert!(theme::text().r() < 0x60);

    h.app.pick_override = Some(black);
    h.click("Выбрать картинку…");
    h.frame(vec![]);
    assert!(theme::palette().dark, "a dark picture gets light text");
    assert!(theme::text().r() > 0xa0);

    // switch it off: back to the theme setting, whatever the picture
    h.app.pick_override = Some(shade(250));
    h.click("Выбрать картинку…");
    h.frame(vec![]);
    assert!(!theme::palette().dark);
    // (the checkbox is an egui widget without a test label)
    h.app.state.ui.auto_text = false;
    LimeApp::apply_prefs(&h.ctx, &h.app.state.ui.clone());
    h.frame(vec![]);
    assert!(!h.app.state.ui.auto_text && theme::palette().dark);
}

#[test]
fn import_dialog_has_tabs_and_reads_archives_and_folders() {
    let _g = lock();
    let existing = Instance { name: "Taken".into(), game_version: "1.21.1".into(), created: "1".into(), ..Default::default() };
    let mut h = H::new(vec![existing]);
    h.size = egui::vec2(1280.0, 1400.0);
    h.frame(vec![]);

    h.click("Импорт");
    assert!(h.app.show_new_instance && h.app.new_tab == 3, "opens on the launchers tab");
    h.frame(vec![]);
    for tab in ["Новая", "Из папки", "Из файла", "Из лаунчеров", "Найти снова"] {
        assert!(h.has(tab), "{tab} missing; screen {:?}", h.labels());
    }

    // ---- from a file: a Prism export
    h.click("Из файла");
    assert_eq!(h.app.new_tab, 2);
    assert!(h.rects("Импортировать").len() == 1, "{:?}", h.labels());
    let zip_path = h.root.join("pack.zip");
    {
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let o: zip::write::SimpleFileOptions = Default::default();
        z.start_file("mmc-pack.json", o).unwrap();
        std::io::Write::write_all(&mut z, br#"{"components":[{"uid":"net.minecraft","version":"1.20.1"},{"uid":"net.fabricmc.fabric-loader","version":"0.15.0"}]}"#).unwrap();
        z.start_file("instance.cfg", o).unwrap();
        std::io::Write::write_all(&mut z, b"name=Taken\n").unwrap();
        z.finish().unwrap();
    }
    h.app.pick_override = Some(zip_path);
    h.click("Выбрать файл…");
    h.frame(vec![]);
    let info = h.app.import_info.clone().expect("archive recognized");
    assert_eq!((info.game_version.as_str(), info.loader.as_str()), ("1.20.1", "fabric"));
    assert_eq!(h.app.new_instance_name, "Taken (2)", "a clashing name is made unique");
    assert!(h.has("field:import-name"));

    // a name that is taken is refused without starting anything
    h.app.new_instance_name = "Taken".into();
    h.click("Импортировать");
    assert!(h.app.new_instance_job.is_none());
    assert!(h.app.toast.clone().unwrap_or_default().contains("уже существует"), "{:?}", h.app.toast);

    // a file that is not a build explains why
    let junk = h.root.join("junk.zip");
    std::fs::write(&junk, b"nope").unwrap();
    h.app.pick_override = Some(junk);
    h.click("Выбрать файл…");
    h.frame(vec![]);
    assert!(h.app.import_info.is_none() && h.app.import_error.is_some());

    // ---- from a folder
    h.click("Из папки");
    assert!(h.app.import_src.is_none());
    h.frame(vec![]);
    let folder = h.root.join("My Old World").join(".minecraft");
    std::fs::create_dir_all(folder.join("mods")).unwrap();
    h.app.pick_override = Some(folder);
    h.click("Выбрать папку…");
    h.frame(vec![]);
    assert!(h.app.import_src.is_some());
    assert_eq!(h.app.new_instance_name, "My Old World", "named after the folder above `.minecraft`");
    assert!(h.has("field:import-name") && h.has("Импортировать"));
}

#[test]
fn rainbow_and_picture_settings_survive_old_files() {
    let p: prefs::UiPrefs = serde_json::from_str(r#"{"accent":[1,2,3],"background":"image"}"#).unwrap();
    assert!(!p.rainbow && p.auto_text && p.bg_luma.is_none());
}

// ------------------------------------------------------------------ skins, style, navigation

fn write_skin(dir: &std::path::Path, name: &str, w: u32, h: u32, shade: u8) -> PathBuf {
    let f = dir.join(name);
    image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([shade, (x * 4) as u8, (y * 4) as u8, 255])).save(&f).unwrap();
    f
}

#[test]
fn skins_tab_adds_equips_removes_and_puts_the_skin_into_the_game() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    h.size = egui::vec2(1280.0, 1000.0);
    h.mock.set(vec![route("/search", 200, search_json(&[("CSL", "CustomSkinLoader")]))]);
    h.click("Скины");
    assert_eq!(h.app.page, Page::Skins);
    h.frame(vec![]);
    assert!(h.has("skin:none") && h.has("Загрузить скин") && h.has("По нику"), "{:?}", h.labels());
    assert!(h.app.state.skins.is_empty());

    // a real skin is added and worn by the active profile
    let file = write_skin(&h.root, "My Skin.png", 64, 64, 90);
    h.app.pick_override = Some(file);
    h.click("Загрузить скин");
    h.frame(vec![]);
    assert_eq!(h.app.state.skins.len(), 1);
    assert!(h.app.sparks.len() > 20, "a new skin comes with sparks: {}", h.app.sparks.len());
    let skin = h.app.state.skins[0].clone();
    assert_eq!(skin.name, "My Skin");
    assert_eq!(h.app.state.active_account().skin.as_deref(), Some(skin.id.as_str()));
    assert!(h.has(&format!("skin:{}", skin.id)));
    assert!(skins::skin_path(&h.root, &skin.id).is_file());
    assert_eq!(instance::load_state(&h.root).profiles[0].skin.as_deref(), Some(skin.id.as_str()), "saved");

    // wrong size: refused, nothing changes
    h.app.pick_override = Some(write_skin(&h.root, "big.png", 128, 128, 1));
    h.click("Загрузить скин");
    assert!(h.app.toast.clone().unwrap_or_default().contains("128×128"), "{:?}", h.app.toast);
    assert_eq!(h.app.state.skins.len(), 1);

    // back view
    h.click("Сзади");
    assert_eq!(h.app.skin_view, skins::View::Back);
    h.click("Спереди");
    assert_eq!(h.app.skin_view, skins::View::Front);

    // the skin goes where the skin mod reads it, under the profile's nickname
    let inst = h.app.state.instances[0].clone();
    let acc = h.app.state.account_for(&inst);
    h.app.apply_skin(&inst, &acc);
    let placed = instance::instance_mines(&h.root, &inst.name).join("CustomSkinLoader/LocalSkin/skins").join(format!("{}.png", acc.username));
    assert!(placed.is_file(), "{}", placed.display());

    // "no skin" takes it off the profile but keeps it in the library
    h.click("skin:none");
    assert!(h.app.state.active_account().skin.is_none() && h.app.state.skins.len() == 1);
    h.app.sparks.clear(); // (the sparks of the first skin are still falling)
    h.click(&format!("skin:{}", skin.id));
    assert!(h.app.state.active_account().skin.is_some());
    assert!(h.app.sparks.len() > 20, "putting a skin on throws sparks: {}", h.app.sparks.len());

    // the mod hint opens the catalog with the search filled in
    h.click("Найти мод");
    assert_eq!(h.app.page, Page::Mods);
    assert_eq!(h.app.mod_search_query, "CustomSkinLoader");
    h.wait("catalog search", 10, |a| a.mod_searched);

    // deleting from the library also frees the profile
    h.click("Скины");
    h.frame(vec![]);
    h.app.skin_preview = Some(skin.id.clone());
    h.frame(vec![]);
    h.click("Удалить из библиотеки");
    assert!(h.app.state.skins.is_empty() && h.app.state.active_account().skin.is_none());
    assert!(!skins::skin_path(&h.root, &skin.id).exists());
}

#[test]
fn skin_mod_status_follows_the_mods_folder() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("26.3 fabric")]);
    h.click("Скины");
    h.frame(vec![]);
    assert!(h.has("Найти мод"));
    let mods = instance::instance_mines(&h.root, "26.3 fabric").join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join("CustomSkinLoader_Fabric-14.28.jar"), b"x").unwrap();
    assert!(skins::has_skin_loader(&instance::instance_mines(&h.root, "26.3 fabric")));
    h.frame(vec![]);
}

#[test]
fn style_can_be_switched_between_pixel_and_soft() {
    let _g = lock();
    let mut h = H::new(vec![]);
    h.size = egui::vec2(1280.0, 3000.0);
    h.frame(vec![]);
    open_settings(&mut h);
    h.frame(vec![]);
    h.frame(vec![]);
    assert!(theme::pixel(), "pixel is the default");
    h.click("Мягкий");
    assert!(!theme::pixel() && h.app.state.ui.style == prefs::UiStyle::Soft);
    h.frame(vec![]);
    h.click("Пиксельный");
    assert!(theme::pixel());
    assert!(instance::load_state(&h.root).ui.style == prefs::UiStyle::Pixel);
    // fonts: the bundled one is the default; others switch without panics
    for font in [prefs::FontChoice::System, prefs::FontChoice::Builtin, prefs::FontChoice::Manrope] {
        h.app.state.ui.font = font;
        LimeApp::apply_prefs(&h.ctx, &h.app.state.ui.clone());
        h.frame(vec![]);
        h.frame(vec![]);
    }
}

#[test]
fn navigation_never_overlaps_at_any_window_width() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("A")]);
    for width in [480.0, 640.0, 800.0, 960.0, 1100.0, 1280.0, 2000.0] {
        h.size = egui::vec2(width, 800.0);
        h.frame(vec![]);
        h.frame(vec![]);
        let tabs: Vec<Rect> = ["Библиотека", "Каталог", "Скины", "Настройки"].iter().map(|l| h.rects(l)[0]).collect();
        let mut all = tabs.clone();
        all.push(h.rects("Аккаунт")[0]);
        all.extend(h.rects("Новая сборка").into_iter().take(1));
        all.extend(h.rects("Импорт").into_iter().take(1));
        for (i, a) in all.iter().enumerate() {
            assert!(a.left() >= -1.0 && a.right() <= width + 1.0, "width {width}: item {i} {a:?} is outside the window");
            for b in all.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "width {width}: {a:?} overlaps {b:?}");
            }
        }
    }
}

// ------------------------------------------------------------------ motion: sand, transitions, fades

fn wheel(dy: f32) -> Event {
    Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, dy),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
        modifiers: Modifiers::NONE,
    }
}

/// Settings page in a short window (so it scrolls), motion switched as asked.
fn scrolling_settings(animations: bool, sand: bool) -> H {
    let mut h = H::new(vec![]);
    h.size = egui::vec2(1280.0, 520.0);
    h.app.state.ui.animations = animations;
    h.app.state.ui.sand = sand;
    LimeApp::apply_prefs(&h.ctx, &h.app.state.ui.clone());
    h.frame(vec![]);
    open_settings(&mut h);
    for _ in 0..14 {
        h.frame(vec![]); // the page has finished fading in
    }
    h.frame(vec![Event::PointerMoved(Pos2::new(640.0, 340.0))]);
    h
}

/// Run `frames` frames, sending a wheel turn in each of the first `turns`; the most sand seen.
fn scroll_by(h: &mut H, dy: f32, turns: usize, frames: usize) -> usize {
    let mut peak = 0;
    for i in 0..frames {
        h.frame(if i < turns { vec![wheel(dy)] } else { vec![] });
        peak = peak.max(h.app.sand.len());
    }
    peak
}

#[test]
fn settings_crumble_into_sand_while_scrolling() {
    let _g = lock();
    let mut h = scrolling_settings(true, true);
    assert_eq!(h.app.sand.len(), 0, "nothing falls while the page is still");
    let mut crumbled = 0.0f32;
    let mut peak = 0;
    for i in 0..16 {
        h.frame(if i < 4 { vec![wheel(-220.0)] } else { vec![] });
        peak = peak.max(h.app.sand.len());
        crumbled = crumbled.max(h.app.sand_fade);
    }
    assert!(crumbled > 0.2, "the page edge breaks up while it moves: {crumbled}");
    assert_eq!(h.app.sand_dir, 1.0, "scrolling down: the content leaves through the top");
    assert!(h.app.sand_last_offset.unwrap_or(0.0) > 40.0, "the page really scrolled: {:?}", h.app.sand_last_offset);
    assert!(peak > 10, "scrolling down must throw sand, saw {peak}");
    assert!(h.app.sand.positions().all(|p| p.x.is_finite() && p.y.is_finite()));
    // and the sand settles by itself
    for _ in 0..90 {
        h.frame(vec![]);
    }
    assert_eq!(h.app.sand.len(), 0, "all grains are gone after a few seconds");
    assert!(h.app.sand_fade < 0.05, "and the edge is whole again: {}", h.app.sand_fade);
}

#[test]
fn pushing_against_an_end_of_the_page_does_nothing() {
    let _g = lock();
    let mut h = scrolling_settings(true, true);
    // at the very top, scrolling further up: no sand, the edge stays whole
    let mut fade = 0.0f32;
    for i in 0..30 {
        h.frame(if i % 3 == 0 { vec![wheel(300.0)] } else { vec![] });
        assert_eq!(h.app.sand.len(), 0, "no sand at the top end (frame {i})");
        fade = fade.max(h.app.sand_fade);
    }
    assert!(fade < 0.02, "the edge does not crumble at the top end: {fade}");

    // go all the way down: moving is what makes sand
    let mut moved = 0;
    for _ in 0..10 {
        moved = moved.max(scroll_by(&mut h, -600.0, 3, 6));
    }
    assert!(moved > 0, "scrolling down still crumbles the page");
    for _ in 0..120 {
        h.frame(vec![]);
    }
    assert_eq!(h.app.sand.len(), 0);
    let at_bottom = h.app.sand_last_offset.unwrap_or(0.0);
    assert!(at_bottom > 100.0, "it is at the bottom now: {at_bottom}");

    // at the bottom end, scrolling further down: nothing
    for i in 0..30 {
        h.frame(if i % 3 == 0 { vec![wheel(-300.0)] } else { vec![] });
        assert_eq!(h.app.sand.len(), 0, "no sand at the bottom end (frame {i})");
    }
    assert!(h.app.sand_fade < 0.02, "{}", h.app.sand_fade);
    assert_eq!(h.app.sand_last_offset.unwrap_or(0.0), at_bottom, "the page did not move");
}

#[test]
fn sand_and_motion_can_be_switched_off() {
    let _g = lock();
    let mut quiet = scrolling_settings(true, false);
    assert_eq!(scroll_by(&mut quiet, -220.0, 4, 16), 0, "sand switched off");
    assert_eq!(scroll_by(&mut quiet, 300.0, 1, 3), 0, "and nothing at an end");

    let mut still = scrolling_settings(false, true);
    assert!(!fx::enabled());
    assert_eq!(scroll_by(&mut still, -220.0, 4, 16), 0, "animations off turn the sand off too");
    // fades jump straight to their end, pages need no time to appear
    assert_eq!(ui::anim(&still.ctx, egui::Id::new("t"), true, 5.0), 1.0);
    assert_eq!(fx::progress(1.0, 1.0, 0.0, 1.0), 1.0);
    LimeApp::apply_prefs(&still.ctx, &prefs::UiPrefs::default());
    assert!(fx::enabled());
}

#[test]
fn pages_fade_in_and_cards_fly_in_one_by_one() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("A"), fabric_instance("B"), fabric_instance("C")]);
    h.app.state.ui.animations = true;
    LimeApp::apply_prefs(&h.ctx, &h.app.state.ui.clone());
    for _ in 0..12 {
        h.frame(vec![]);
    }
    let before = h.app.page_t0;
    h.click("Скины");
    assert_eq!(h.app.page, Page::Skins);
    assert!(h.app.page_t0 > before, "a new page restarts the transition");
    h.click("Библиотека");
    assert_eq!(h.app.page_seen, Page::Home);
    // right after the switch the cards are still flying in: lower than their final place
    let t0 = h.app.page_t0;
    let rects: Vec<Rect> = ["card:A", "card:B", "card:C"].iter().map(|l| h.rects(l)[0]).collect();
    for _ in 0..16 {
        h.frame(vec![]);
    }
    assert!(h.t as f64 - t0 > 0.5);
    let settled: Vec<Rect> = ["card:A", "card:B", "card:C"].iter().map(|l| h.rects(l)[0]).collect();
    for (a, b) in rects.iter().zip(&settled) {
        assert_eq!((a.min.x, a.size()), (b.min.x, b.size()), "cards only move vertically while flying in");
    }
}

#[test]
fn the_skin_figure_turns_around_smoothly() {
    let _g = lock();
    let mut h = H::new(vec![fabric_instance("A")]);
    h.size = egui::vec2(1280.0, 1000.0);
    h.click("Скины");
    h.app.pick_override = Some(write_skin(&h.root, "s.png", 64, 64, 50));
    h.click("Загрузить скин");
    h.frame(vec![]);
    assert!(h.app.skin_flip.is_none());
    h.click("Сзади");
    assert_eq!(h.app.skin_view, skins::View::Back);
    assert!(matches!(h.app.skin_flip, Some((_, skins::View::Front))), "turning from the front");
    for _ in 0..12 {
        h.frame(vec![]);
    }
    assert!(h.app.skin_flip.is_none(), "the turn is over after a third of a second");
}

#[test]
fn hover_and_selection_fade_instead_of_jumping() {
    let _g = lock();
    let h = H::new(vec![]);
    let id = egui::Id::new("fade-test");
    let mut seen = Vec::new();
    for k in 0..8 {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0))), time: Some(50.0 + k as f64 * 0.05), ..Default::default() };
        let _ = h.ctx.run_ui(raw, |_| {
            // (the first look at a state has nothing to fade from: start from "off")
            seen.push(ui::anim(&h.ctx, id, k > 0, 0.2));
        });
    }
    assert!(seen[0] < 0.1, "starts at the beginning: {seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "only grows: {seen:?}");
    assert!(seen.iter().any(|v| *v > 0.1 && *v < 0.95), "has in-between values: {seen:?}");
    assert!(*seen.last().unwrap() > 0.95, "ends at the target: {seen:?}");
}

#[test]
fn a_gliding_number_eases_out_and_can_change_course() {
    let _g = lock();
    let h = H::new(vec![]);
    let id = egui::Id::new("glide");
    let mut at = |time: f64, target: f32| {
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0))), time: Some(time), ..Default::default() };
        let mut v = 0.0;
        let _ = h.ctx.run_ui(raw, |_| v = ui::tween(&h.ctx, id, target, 0.4));
        v
    };
    assert_eq!(at(100.0, 10.0), 10.0, "the first value is taken as it is");
    let mut seen = vec![at(100.05, 110.0)];
    for k in 2..=10 {
        seen.push(at(100.0 + k as f64 * 0.05, 110.0));
    }
    assert_eq!(seen[0], 10.0, "the glide starts at the moment the target changes");
    assert!(seen[1] > 30.0 && seen[1] < 60.0, "then moves at once: {seen:?}");
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "only forward: {seen:?}");
    let first_step = seen[1] - seen[0];
    let last_step = seen[8] - seen[7];
    assert!(first_step > last_step * 3.0, "fast at first, slow at the end: {seen:?}");
    assert!((seen[9] - 110.0).abs() < 0.5, "arrives: {seen:?}");
    // a new target in the middle of the way starts from where it is now
    let mid = at(101.0, 0.0);
    assert!(mid > 100.0, "{mid}");
    let next = at(101.05, 0.0);
    assert!(next < mid && next > 50.0, "turns around smoothly: {mid} -> {next}");
}
