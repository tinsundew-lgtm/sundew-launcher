//! LimeCraft launcher UI built on eframe/egui.
//!
//! All blocking core operations (Modrinth / Mojang / installs / launch) run on worker
//! threads; the app only holds shared result slots (`Job<T>`) which are drained each
//! frame so the UI never blocks.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, RichText, Stroke};

use crate::assets::Assets;
use crate::fx;
use crate::theme;
use crate::ui::{self, Icon};
use crate::core::{account, images, import, install, instance, launch, modrinth, mods, mojang, netcheck, prefs, skins};
use crate::core::download;
use crate::core::instance::{Instance, LauncherState};

/// Shared result slot filled by a worker thread: `None` = still running.
type Job<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Активная вкладка (State). Три раздела по ТЗ:
///  - `Instances` — «Сборки» (главная, по умолчанию): список установленных
///    чистых клиентов (Vanilla/Fabric/Forge/NeoForge) + создание новых.
///  - `Mods`     — «Моды»: каталог и управление модами (+ модпаки Modrinth).
///  - `Other`    — «Другое»: ресурспаки, шейдеры, глобальные настройки.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Page {
    Home,
    Mods,
    Skins,
    Other,
    Instance,
}

/// What a file dialog is for.
#[derive(Clone, Copy)]
enum Pick {
    Skin,
    Image,
    Archive,
    Folder,
}

/// Раздел вкладки «Моды».
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModsView {
    Catalog,
    Installed,
    Modpacks,
}

/// Run a blocking closure on a worker thread, exposing the result through a
/// shared slot that is drained by the UI thread.
fn start_detached<T: Send + 'static>(
    client: &reqwest::blocking::Client,
    f: impl FnOnce(&reqwest::blocking::Client) -> Result<T, String> + Send + 'static,
) -> Job<T> {
    let job: Job<T> = Arc::new(Mutex::new(None));
    let slot = job.clone();
    let client = client.clone();
    std::thread::spawn(move || {
        let res = catch_job(|| f(&client));
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(res);
    });
    job
}

/// Run a job body, turning a panic into an `Err` so the UI never spins forever.
fn catch_job<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "неизвестная ошибка".to_string());
            Err(format!("внутренняя ошибка: {msg}"))
        }
    }
}

/// Human-readable error text ("Cancelled" comes from the download layer).
fn friendly(e: &str) -> String {
    if e == "Cancelled" { "Установка отменена".to_string() } else { e.to_string() }
}

/// Drain a finished job slot, if finished.
fn drain<T>(job: &Job<T>) -> Option<Result<T, String>> {
    job.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// `(fraction 0..=1, current file label)` from an InstallProgress.
fn progress_parts(prog: &install::InstallProgress) -> (f32, String) {
    let cur = prog.current.load(Ordering::Relaxed);
    let tot = prog.total.load(Ordering::Relaxed);
    let file = prog.current_file.lock().map(|g| g.clone()).unwrap_or_default();
    let frac = if tot > 0 { (cur as f32 / tot as f32).clamp(0.0, 1.0) } else { 0.0 };
    (frac, file)
}

/// Set up UI fonts: the bundled Manrope (or a Windows system font, or egui's own),
/// a bold family for titles and buttons, and the icon fonts.
fn setup_fonts(ctx: &egui::Context, choice: prefs::FontChoice) {
    let mut fonts = egui::FontDefinitions::default();
    let mut bold_font: Option<String> = None;
    match choice {
        prefs::FontChoice::Manrope => {
            // one variable font, two weights
            for (name, weight) in [("manrope", 500.0f32), ("manrope-bold", 800.0)] {
                let tweak = egui::FontTweak { coords: egui::epaint::text::VariationCoords::new([(b"wght", weight)]), ..Default::default() };
                let data = egui::FontData::from_static(include_bytes!("../assets/fonts/Manrope.ttf")).tweak(tweak);
                fonts.font_data.insert(name.to_owned(), Arc::new(data));
            }
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, "manrope".to_owned());
            }
            bold_font = Some("manrope-bold".to_owned());
        }
        prefs::FontChoice::System => {
            for (regular, bold) in [
                ("C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\segoeuib.ttf"),
                ("C:\\Windows\\Fonts\\arial.ttf", "C:\\Windows\\Fonts\\arialbd.ttf"),
                ("C:\\Windows\\Fonts\\DejaVuSans.ttf", "C:\\Windows\\Fonts\\DejaVuSans-Bold.ttf"),
                // Linux (Mint, Ubuntu, Debian, Fedora)
                ("/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf", "/usr/share/fonts/truetype/ubuntu/Ubuntu-B.ttf"),
                ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
                ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf"),
                ("/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf"),
                ("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf", "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf"),
            ] {
                if let Ok(bytes) = std::fs::read(regular) {
                    fonts.font_data.insert("ui".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
                    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                        list.insert(0, "ui".to_owned());
                    }
                    if let Ok(bytes) = std::fs::read(bold) {
                        fonts.font_data.insert("ui-bold".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
                        bold_font = Some("ui-bold".to_owned());
                    }
                    break;
                }
            }
        }
        prefs::FontChoice::Builtin => {}
    }
    // bold family: the bold font first, then everything ordinary text falls back to
    let mut bold_list: Vec<String> = bold_font.into_iter().collect();
    bold_list.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(ui::bold_family(), bold_list);
    // icon set (Phosphor): own families, so they never end up inside ordinary text
    for (name, bytes) in [
        (ui::ICON_FONT, include_bytes!("../assets/fonts/Phosphor.ttf").as_slice()),
        (ui::ICON_FONT_FILL, include_bytes!("../assets/fonts/Phosphor-Fill.ttf").as_slice()),
    ] {
        fonts.font_data.insert(name.to_owned(), Arc::new(egui::FontData::from_static(bytes)));
        fonts.families.insert(egui::FontFamily::Name(name.into()), vec![name.to_owned()]);
    }
    ctx.set_fonts(fonts);
}

/// The face (with the hat layer) of a skin, for small avatars.
fn paint_skin_head(ui: &egui::Ui, rect: egui::Rect, tex: &egui::TextureHandle) {
    let h = tex.size()[1] as f32;
    let uv = |x: f32, y: f32| egui::Rect::from_min_max(egui::pos2(x / 64.0, y / h), egui::pos2((x + 8.0) / 64.0, (y + 8.0) / h));
    ui::paint_notched(ui.painter(), rect, if theme::pixel() { (rect.width() * 0.08).clamp(2.0, 5.0) } else { 0.0 }, theme::button());
    ui.painter().image(tex.id(), rect, uv(8.0, 8.0), egui::Color32::WHITE);
    ui.painter().image(tex.id(), rect.expand(rect.width() * 0.06), uv(40.0, 8.0), egui::Color32::WHITE);
}

pub struct LimeApp {
    root: PathBuf,
    state: LauncherState,
    page: Page,
    assets: Assets,
    toast: Option<String>,

    // index of the "active" instance shared by Home / Mods / launch
    active_instance: usize,

    // ---- Пункт 2: версии из манифеста Mojang ----
    versions_job: Option<Job<Vec<mojang::VersionEntry>>>,
    versions_list: Vec<mojang::VersionEntry>,
    selected_version: String,
    /// Показывать снапшоты/беты в списке версий (по умолчанию — только релизы).
    show_snapshots: bool,
    /// Манифест уже запрошен хотя бы раз (чтобы не спамить сетевой запрос).
    manifest_requested: bool,

    /// Раздел вкладки «Моды» и тип содержимого (моды / ресурспаки / шейдеры).
    mods_view: ModsView,
    content_kind: mods::ContentKind,
    /// Искать только то, что подходит версии и загрузчику выбранной сборки.
    only_compatible: bool,

    // ---- Modpacks ----
    pack_search_query: String,
    pack_search_job: Option<Job<Vec<modrinth::SearchHit>>>,
    pack_results: Vec<modrinth::SearchHit>,
    selected_pack: Option<usize>,
    pack_install_name: String,
    pack_install_prog: Option<Arc<install::InstallProgress>>,
    pack_install_job: Option<Job<instance::Instance>>,

    // ---- Mods ----
    mod_search_query: String,
    mod_search_job: Option<Job<Vec<modrinth::SearchHit>>>,
    mod_results: Vec<modrinth::SearchHit>,
    selected_mod: Option<usize>,
    mod_versions_job: Option<Job<Vec<modrinth::Version>>>,
    mod_versions: Vec<modrinth::Version>,
    selected_mod_version: Option<usize>,
    mod_install_prog: Option<Arc<install::InstallProgress>>,
    mod_searched: bool,
    mod_search_error: Option<String>,
    pack_searched: bool,
    pack_search_error: Option<String>,
    mod_install_job: Option<Job<mods::InstallReport>>,
    mod_install_report: Option<String>,

    // ---- Установленное содержимое выбранной сборки ----
    installed_items: Vec<mods::InstalledItem>,
    installed_job: Option<Job<Vec<mods::InstalledItem>>>,
    /// (имя сборки, тип), для которых список уже загружен.
    installed_for: Option<(String, mods::ContentKind)>,
    updates: Vec<mods::UpdateInfo>,
    updates_job: Option<Job<Vec<mods::UpdateInfo>>>,
    update_job: Option<Job<String>>,

    // ---- Settings ----
    java_input: String,
    ram_input: String,
    /// Ник офлайн-аккаунта (редактируется в «Настройках», применяется при сохранении).
    username_input: String,

    // ---- Create-instance dialog ----
    show_new_instance: bool,
    new_instance_name: String,
    new_instance_version: String,
    new_instance_loader: String,
    new_instance_prog: Option<Arc<install::InstallProgress>>,
    new_instance_job: Option<Job<instance::Instance>>,
    new_instance_result: Option<Result<(), String>>,

    // ---- launch ----
    launch_job: Option<Job<String>>,
    launch_status: Option<String>,
    launch_prog: Option<Arc<install::InstallProgress>>,

    ctx: egui::Context,


    // ---- проверка сети ----
    net_job: Option<Job<Vec<netcheck::CheckResult>>>,
    net_results: Vec<netcheck::CheckResult>,

    // ---- новый интерфейс ----
    library_query: String,
    inst_tab: usize,
    inst_edit_for: Option<String>,
    inst_mem_input: String,
    inst_jvm_input: String,
    inst_java_input: String,
    log_lines: Vec<String>,
    log_offset: u64,
    log_for: Option<String>,
    /// Instance whose game window made us minimize: restore when it exits.
    restore_for: Option<String>,
    /// Loaded pictures: instance icons ("i:<name>") and profile avatars ("p:<file>").
    pics: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    bg_tex: Option<(String, egui::TextureHandle)>,
    new_profile_name: String,
    new_group_name: String,
    inst_use_color: bool,
    inst_color: [u8; 3],
    inst_group_input: String,
    inst_pinned: bool,
    inst_nick: String,
    /// Test hook: the answer of the next file dialog.
    pick_override: Option<PathBuf>,
    /// Instance being launched (for the after-launch behaviour).
    launching: Option<String>,
    profile_menu: bool,
    chip_rect: Option<egui::Rect>,
    /// Last applied light/dark decision for the "system" theme.
    applied_dark: Option<bool>,
    /// Project icons being downloaded (url -> job).
    icon_jobs: std::collections::HashMap<String, Job<(Vec<u8>, u32, u32)>>,
    /// Accent last applied by the rainbow mode.
    rainbow_last: Option<[u8; 3]>,

    // ---- motion ----
    /// Page shown last frame, and when it appeared (page transition, cards flying in).
    page_seen: Page,
    page_t0: f64,
    /// Sand that crumbles off the settings page while it scrolls.
    sand: fx::Sand,
    sand_last_offset: Option<f32>,
    /// How strongly the page edges dissolve right now (0..=1).
    sand_fade: f32,
    /// Side of the page the content is leaving through (+1 = scrolling down, the top edge).
    sand_dir: f32,
    /// Sparks that fly when a skin is put on.
    sparks: fx::Sand,
    skin_burst: bool,
    /// Skin preview turning around: (start time, the side it turns from).
    skin_flip: Option<(f64, skins::View)>,

    // ---- skins ----
    skin_view: skins::View,
    skin_nick: String,
    skin_nick_open: bool,
    skin_job: Option<Job<skins::Skin>>,
    /// Skin shown in the preview (the active profile's one by default).
    skin_preview: Option<String>,

    // ---- import (tabs of the "new build" dialog) ----
    new_tab: usize,
    /// Folder or archive chosen for the import.
    import_src: Option<PathBuf>,
    import_info: Option<import::ArchiveInfo>,
    import_error: Option<String>,
    import_worlds: bool,
    launcher_found: Vec<import::Found>,
    launchers_scanned: bool,
    toast_seen: Option<(String, std::time::Instant)>,

    /// Instance index awaiting delete confirmation.
    confirm_delete: Option<usize>,
}

impl LimeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut startup_error = None;
        let root = install::launcher_root().unwrap_or_else(|e| {
            // Нигде нет прав на запись — не падаем, показываем причину.
            eprintln!("LimeCraft: {e}");
            startup_error = Some(format!("Нет доступа к папке данных: {e}"));
            std::env::temp_dir().join("LimeCraft")
        });
        if startup_error.is_none() && root != instance::preferred_dir() {
            startup_error = Some(format!("Папка данных: {} (стандартная недоступна для записи)", root.display()));
        }
        Self::with_root(&cc.egui_ctx, root, startup_error)
    }

    /// Build the app for a given data root (also used by the headless UI tests).
    fn with_root(ctx: &egui::Context, root: PathBuf, startup_error: Option<String>) -> Self {
        Self::build(ctx, root, startup_error)
    }

    /// Theme, accent, zoom and font from the saved preferences.
    fn apply_prefs(ctx: &egui::Context, p: &prefs::UiPrefs) {
        fx::set_enabled(p.animations);
        let dark = p.use_dark(ctx.system_theme().map(|t| t == egui::Theme::Dark).unwrap_or(true));
        let accent = if p.rainbow { theme::rainbow_accent(ctx.input(|i| i.time)) } else { p.accent };
        theme::apply(ctx, dark, accent, p.scale, p.compact, p.style == prefs::UiStyle::Pixel);
        setup_fonts(ctx, p.font);
    }

    fn build(ctx: &egui::Context, root: PathBuf, startup_error: Option<String>) -> Self {
        let state = instance::load_state(&root);
        Self::apply_prefs(ctx, &state.ui);
        download::set_mirrors_enabled(!state.disable_mirrors);
        let selected_version = state
            .instances
            .first()
            .map(|i| i.game_version.clone())
            .unwrap_or_else(|| "latest".to_string());
        let mut app = Self {
            root,
            state: state.clone(),
            // debug aid for screenshots: LIMECRAFT_PAGE=instance|mods|installed|settings
            page: match std::env::var("LIMECRAFT_PAGE").as_deref() {
                Ok("instance") => Page::Instance,
                Ok("mods") | Ok("installed") => Page::Mods,
                Ok("settings") => Page::Other,
                Ok("skins") => Page::Skins,
                _ => Page::Home,
            },
            assets: Assets::load(ctx),
            toast: startup_error,
            active_instance: 0,
            versions_job: None,
            versions_list: Vec::new(),
            selected_version,
            show_snapshots: false,
            manifest_requested: false,
            mods_view: if std::env::var("LIMECRAFT_PAGE").as_deref() == Ok("installed") { ModsView::Installed } else { ModsView::Catalog },
            content_kind: mods::ContentKind::Mod,
            only_compatible: true,
            pack_search_query: String::new(),
            pack_search_job: None,
            pack_results: Vec::new(),
            selected_pack: None,
            pack_install_name: String::new(),
            pack_install_prog: None,
            pack_install_job: None,
            mod_search_query: String::new(),
            mod_search_job: None,
            mod_results: Vec::new(),
            selected_mod: None,
            mod_versions_job: None,
            mod_versions: Vec::new(),
            selected_mod_version: None,
            mod_install_prog: None,
            mod_install_job: None,
            mod_install_report: None,
            mod_searched: false,
            mod_search_error: None,
            pack_searched: false,
            pack_search_error: None,
            installed_items: Vec::new(),
            installed_job: None,
            installed_for: None,
            updates: Vec::new(),
            updates_job: None,
            update_job: None,
            java_input: state.java_path.clone().unwrap_or_default(),
            ram_input: if state.ram_mb > 0 {
                state.ram_mb.to_string()
            } else {
                "4096".to_string()
            },
            username_input: state
                .account
                .as_ref()
                .map(|a| a.username.clone())
                .unwrap_or_else(|| "Player".to_string()),
            show_new_instance: false,
            new_instance_name: String::new(),
            new_instance_version: String::new(),
            new_instance_loader: String::new(),
            new_instance_prog: None,
            new_instance_job: None,
            new_instance_result: None,
            launch_job: None,
            launch_status: None,
            launch_prog: None,
            ctx: ctx.clone(),
            net_job: None,
            net_results: Vec::new(),
            library_query: String::new(),
            inst_tab: 0,
            inst_edit_for: None,
            inst_mem_input: String::new(),
            inst_jvm_input: String::new(),
            inst_java_input: String::new(),
            log_lines: Vec::new(),
            log_offset: 0,
            log_for: None,
            restore_for: None,
            pics: std::collections::HashMap::new(),
            bg_tex: None,
            new_profile_name: String::new(),
            new_group_name: String::new(),
            inst_use_color: false,
            inst_color: prefs::DEFAULT_ACCENT,
            inst_group_input: String::new(),
            inst_pinned: false,
            inst_nick: String::new(),
            pick_override: None,
            launching: None,
            profile_menu: false,
            chip_rect: None,
            applied_dark: None,
            icon_jobs: std::collections::HashMap::new(),
            rainbow_last: None,
            page_seen: Page::Home,
            page_t0: 0.0,
            sand: fx::Sand::new(0x5eed_5a4d),
            sand_last_offset: None,
            sand_fade: 0.0,
            sand_dir: 1.0,
            sparks: fx::Sand::new(0x5ba7_c1e5),
            skin_burst: false,
            skin_flip: None,
            skin_view: skins::View::Front,
            skin_nick: String::new(),
            skin_nick_open: false,
            skin_job: None,
            skin_preview: None,
            new_tab: 0,
            import_src: None,
            import_info: None,
            import_error: None,
            import_worlds: true,
            launcher_found: Vec::new(),
            launchers_scanned: false,
            toast_seen: None,
            confirm_delete: None,
        };
        // debug aid for screenshots: LIMECRAFT_DIALOG=0..3 opens the "new build" dialog on a tab
        if let Some(tab) = std::env::var("LIMECRAFT_DIALOG").ok().and_then(|v| v.parse::<usize>().ok()) {
            app.open_new_instance_tab(tab.min(3));
        }
        app
    }

    /// Forget catalog results (target instance or content type changed).
    fn reset_mod_search(&mut self) {
        self.mod_results.clear();
        self.selected_mod = None;
        self.mod_versions.clear();
        self.mod_searched = false;
        self.mod_search_error = None;
        self.mod_install_report = None;
    }

    fn is_busy(&self) -> bool {
        self.versions_job.is_some()
            || self.pack_search_job.is_some()
            || self.pack_install_job.is_some()
            || self.mod_search_job.is_some()
            || self.mod_versions_job.is_some()
            || self.mod_install_job.is_some()
            || self.installed_job.is_some()
            || self.net_job.is_some()
            || !self.icon_jobs.is_empty()
            || self.skin_job.is_some()
            || self.updates_job.is_some()
            || self.update_job.is_some()
            || self.new_instance_job.is_some()
            || self.launch_job.is_some()
    }

    /// Instance name for a new build that cannot clash with an existing one
    /// (also after `sanitize`, which maps different names onto one folder).
    fn unique_name(&self, base: &str) -> String {
        let taken = |n: &str| self.state.instances.iter().any(|i| instance::sanitize(&i.name) == instance::sanitize(n));
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|k| format!("{base} ({k})"))
            .find(|n| !taken(n))
            .unwrap_or_else(|| base.to_string())
    }

    fn active_index(&self) -> usize {
        if self.state.instances.is_empty() {
            0
        } else {
            self.active_instance.min(self.state.instances.len() - 1)
        }
    }

    fn save_state(&mut self) {
        if let Err(e) = instance::save_state(&self.root, &self.state) {
            self.toast = Some(format!("Не удалось сохранить состояние: {e}"));
        }
    }

    fn upsert_instance(&mut self, inst: Instance) {
        if let Some(existing) = self.state.instances.iter_mut().find(|i| i.name == inst.name) {
            *existing = inst;
        } else {
            self.state.instances.push(inst);
        }
        self.save_state();
    }

    /// Shared progress bar renderer.
    fn install_progress_bar(&self, ui: &mut egui::Ui, prog: &Arc<install::InstallProgress>, cancel: Option<&AtomicBool>) {
        let (frac, label) = progress_parts(prog);
        ui.add(
            egui::ProgressBar::new(frac)
                .animate(true)
                .desired_width(ui.available_width())
                .fill(theme::green())
                .text(RichText::new(label).color(theme::text())),
        );
        if let Some(c) = cancel {
            if ui::secondary(ui, None, "Отмена") {
                c.store(true, Ordering::Relaxed);
            }
        }
    }
}

fn now_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

impl eframe::App for LimeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame_ui(ui);
    }
}

impl LimeApp {
    /// One UI frame (separate from `eframe::App::ui` so tests can drive it).
    fn frame_ui(&mut self, ui: &mut egui::Ui) {
        // Keep animating only while something runs; otherwise idle cheaply.
        if self.is_busy() {
            ui.ctx().request_repaint();
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }

        self.poll_jobs();
        self.restore_after_game();
        if self.state.ui.theme == prefs::ThemeMode::System {
            let dark = ui.ctx().system_theme().map(|t| t == egui::Theme::Dark).unwrap_or(true);
            if self.applied_dark != Some(dark) {
                self.applied_dark = Some(dark);
                Self::apply_prefs(ui.ctx(), &self.state.ui);
            }
        }
        let now = ui.ctx().input(|i| i.time);
        if self.page != self.page_seen {
            self.page_seen = self.page;
            self.page_t0 = now;
            self.sand.clear();
            self.sparks.clear();
            self.sand_last_offset = None;
        }
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("fx-page-t0"), self.page_t0);
            d.insert_temp(egui::Id::new("fx-card-order"), 0usize);
        });
        self.tick_rainbow(ui.ctx());
        self.paint_background(ui);
        let page_fill = if self.state.ui.background == prefs::Background::Plain { theme::bg() } else { egui::Color32::TRANSPARENT };

        let bar_fill = if self.state.ui.background == prefs::Background::Plain { theme::bg() } else { egui::Color32::TRANSPARENT };
        egui::Panel::top("brand_bar")
            .exact_size(42.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(bar_fill))
            .show(ui, |ui| self.draw_brand_bar(ui));
        egui::Panel::top("nav_bar")
            .exact_size(64.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(bar_fill).inner_margin(egui::Margin::symmetric(16, 0)))
            .show(ui, |ui| self.draw_nav_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(page_fill).inner_margin(egui::Margin::symmetric(26, 6)))
            .show(ui, |ui| {
                // a page fades in and slides up a little when it appears
                let enter = fx::ease_out_cubic(fx::progress(now, self.page_t0, 0.0, 0.2));
                if enter < 1.0 {
                    ui.ctx().request_repaint();
                }
                ui.set_opacity(enter);
                let out = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    match self.page {
                        Page::Home => self.draw_home(ui),
                        Page::Mods => self.draw_mods(ui),
                        Page::Skins => self.draw_skins(ui),
                        Page::Other => self.draw_other(ui),
                        Page::Instance => self.draw_instance_page(ui),
                    }
                });
                if self.page == Page::Other {
                    self.sand_from_scroll(ui.ctx(), &out);
                }
            });

        self.draw_toast(ui.ctx());
        self.draw_profile_menu(ui.ctx());

        if self.show_new_instance {
            self.draw_new_instance_dialog(ui.ctx());
        }
        if let Some(idx) = self.confirm_delete {
            self.draw_confirm_delete(ui.ctx(), idx);
        }
    }
}

impl LimeApp {
    /// The settings page crumbles like sand while it scrolls: grains break off the
    /// edge the content leaves through, and pulling past an end throws a burst.
    fn sand_from_scroll(&mut self, ctx: &egui::Context, out: &egui::scroll_area::ScrollAreaOutput<()>) {
        let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.1);
        let now = ctx.input(|i| i.time);
        let area = out.inner_rect;
        // where the page really is (rubber-banding past an end does not count)
        let offset = out.state.clamped_offset().y;
        let delta = self.sand_last_offset.map(|last| offset - last).unwrap_or(0.0);
        self.sand_last_offset = Some(offset);

        let on = fx::enabled() && self.state.ui.sand;
        let mut speed = 0.0f32; // points per second, positive = scrolling down
        if on {
            // only a page that really moves crumbles: pushing against an end does nothing
            speed = (delta / dt).clamp(-6000.0, 6000.0);
            if speed.abs() > 90.0 {
                self.sand_dir = speed.signum();
            }
            self.sand.wind = 90.0 + 70.0 * (now * 0.8).sin() as f32;
            let palette = fx::palette(theme::green(), theme::text());
            let depth = 24.0 + 90.0 * self.sand_fade;
            // content moving away: it crumbles at the edge it leaves through
            if speed.abs() > 90.0 {
                let count = ((speed.abs() * dt * 2.4) as usize).clamp(2, 170);
                let edge = if speed > 0.0 { fx::Edge::Top } else { fx::Edge::Bottom };
                self.sand.emit_deep(area, edge, count, (speed.abs() / 1400.0).clamp(0.35, 1.7), &palette, depth);
            }
        }
        // how much of the page edge has crumbled: quick to rise, slow to settle
        let target = (speed.abs() / 600.0).clamp(0.0, 1.0);
        let rate = if target > self.sand_fade { 16.0 } else { 3.5 };
        self.sand_fade += (target - self.sand_fade) * (dt * rate).min(1.0);

        self.sand.update(dt);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("sand-fx")));
        // blocks of background over the content: the edge it leaves through breaks up strongly,
        // the edge new content comes from only a little
        if self.sand_fade > 0.02 && self.state.ui.background == prefs::Background::Plain {
            let (leaving, arriving) = if self.sand_dir >= 0.0 { (fx::Edge::Top, fx::Edge::Bottom) } else { (fx::Edge::Bottom, fx::Edge::Top) };
            let band = 30.0 + 110.0 * self.sand_fade;
            for (edge, k) in [(leaving, 1.0), (arriving, 0.35)] {
                let mesh = fx::dissolve_mesh(area, edge, self.sand_fade * k, band, 6.0, theme::bg(), 7);
                if !mesh.is_empty() {
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
        if !self.sand.is_empty() {
            self.sand.paint(&painter, theme::pixel());
        }
        if !self.sand.is_empty() || self.sand_fade > 0.02 {
            ctx.request_repaint();
        }
    }

    /// Rainbow accent: walk through the hues a few times per second.
    fn tick_rainbow(&mut self, ctx: &egui::Context) {
        let p = &self.state.ui;
        if !p.rainbow {
            self.rainbow_last = None;
            return;
        }
        let accent = theme::rainbow_accent(ctx.input(|i| i.time));
        if self.rainbow_last != Some(accent) {
            self.rainbow_last = Some(accent);
            let dark = p.use_dark(ctx.system_theme().map(|t| t == egui::Theme::Dark).unwrap_or(true));
            theme::apply(ctx, dark, accent, p.scale, p.compact, p.style == prefs::UiStyle::Pixel);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }

    /// Bottom-center notification that disappears after a few seconds.
    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.toast.clone() else {
            self.toast_seen = None;
            return;
        };
        if self.toast_seen.as_ref().map(|(m, _)| m != &msg).unwrap_or(true) {
            self.toast_seen = Some((msg.clone(), std::time::Instant::now()));
        }
        let age = self.toast_seen.as_ref().map(|(_, t)| t.elapsed().as_secs()).unwrap_or(0);
        let bad = msg.starts_with("Ошибка") || msg.contains("Не удалось") || msg.contains("Нет доступа") || msg.contains("Сначала");
        if age >= if bad { 25 } else { 7 } {
            self.toast = None;
            self.toast_seen = None;
            return;
        }
        let appear = if fx::enabled() {
            fx::ease_out_cubic(self.toast_seen.as_ref().map(|(_, t)| t.elapsed().as_secs_f32()).unwrap_or(1.0) / 0.28)
        } else {
            1.0
        };
        if appear < 1.0 {
            ctx.request_repaint();
        }
        egui::Area::new(egui::Id::new("limecraft_toast"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -18.0 + (1.0 - appear) * 28.0))
            .show(ctx, |ui| {
                ui.set_opacity(appear);
                egui::Frame::new()
                    .fill(theme::card())
                    .stroke(Stroke::new(1.0, if bad { theme::danger() } else { theme::green() }))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(560.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&msg).color(if bad { theme::danger() } else { theme::text() }));
                            if ui::icon_button(ui, Icon::Close, 26.0, false, "Закрыть").clicked() {
                                self.toast = None;
                            }
                        });
                    });
            });
    }
}

// ------------------------------------------------------------------ nav rail

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "5 мин назад", "вчера", ...
fn ago(secs: u64) -> String {
    let d = now_secs().saturating_sub(secs);
    match d {
        0..=59 => "только что".to_string(),
        60..=3599 => format!("{} мин назад", d / 60),
        3600..=86399 => format!("{} ч назад", d / 3600),
        _ => format!("{} дн назад", d / 86400),
    }
}

fn loader_title(loader: &str) -> &str {
    match loader {
        "" => "Vanilla",
        "fabric" => "Fabric",
        "quilt" => "Quilt",
        "forge" => "Forge",
        "neoforge" => "NeoForge",
        other => other,
    }
}

fn loader_label(inst: &Instance) -> String {
    format!("{} {}", loader_title(&inst.loader), inst.game_version)
}

fn open_in_explorer(path: &std::path::Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

/// Persistent error box with a retry button. Returns true when "Повторить" was pressed.
fn error_banner(ui: &mut egui::Ui, title: &str, detail: &str) -> bool {
    let mut retry = false;
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(0x2e, 0x1d, 0x1b))
        .stroke(Stroke::new(1.0, theme::danger()))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 150.0).max(240.0));
                    ui.label(RichText::new(title).family(ui::bold_family()).color(theme::danger()));
                    ui.label(RichText::new(truncate(detail, 300)).color(theme::muted()).size(12.5));
                    if detail.contains("timed out") || detail.contains("Connect") || detail.contains("error sending request") {
                        ui.label(
                            RichText::new("Похоже, сервер недоступен из вашей сети: проверьте интернет и VPN/прокси (Настройки → Проверка сети).")
                                .color(theme::yellow())
                                .size(12.5),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::secondary(ui, Some(Icon::Refresh), "Повторить") {
                        retry = true;
                    }
                });
            });
        });
    retry
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

impl LimeApp {
    /// Wordmark in the middle of the very top.
    fn draw_brand_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let galley = ui.painter().layout_no_wrap("LIMECRAFT LAUNCHER".into(), egui::FontId::new(14.0, ui::bold_family()), theme::text());
        let (icon, gap) = (32.0, 10.0); // the 16 px icon enlarged exactly twice
        let x0 = (rect.center().x - (icon + gap + galley.size().x) / 2.0).round();
        let cy = rect.center().y;
        if let Some(badge) = &self.assets.icon {
            let r = egui::Rect::from_min_size(egui::pos2(x0, (cy - icon / 2.0).round()), egui::vec2(icon, icon));
            egui::Image::from_texture(egui::load::SizedTexture::from_handle(badge)).paint_at(ui, r);
        }
        ui.painter().galley(egui::pos2(x0 + icon + gap, cy - galley.size().y / 2.0), galley, theme::text());
        ui.allocate_rect(rect, egui::Sense::hover());
    }

    /// Profile (or "back") on the left, page tabs in the middle, actions on the right.
    /// The narrower the window, the more it turns into icons.
    fn draw_nav_bar(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        let line_y = full.bottom() - 0.5;
        ui.painter().line_segment([egui::pos2(full.left() - 16.0, line_y), egui::pos2(full.right() + 16.0, line_y)], Stroke::new(1.0, theme::divider()));

        let w = full.width();
        let (tab_icons, icon_only_tabs, full_buttons, small_chip) = (w >= 1100.0, w < 740.0, w >= 990.0, w < 560.0);
        let tab_defs = [
            (Icon::Home, "Библиотека", Page::Home),
            (Icon::Compass, "Каталог", Page::Mods),
            (Icon::Shirt, "Скины", Page::Skins),
            (Icon::Gear, "Настройки", Page::Other),
        ];
        let tabs: Vec<(Option<Icon>, &str)> = tab_defs.iter().map(|(i, l, _)| (tab_icons.then_some(*i), *l)).collect();
        let icon_tabs: Vec<(Icon, &str)> = tab_defs.iter().map(|(i, l, _)| (*i, *l)).collect();
        let tw = if icon_only_tabs { ui::icon_tabs_width(tab_defs.len()) } else { ui::pill_tabs_width(ui, &tabs) };
        let (left_w, right_w) = (if small_chip { 50.0 } else { 224.0 }, if full_buttons { 300.0 } else { 100.0 });
        let x0 = (full.center().x - tw / 2.0).max(full.left() + left_w + 12.0).min((full.right() - right_w - 12.0 - tw).max(full.left()));
        let mid = egui::Rect::from_min_size(egui::pos2(x0, full.top()), egui::vec2(tw, full.height()));
        let left = egui::Rect::from_min_max(full.left_top(), egui::pos2((mid.left() - 12.0).max(full.left()), full.bottom()));
        let right = egui::Rect::from_min_max(egui::pos2(mid.right() + 12.0, full.top()), full.right_bottom());

        ui.scope_builder(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            if self.page == Page::Instance {
                if ui::secondary(ui, Some(Icon::Back), "Назад") {
                    self.page = Page::Home;
                }
            } else {
                self.account_chip(ui, small_chip);
            }
        });

        let mut sel = match self.page {
            Page::Home | Page::Instance => 0,
            Page::Mods => 1,
            Page::Skins => 2,
            Page::Other => 3,
        };
        let before = sel;
        ui.scope_builder(egui::UiBuilder::new().max_rect(mid).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            if icon_only_tabs {
                ui::icon_tabs(ui, &icon_tabs, &mut sel);
            } else {
                ui::pill_tabs(ui, &tabs, &mut sel);
            }
        });
        if sel != before {
            self.page = tab_defs[sel].2;
        }

        ui.scope_builder(egui::UiBuilder::new().max_rect(right).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            if matches!(self.page, Page::Home | Page::Instance) {
                let (new, import) = if full_buttons {
                    (ui::primary(ui, Some(Icon::Plus), "Новая сборка"), ui::secondary(ui, Some(Icon::Import), "Импорт"))
                } else {
                    (
                        ui::icon_key(ui, Icon::Plus, ui::BtnKind::Primary, "Новая сборка").clicked(),
                        ui::icon_key(ui, Icon::Import, ui::BtnKind::Secondary, "Импорт").clicked(),
                    )
                };
                if new {
                    self.open_new_instance_dialog();
                }
                if import {
                    self.open_new_instance_tab(3);
                }
            }
        });
    }
}

// ------------------------------------------------------------------ poll

impl LimeApp {
    fn poll_jobs(&mut self) {
        // manifest versions
        if let Some(job) = &self.versions_job {
            if let Some(res) = drain(job) {
                self.versions_job = None;
                match res {
                    Ok(list) => {
                        self.versions_list = list;
                        if self.versions_list.iter().find(|v| v.id == self.selected_version).is_none() {
                            if let Some(first) = self.versions_list.first() {
                                self.selected_version = first.id.clone();
                            }
                        }
                        self.toast = Some(format!(
                            "Манифест загружен ({} версий)",
                            self.versions_list.len()
                        ));
                    }
                    Err(e) => self.toast = Some(format!("Ошибка манифеста: {e}")),
                }
            }
        }

        // skin by nickname
        if let Some(job) = &self.skin_job {
            if let Some(res) = drain(job) {
                self.skin_job = None;
                match res {
                    Ok(skin) => {
                        self.skin_nick_open = false;
                        self.skin_nick.clear();
                        self.toast = Some(format!("Скин «{}» добавлен", skin.name));
                        self.add_skin(skin, true);
                    }
                    Err(e) => self.toast = Some(format!("Не удалось скачать скин: {e}")),
                }
            }
        }

        // new-instance dialog install
        if let Some(job) = &self.new_instance_job {
            if let Some(res) = drain(job) {
                self.new_instance_job = None;
                self.new_instance_prog = None;
                match &res {
                    Ok(inst) => {
                        self.upsert_instance(inst.clone());
                    }
                    Err(_) => {}
                }
                self.new_instance_result = Some(res.map(|_| ()).map_err(|e| friendly(&e)));
            }
        }

        // modpack search
        if let Some(job) = &self.pack_search_job {
            if let Some(res) = drain(job) {
                self.pack_search_job = None;
                match res {
                    Ok(list) => {
                        self.pack_results = list;
                        self.selected_pack = None;
                        self.pack_searched = true;
                        self.pack_search_error = None;
                    }
                    Err(e) => self.pack_search_error = Some(e),
                }
            }
        }

        // modpack install
        if let Some(job) = &self.pack_install_job {
            if let Some(res) = drain(job) {
                self.pack_install_job = None;
                self.pack_install_prog = None;
                match res {
                    Ok(inst) => {
                        self.upsert_instance(inst);
                        self.toast = Some("Модпак установлен".to_string());
                    }
                    Err(e) => {
                        self.toast = Some(format!("Ошибка установки модпака: {}", friendly(&e)));
                    }
                }
            }
        }

        // mod search
        if let Some(job) = &self.mod_search_job {
            if let Some(res) = drain(job) {
                self.mod_search_job = None;
                match res {
                    Ok(list) => {
                        self.mod_results = list;
                        self.selected_mod = None;
                        self.mod_searched = true;
                        self.mod_search_error = None;
                    }
                    Err(e) => self.mod_search_error = Some(e),
                }
            }
        }

        // mod versions
        if let Some(job) = &self.mod_versions_job {
            if let Some(res) = drain(job) {
                self.mod_versions_job = None;
                match res {
                    Ok(list) => {
                        self.mod_versions = list;
                        self.selected_mod_version = None;
                    }
                    Err(e) => self.toast = Some(format!("Ошибка версий мода: {e}")),
                }
            }
        }

        // mod install
        if let Some(job) = &self.mod_install_job {
            if let Some(res) = drain(job) {
                self.mod_install_job = None;
                self.mod_install_prog = None;
                match res {
                    Ok(report) => {
                        let mut text = format!("Установлено: {}", report.installed.join(", "));
                        if !report.warnings.is_empty() {
                            text.push_str(&format!("\nПредупреждения: {}", report.warnings.join("; ")));
                        }
                        self.mod_install_report = Some(text);
                        self.installed_for = None; // список «Установленные» устарел
                    }
                    Err(e) => self.toast = Some(format!("Ошибка установки: {}", friendly(&e))),
                }
            }
        }


        // network check
        if let Some(job) = &self.net_job {
            if let Some(res) = drain(job) {
                self.net_job = None;
                match res {
                    Ok(list) => self.net_results = list,
                    Err(e) => self.toast = Some(format!("Ошибка проверки сети: {e}")),
                }
            }
        }

        // installed content list
        if let Some(job) = &self.installed_job {
            if let Some(res) = drain(job) {
                self.installed_job = None;
                match res {
                    Ok(items) => self.installed_items = items,
                    Err(e) => self.toast = Some(format!("Не удалось прочитать папку: {e}")),
                }
            }
        }

        // update check
        if let Some(job) = &self.updates_job {
            if let Some(res) = drain(job) {
                self.updates_job = None;
                match res {
                    Ok(list) => {
                        self.toast = Some(if list.is_empty() {
                            "Все файлы актуальны".to_string()
                        } else {
                            format!("Доступно обновлений: {}", list.len())
                        });
                        self.updates = list;
                    }
                    Err(e) => self.toast = Some(format!("Ошибка проверки обновлений: {}", friendly(&e))),
                }
            }
        }

        // applied updates
        if let Some(job) = &self.update_job {
            if let Some(res) = drain(job) {
                self.update_job = None;
                self.mod_install_prog = None;
                match res {
                    Ok(msg) => self.toast = Some(msg),
                    Err(e) => self.toast = Some(format!("Ошибка обновления: {}", friendly(&e))),
                }
                self.installed_for = None; // перечитать список
            }
        }

        // launch
        if let Some(job) = &self.launch_job {
            if let Some(res) = drain(job) {
                self.launch_job = None;
                self.launch_prog = None;
                match res {
                    Ok(msg) => {
                        self.launch_status = Some(msg.clone());
                        self.toast = Some(msg);
                        self.after_game_started();
                    }
                    Err(e) => {
                        self.launch_status = Some(e.clone());
                        self.toast = Some(e);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Home

impl LimeApp {
    /// Instance indexes, most recently played first.
    fn recent_instances(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.state.instances.len()).collect();
        idx.sort_by_key(|&i| {
            let inst = &self.state.instances[i];
            std::cmp::Reverse(inst.last_played.unwrap_or_else(|| inst.created.parse().unwrap_or(0)))
        });
        idx
    }

    fn instance_subtitle(inst: &Instance) -> String {
        match inst.last_played {
            Some(t) => format!("{} · {}", loader_label(inst), ago(t)),
            None => format!("{} · ещё не запускалась", loader_label(inst)),
        }
    }

    /// Play / Stop button for an instance row. Returns nothing; acts directly.
    fn play_button(&mut self, ui: &mut egui::Ui, idx: usize) {
        let name = self.state.instances[idx].name.clone();
        if launch::is_running(&name) {
            if ui::button(ui, ui::BtnKind::Danger, Some(Icon::Stop), "Остановить", true).clicked() {
                if let Err(e) = launch::stop(&name) {
                    self.toast = Some(e);
                }
            }
        } else {
            let enabled = self.launch_job.is_none();
            if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Play), "Играть", enabled).clicked() {
                self.active_instance = idx;
                self.launch_selected();
            }
        }
    }

    /// Главная вкладка «Сборки»: «Продолжить» + библиотека карточек.
    fn draw_home(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);

        if self.state.instances.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                if let Some(empty) = &self.assets.empty_state {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(180.0, 140.0)));
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Пока нет сборок").size(20.0).family(ui::bold_family()));
                ui.label(RichText::new("Создайте первую: выберите версию Minecraft и загрузчик — или импортируйте готовую.").color(theme::muted()));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - 340.0) / 2.0).max(0.0));
                    if ui::primary(ui, Some(Icon::Plus), "Новая сборка") {
                        self.open_new_instance_dialog();
                    }
                    if ui::secondary(ui, Some(Icon::Import), "Импорт") {
                        self.open_new_instance_tab(3);
                    }
                });
            });
            return;
        }

        // ---- Продолжить ----
        ui.label(RichText::new("Продолжить").size(17.0).family(ui::bold_family()));
        ui.add_space(4.0);
        for idx in self.recent_instances().into_iter().take(3) {
            let inst = self.state.instances[idx].clone();
            ui::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                    self.paint_instance_tile(ui, rect, &inst);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&inst.name).size(16.0).family(ui::bold_family()));
                        ui.label(RichText::new(Self::instance_subtitle(&inst)).color(theme::muted()).size(13.0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(ui, Icon::Dots, 34.0, false, "Открыть страницу сборки").clicked() {
                            self.active_instance = idx;
                            self.page = Page::Instance;
                        }
                        self.play_button(ui, idx);
                    });
                });
            });
            ui.add_space(4.0);
        }
        if let Some(status) = &self.launch_status {
            ui.label(RichText::new(status).size(13.0).color(theme::muted()));
        }
        if let Some(prog) = &self.launch_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
        }

        // ---- Библиотека ----
        ui.add_space(16.0);
        ui.label(RichText::new("Библиотека").size(17.0).family(ui::bold_family()));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 4.0).max(160.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.library_query)
                    .hint_text("Поиск по сборкам")
                    .desired_width(w)
                    .margin(egui::Margin::symmetric(10, 8)),
            );
            ui::reg("field:library-search", resp.rect);
        });
        ui.add_space(8.0);
        let q = self.library_query.trim().to_lowercase();
        let sections = self.library_sections(&q);
        if sections.iter().all(|(_, v)| v.is_empty()) {
            ui.label(RichText::new("Ничего не найдено.").color(theme::muted()));
        }
        let show_headers = sections.len() > 1;
        for (title, items) in sections {
            if items.is_empty() {
                continue;
            }
            if show_headers {
                ui.add_space(6.0);
                let resp = ui.label(RichText::new(format!("{title} · {}", items.len())).family(ui::bold_family()).color(theme::muted()));
                ui::reg(&format!("group:{title}"), resp.rect);
            }
            match self.state.ui.library_view {
                prefs::LibraryView::Grid => {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
                        for idx in items {
                            self.instance_card(ui, idx);
                        }
                    });
                }
                prefs::LibraryView::List => {
                    for idx in items {
                        self.instance_row(ui, idx);
                    }
                }
            }
            ui.add_space(6.0);
        }
    }

    /// Library sections: pinned first, then each group, then the rest; each
    /// sorted as chosen in the settings.
    fn library_sections(&self, query: &str) -> Vec<(String, Vec<usize>)> {
        let mut idx: Vec<usize> = (0..self.state.instances.len())
            .filter(|&i| query.is_empty() || self.state.instances[i].name.to_lowercase().contains(query))
            .collect();
        let insts = &self.state.instances;
        let last = |i: usize| insts[i].last_played.unwrap_or_else(|| insts[i].created.parse().unwrap_or(0));
        let created = |i: usize| insts[i].created.parse::<u64>().unwrap_or(0);
        let ver = |i: usize| insts[i].game_version.split(['.', '-']).map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
        match self.state.ui.sort {
            prefs::SortBy::LastPlayed => idx.sort_by_key(|&i| std::cmp::Reverse(last(i))),
            prefs::SortBy::Name => idx.sort_by_key(|&i| insts[i].name.to_lowercase()),
            prefs::SortBy::Created => idx.sort_by_key(|&i| std::cmp::Reverse(created(i))),
            prefs::SortBy::Version => idx.sort_by(|&a, &b| ver(b).cmp(&ver(a))),
        }
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        let pinned: Vec<usize> = idx.iter().copied().filter(|&i| insts[i].pinned).collect();
        if !pinned.is_empty() {
            out.push(("Закреплённые".to_string(), pinned));
        }
        for g in self.groups() {
            let items: Vec<usize> = idx.iter().copied().filter(|&i| !insts[i].pinned && insts[i].group.as_deref() == Some(g.as_str())).collect();
            out.push((g, items));
        }
        let rest: Vec<usize> = idx
            .iter()
            .copied()
            .filter(|&i| !insts[i].pinned && insts[i].group.as_deref().map(str::is_empty).unwrap_or(true))
            .collect();
        out.push((if out.is_empty() { "Все сборки".to_string() } else { "Без группы".to_string() }, rest));
        out
    }

    /// Compact list row for the library.
    fn instance_row(&mut self, ui: &mut egui::Ui, idx: usize) {
        let inst = self.state.instances[idx].clone();
        let inner = ui::card().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                self.paint_instance_tile(ui, rect, &inst);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&inst.name).family(ui::bold_family()));
                    ui.label(RichText::new(Self::instance_subtitle(&inst)).color(theme::muted()).size(12.0));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::icon_button(ui, Icon::Dots, 30.0, false, &format!("Открыть «{}»", inst.name)).clicked() {
                        self.active_instance = idx;
                        self.page = Page::Instance;
                        self.inst_tab = 0;
                    }
                    self.play_button(ui, idx);
                });
            });
        });
        ui::reg(&format!("row:{}", inst.name), inner.response.rect);
        ui.add_space(4.0);
    }

    /// Library card: pixel style = a wide cover with the icon, soft style = a square card.
    fn instance_card(&mut self, ui: &mut egui::Ui, idx: usize) {
        if theme::pixel() {
            self.instance_card_cover(ui, idx);
        } else {
            self.instance_card_soft(ui, idx);
        }
    }

    /// Wide card of the pixel style: tinted cover, big icon, name and version on a dark fade.
    /// The cards fly in one after another and lift when the pointer is over them.
    fn instance_card_cover(&mut self, ui: &mut egui::Ui, idx: usize) {
        let inst = self.state.instances[idx].clone();
        let running = launch::is_running(&inst.name);
        let (cw, ch) = self.state.ui.card_size.cover();
        let (hit, resp) = ui.allocate_exact_size(egui::vec2(cw, ch), egui::Sense::click());
        ui::reg(&format!("card:{}", inst.name), hit);
        let now = ui.ctx().input(|i| i.time);
        let enter = fx::ease_out_cubic(fx::progress(now, self.page_t0, idx as f32 * 0.045, 0.3));
        let hover = ui::anim(ui.ctx(), resp.id.with("pop"), resp.hovered(), 0.15);
        let hovered = resp.hovered();
        // where the card is drawn: a bit low while flying in, a bit high when hovered
        let rect = hit.translate(egui::vec2(0.0, (1.0 - enter) * 14.0 - 3.0 * hover));
        let saved_opacity = ui.opacity();
        ui.set_opacity(saved_opacity * enter);
        let painter = ui.painter().clone();
        let border = theme::mix(theme::divider(), theme::green(), hover);
        painter.extend(ui::pixel_box_shapes(rect, 4.0, theme::card(), border, 1.5 + 0.5 * hover));

        let inner = rect.shrink(2.0);
        let (r, g, b) = ui::tile_rgb(&inst.name, inst.tile_color);
        let tint = egui::Color32::from_rgb(r, g, b);
        let base = theme::card();
        ui::paint_gradient_notched(&painter, inner, 3.0, theme::mix(base, tint, 0.42 + 0.1 * hover), theme::mix(base, tint, 0.12 + 0.04 * hover));

        // the icon grows a little when hovered
        let isz = (ch * 0.42 * (1.0 + 0.07 * hover)).round();
        let icon = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + ch * 0.36 - 2.0 * hover), egui::vec2(isz, isz));
        self.paint_instance_tile(ui, icon, &inst);

        // dark fade under the text
        let fade = egui::Rect::from_min_max(egui::pos2(inner.left(), inner.top() + inner.height() * 0.48), inner.right_bottom());
        ui::paint_gradient_notched(&painter, fade, 3.0, egui::Color32::TRANSPARENT, egui::Color32::from_black_alpha(215));
        let name = truncate(&inst.name, (((cw - 24.0) / 8.6) as usize).max(6));
        painter.text(egui::pos2(rect.left() + 12.0, rect.bottom() - 36.0), egui::Align2::LEFT_TOP, name, egui::FontId::new(15.0, ui::bold_family()), egui::Color32::WHITE);
        painter.text(
            egui::pos2(rect.left() + 12.0, rect.bottom() - 18.0),
            egui::Align2::LEFT_TOP,
            truncate(&format!("{} · {}", loader_title(&inst.loader), inst.game_version), (((cw - 24.0) / 6.6) as usize).max(8)),
            egui::FontId::proportional(11.5),
            egui::Color32::from_white_alpha(185),
        );
        if inst.pinned {
            ui::paint_icon(ui, Icon::Pin, egui::Rect::from_center_size(rect.right_top() + egui::vec2(-16.0, 16.0), egui::vec2(16.0, 16.0)), egui::Color32::from_white_alpha(220));
        }
        if running {
            // the "running" tag breathes
            let pulse = if fx::enabled() { 0.5 + 0.5 * (now * 4.0).sin() as f32 } else { 0.0 };
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            let tag = egui::Rect::from_min_size(rect.left_top() + egui::vec2(10.0, 10.0), egui::vec2(86.0, 22.0));
            ui::paint_notched(&painter, tag, 3.0, theme::mix(theme::green(), egui::Color32::WHITE, 0.22 * pulse));
            painter.text(tag.center(), egui::Align2::CENTER_CENTER, "запущена", egui::FontId::new(11.5, ui::bold_family()), theme::accent_fg());
        }

        // play / stop key at the right edge (visible on hover or while running)
        let play = egui::Rect::from_center_size(egui::pos2(rect.right() - 28.0, rect.bottom() - 28.0), egui::vec2(38.0, 38.0));
        let play_resp = ui.interact(play, ui.id().with(("card-play", idx)), egui::Sense::click());
        if hovered || play_resp.hovered() || running {
            let kind = if running { ui::BtnKind::Danger } else { ui::BtnKind::Primary };
            let (face, lip, fg, hi) = ui::key_colors(kind, true, play_resp.hovered());
            let f = ui::paint_key(&painter, play, face, lip, play_resp.is_pointer_button_down_on(), hi);
            ui::paint_icon(ui, if running { Icon::Stop } else { Icon::Play }, egui::Rect::from_center_size(f.center(), egui::vec2(16.0, 16.0)), fg);
        }
        ui.set_opacity(saved_opacity);
        if play_resp.clicked() {
            if running {
                if let Err(e) = launch::stop(&inst.name) {
                    self.toast = Some(e);
                }
            } else if self.launch_job.is_none() {
                self.active_instance = idx;
                self.launch_selected();
            }
        } else if resp.clicked() {
            self.active_instance = idx;
            self.page = Page::Instance;
            self.inst_tab = 0;
        }
    }

    /// Square library card painted by hand: tile, name, loader/version, hover play.
    fn instance_card_soft(&mut self, ui: &mut egui::Ui, idx: usize) {
        let inst = self.state.instances[idx].clone();
        let running = launch::is_running(&inst.name);
        let (cw, ch, ts) = self.state.ui.card_size.dims();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(cw, ch), egui::Sense::click());
        ui::reg(&format!("card:{}", inst.name), rect);
        let hovered = resp.hovered();
        let radius = egui::CornerRadius::same(14);
        ui.painter().rect_filled(rect, radius, if hovered { theme::hover_bg() } else { theme::card() });
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(1.0, if hovered { theme::green() } else { theme::divider() }),
            egui::StrokeKind::Inside,
        );
        let tile = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + 14.0 + ts / 2.0), egui::vec2(ts, ts));
        self.paint_instance_tile(ui, tile, &inst);
        if inst.pinned {
            ui.painter().circle_filled(rect.right_top() + egui::vec2(-14.0, 14.0), 5.0, theme::green());
        }
        let max_chars = ((cw - 20.0) / 9.0) as usize;
        ui.painter().text(
            egui::pos2(rect.center().x, tile.bottom() + 16.0),
            egui::Align2::CENTER_TOP,
            truncate(&inst.name, max_chars),
            egui::FontId::proportional(15.5),
            theme::text(),
        );
        ui.painter().text(
            egui::pos2(rect.center().x, tile.bottom() + 38.0),
            egui::Align2::CENTER_TOP,
            truncate(&loader_label(&inst), max_chars + 4),
            egui::FontId::proportional(12.5),
            theme::muted(),
        );
        if running {
            ui.painter().text(
                egui::pos2(rect.center().x, (tile.bottom() + 56.0).min(rect.bottom() - 16.0)),
                egui::Align2::CENTER_TOP,
                "● запущена",
                egui::FontId::proportional(12.0),
                theme::green(),
            );
        }

        // round play button over the tile corner (visible on hover)
        let play_rect = egui::Rect::from_center_size(tile.right_bottom() - egui::vec2(8.0, 8.0), egui::vec2(34.0, 34.0));
        let play_resp = ui.interact(play_rect, ui.id().with(("card-play", idx)), egui::Sense::click());
        if hovered || play_resp.hovered() {
            let fill = if running { theme::danger() } else if play_resp.hovered() { theme::green_dark() } else { theme::green() };
            ui.painter().circle_filled(play_rect.center(), 17.0, fill);
            let icon = if running { Icon::Stop } else { Icon::Play };
            ui::paint_icon(ui, icon, egui::Rect::from_center_size(play_rect.center(), egui::vec2(16.0, 16.0)), theme::accent_fg());
        }
        if play_resp.clicked() {
            if running {
                if let Err(e) = launch::stop(&inst.name) {
                    self.toast = Some(e);
                }
            } else if self.launch_job.is_none() {
                self.active_instance = idx;
                self.launch_selected();
            }
        } else if resp.clicked() {
            self.active_instance = idx;
            self.page = Page::Instance;
            self.inst_tab = 0;
        }
    }

    fn remove_instance(&mut self, idx: usize) {
        let Some(inst) = self.state.instances.get(idx).cloned() else { return };
        if launch::is_running(&inst.name) {
            self.toast = Some("Сначала закройте игру этой сборки".to_string());
            return;
        }
        let dir = instance::instance_dir(&self.root, &inst.name);
        if dir.starts_with(self.root.join("instances")) {
            if let Err(e) = instance::remove_dir_robust(&dir) {
                // keep the entry so the user can retry
                self.toast = Some(format!("Не удалось удалить файлы: {e}"));
                return;
            }
        }
        self.state.instances.remove(idx);
        if self.active_instance >= self.state.instances.len() && !self.state.instances.is_empty() {
            self.active_instance = self.state.instances.len() - 1;
        }
        if self.page == Page::Instance {
            self.page = Page::Home;
        }
        self.save_state();
        self.toast = Some(format!("Сборка «{}» удалена", inst.name));
    }

    fn draw_confirm_delete(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(inst) = self.state.instances.get(idx).cloned() else {
            self.confirm_delete = None;
            return;
        };
        egui::Window::new("Удалить сборку?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!("«{}» и все её файлы (миры, моды, настройки) будут удалены с диска.", inst.name));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui::button(ui, ui::BtnKind::Danger, Some(Icon::Trash), "Удалить", true).clicked() {
                        self.confirm_delete = None;
                        self.remove_instance(idx);
                    }
                    if ui::secondary(ui, None, "Отмена") {
                        self.confirm_delete = None;
                    }
                });
            });
    }

    fn launch_selected(&mut self) {
        let idx = self.active_index();
        let Some(inst) = self.state.instances.get(idx).cloned() else {
            self.toast = Some("Выберите сборку".to_string());
            return;
        };
        if self.launch_job.is_some() {
            return;
        }
        if launch::is_running(&inst.name) {
            self.toast = Some("Эта сборка уже запущена".to_string());
            return;
        }
        let account = self.state.account_for(&inst);
        if let Err(e) = account::launch_auth(&account) {
            self.toast = Some(e);
            return;
        }
        self.state.instances[idx].last_played = Some(now_secs());
        self.save_state();
        // the chosen skin goes where the skin mod of the instance looks for it
        self.apply_skin(&inst, &account);

        let java_custom = self.state.java_path.clone();
        let ram_mb = std::cmp::max(self.state.ram_mb, 1024);
        let window_args = self.state.ui.window.game_args();
        self.launching = Some(inst.name.clone());
        let root = self.root.clone();
        let job: Job<String> = Arc::new(Mutex::new(None));
        let slot = job.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        self.launch_prog = Some(prog);
        std::thread::spawn(move || {
            let result = catch_job(|| {
                // No suitable Java anywhere? Fetch one (unless the user set a path).
                let custom = inst.java_path.as_deref().filter(|s| !s.trim().is_empty()).or(java_custom.as_deref());
                if custom.is_none() {
                    let major = launch::required_java_major(&root, &inst).unwrap_or(17);
                    install::ensure_java(&install::make_client(), &root, major, Some(&prog_ref))?;
                }
                let auth = account::launch_auth(&account)?;
                let launch = launch::build_launch_command(java_custom.as_deref(), &root, &inst, &auth.username, &auth.uuid, &auth.token, ram_mb, &window_args)?;
                let game_dir = launch.game_dir.clone();
                let mut child = launch::spawn_game(&launch.java, &launch.command, &game_dir)?;
                launch::check_early_exit(&mut child, &game_dir, 4)?;
                launch::track(&inst.name, child);
                Ok(format!("Игра «{}» запущена", inst.name))
            });
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        });
        self.launch_job = Some(job);
        self.launch_status = Some("Запуск…".to_string());
    }
}

// ------------------------------------------------------------------ Instance page

impl LimeApp {
    fn load_instance_edit(&mut self, inst: &Instance) {
        self.inst_edit_for = Some(inst.name.clone());
        self.inst_mem_input = inst.memory_mb.map(|m| m.to_string()).unwrap_or_default();
        self.inst_jvm_input = inst.jvm_args.clone().unwrap_or_default();
        self.inst_java_input = inst.java_path.clone().unwrap_or_default();
        self.inst_use_color = inst.tile_color.is_some();
        self.inst_color = inst.tile_color.unwrap_or(prefs::DEFAULT_ACCENT);
        self.inst_group_input = inst.group.clone().unwrap_or_default();
        self.inst_pinned = inst.pinned;
        self.inst_nick = inst.username.clone().unwrap_or_default();
        self.log_for = None;
    }

    fn draw_instance_page(&mut self, ui: &mut egui::Ui) {
        let idx = self.active_index();
        let Some(inst) = self.state.instances.get(idx).cloned() else {
            self.page = Page::Home;
            return;
        };
        if self.inst_edit_for.as_deref() != Some(inst.name.as_str()) {
            self.load_instance_edit(&inst);
        }
        let running = launch::is_running(&inst.name);

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui::icon_button(ui, Icon::Back, 36.0, false, "К списку сборок").clicked() {
                self.page = Page::Home;
            }
            let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
            self.paint_instance_tile(ui, rect, &inst);
            ui.vertical(|ui| {
                ui.label(RichText::new(&inst.name).size(24.0).family(ui::bold_family()));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(loader_label(&inst)).color(theme::muted()));
                    ui.label(RichText::new("·").color(theme::muted()));
                    let last = inst.last_played.map(|t| format!("последний запуск {}", ago(t))).unwrap_or_else(|| "ещё не запускалась".into());
                    ui.label(RichText::new(last).color(theme::muted()));
                    if running {
                        ui.label(RichText::new("● запущена").color(theme::green()));
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::icon_button(ui, Icon::Trash, 36.0, false, "Удалить сборку").clicked() {
                    self.confirm_delete = Some(idx);
                }
                if ui::icon_button(ui, Icon::Folder, 36.0, false, "Открыть папку сборки").clicked() {
                    let dir = instance::instance_mines(&self.root, &inst.name);
                    let _ = std::fs::create_dir_all(&dir);
                    open_in_explorer(&dir);
                }
                self.play_button(ui, idx);
            });
        });
        if let Some(status) = &self.launch_status {
            ui.add_space(4.0);
            ui.label(RichText::new(status).size(13.0).color(theme::muted()));
        }
        if let Some(prog) = &self.launch_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let tabs = [(Some(Icon::Cube), "Содержимое"), (Some(Icon::Terminal), "Журнал"), (Some(Icon::Sliders), "Настройки")];
        if ui::pill_tabs(ui, &tabs, &mut self.inst_tab) {
            self.log_for = None;
        }
        ui.add_space(10.0);

        match self.inst_tab {
            0 => {
                // content kinds + jump to the catalog
                ui.horizontal(|ui| {
                    let kinds = mods::ContentKind::ALL;
                    let mut sel = kinds.iter().position(|k| *k == self.content_kind).unwrap_or(0);
                    let labels: Vec<(Option<Icon>, &str)> = kinds.iter().map(|k| (None, k.label())).collect();
                    if ui::pill_tabs(ui, &labels, &mut sel) {
                        self.content_kind = kinds[sel];
                        self.installed_for = None;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::primary(ui, Some(Icon::Compass), "Найти в каталоге") {
                            self.mods_view = ModsView::Catalog;
                            self.reset_mod_search();
                            self.page = Page::Mods;
                        }
                    });
                });
                ui.add_space(8.0);
                self.draw_installed(ui);
            }
            1 => self.draw_instance_log(ui, &inst),
            _ => self.draw_instance_settings_tab(ui, &inst),
        }
    }
}

// ------------------------------------------------------------------ Customization

impl LimeApp {
    /// What the settings ask for once the game window is up.
    fn after_game_started(&mut self) {
        let Some(name) = self.launching.take() else { return };
        if self.state.ui.console_on_launch {
            if let Some(i) = self.state.instances.iter().position(|x| x.name == name) {
                self.active_instance = i;
                self.page = Page::Instance;
                self.inst_tab = 1;
                self.log_for = None;
            }
        }
        match self.state.ui.after_launch {
            prefs::AfterLaunch::Keep => {}
            prefs::AfterLaunch::Minimize => {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                self.restore_for = Some(name);
            }
            prefs::AfterLaunch::Close => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    /// Bring the window back when the game we minimized for has exited.
    fn restore_after_game(&mut self) {
        if let Some(name) = &self.restore_for {
            if !launch::is_running(name) {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.restore_for = None;
            }
        }
    }

    /// Quick profile switcher under the account chip.
    fn draw_profile_menu(&mut self, ctx: &egui::Context) {
        if !self.profile_menu {
            return;
        }
        let Some(chip) = self.chip_rect else { return };
        let mut close = false;
        let area = egui::Area::new(egui::Id::new("profile-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(chip.left(), chip.bottom() + 6.0))
            .show(ctx, |ui| {
                ui::card().show(ui, |ui| {
                    ui.set_width(196.0);
                    ui.label(RichText::new("Играть как").color(theme::muted()).size(12.0));
                    for i in 0..self.state.profiles.len() {
                        let acc = self.state.profiles[i].clone();
                        let active = i == self.state.active_profile;
                        let label = format!("{}{}", if active { "✓ " } else { "" }, acc.username);
                        let resp = ui.selectable_label(active, label);
                        ui::reg(&format!("profile:{}", acc.username), resp.rect);
                        if resp.clicked() {
                            self.state.active_profile = i;
                            self.state.normalize();
                            self.username_input = acc.username.clone();
                            self.save_state();
                            close = true;
                        }
                    }
                    ui.separator();
                    let resp = ui.selectable_label(false, "Настроить профили…");
                    ui::reg("profile:settings", resp.rect);
                    if resp.clicked() {
                        self.page = Page::Other;
                        close = true;
                    }
                });
            });
        let clicked_outside = ctx.input(|i| i.pointer.any_click())
            && !area.response.rect.contains(ctx.input(|i| i.pointer.interact_pos().unwrap_or_default()))
            && !chip.contains(ctx.input(|i| i.pointer.interact_pos().unwrap_or_default()));
        if close || clicked_outside {
            self.profile_menu = false;
        }
    }

    // ---------------------------------------------------------------- pictures

    /// Cached texture of a picture file (`None` when it cannot be read).
    fn pic(&mut self, key: &str, path: &std::path::Path) -> Option<egui::TextureHandle> {
        if let Some(t) = self.pics.get(key) {
            return t.clone();
        }
        let tex = images::load_rgba(path).ok().map(|(rgba, w, h)| {
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            self.ctx.load_texture(format!("pic-{key}"), img, egui::TextureOptions::LINEAR)
        });
        self.pics.insert(key.to_string(), tex.clone());
        tex
    }

    /// Icon of a Modrinth project: from memory, the disk cache or the network
    /// (in the background; at most a few downloads at once). `None` until ready.
    fn remote_icon(&mut self, url: &str) -> Option<egui::TextureHandle> {
        let key = format!("u:{url}");
        if let Some(t) = self.pics.get(&key) {
            return t.clone();
        }
        if let Some(job) = self.icon_jobs.get(url) {
            if let Some(res) = drain(job) {
                self.icon_jobs.remove(url);
                let tex = res.ok().map(|(rgba, w, h)| {
                    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    self.ctx.load_texture(format!("icon-{url}"), img, egui::TextureOptions::LINEAR)
                });
                self.pics.insert(key, tex.clone());
                return tex;
            }
            return None;
        }
        if self.icon_jobs.len() < 6 {
            let (url_owned, cache) = (url.to_string(), self.root.join("cache").join("icons"));
            let client = install::make_client();
            let job = start_detached(&client, move |c| images::cached_icon(c, &url_owned, &cache, 96));
            self.icon_jobs.insert(url.to_string(), job);
        }
        None
    }

    /// Project icon, or a coloured tile with initials while loading / without icon.
    fn project_icon(&mut self, ui: &mut egui::Ui, title: &str, icon_url: Option<&str>, size: f32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        match icon_url.filter(|u| !u.is_empty()).and_then(|u| self.remote_icon(u)) {
            Some(tex) => ui::paint_picture(ui, rect, &tex),
            None => ui::paint_tile(ui, rect, title),
        }
    }

    fn instance_icon(&mut self, inst: &Instance) -> Option<egui::TextureHandle> {
        let file = inst.icon.as_deref()?;
        let path = instance::instance_dir(&self.root, &inst.name).join(file);
        self.pic(&format!("i:{}:{file}", inst.name), &path)
    }

    /// Icon, or a coloured tile with initials.
    fn paint_instance_tile(&mut self, ui: &egui::Ui, rect: egui::Rect, inst: &Instance) {
        match self.instance_icon(inst) {
            Some(tex) => ui::paint_picture(ui, rect, &tex),
            None => ui::paint_tile_color(ui, rect, &inst.name, inst.tile_color),
        }
    }

    fn avatar_texture(&mut self, acc: &account::Account) -> Option<egui::TextureHandle> {
        let file = acc.avatar.as_deref()?;
        let path = self.root.join("profiles").join(file);
        self.pic(&format!("p:{file}"), &path)
    }

    /// Open a file dialog for a picture (tests inject the answer).
    fn pick_image(&mut self) -> Option<PathBuf> {
        self.pick(Pick::Image)
    }

    /// File or folder dialog (tests inject the answer).
    fn pick(&mut self, what: Pick) -> Option<PathBuf> {
        if let Some(p) = self.pick_override.take() {
            return Some(p);
        }
        #[cfg(not(test))]
        {
            let dialog = rfd::FileDialog::new();
            return match what {
                Pick::Skin => dialog.add_filter("Скины", &["png"]).pick_file(),
                Pick::Image => dialog.add_filter("Картинки", &["png", "jpg", "jpeg", "bmp", "webp", "gif"]).pick_file(),
                Pick::Archive => dialog.add_filter("Сборки", &["mrpack", "zip"]).pick_file(),
                Pick::Folder => dialog.pick_folder(),
            };
        }
        #[cfg(test)]
        {
            let _ = what;
            None
        }
    }

    fn set_instance_icon(&mut self, name: &str, src: Option<PathBuf>) {
        let dest = instance::instance_dir(&self.root, name).join("icon.png");
        match src {
            Some(src) => {
                if let Err(e) = images::save_square(&src, &dest, 256) {
                    self.toast = Some(format!("Не удалось поставить иконку: {e}"));
                    return;
                }
            }
            None => {
                let _ = std::fs::remove_file(&dest);
            }
        }
        self.pics.retain(|k, _| !k.starts_with(&format!("i:{name}:")));
        let has = dest.exists();
        if let Some(i) = self.state.instances.iter_mut().find(|i| i.name == name) {
            i.icon = has.then(|| "icon.png".to_string());
        }
        self.save_state();
    }

    // ---------------------------------------------------------------- background

    /// Gradient or picture behind the pages (plain = the panels' own fill).
    fn paint_background(&mut self, ui: &egui::Ui) {
        let p = self.state.ui.clone();
        if p.background == prefs::Background::Plain {
            return;
        }
        let rect = ui.ctx().content_rect();
        let painter = ui.ctx().layer_painter(egui::LayerId::background());
        let (bg, acc) = (theme::bg(), theme::green());
        match p.background {
            prefs::Background::Gradient => {
                let mut mesh = egui::Mesh::default();
                mesh.colored_vertex(rect.left_top(), theme::mix(bg, acc, 0.05));
                mesh.colored_vertex(rect.right_top(), theme::mix(bg, acc, 0.16));
                mesh.colored_vertex(rect.right_bottom(), theme::mix(bg, acc, 0.30));
                mesh.colored_vertex(rect.left_bottom(), theme::mix(bg, acc, 0.10));
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(0, 2, 3);
                painter.add(egui::Shape::mesh(mesh));
            }
            prefs::Background::Image => {
                painter.rect_filled(rect, 0.0, bg);
                let Some(file) = p.bg_image.clone() else { return };
                let path = self.root.join("custom").join(&file);
                let tex = match &self.bg_tex {
                    Some((f, t)) if *f == file => Some(t.clone()),
                    _ => {
                        let t = images::load_rgba(&path).ok().map(|(rgba, w, h)| {
                            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                            self.ctx.load_texture("window-bg", img, egui::TextureOptions::LINEAR)
                        });
                        self.bg_tex = t.clone().map(|t| (file.clone(), t));
                        t
                    }
                };
                if let Some(tex) = tex {
                    // cover-fit
                    let size = tex.size_vec2();
                    let scale = (rect.width() / size.x).max(rect.height() / size.y);
                    let shown = size * scale;
                    let draw = egui::Rect::from_center_size(rect.center(), shown);
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.with_clip_rect(rect).image(tex.id(), draw, uv, egui::Color32::WHITE);
                    painter.rect_filled(rect, 0.0, bg.gamma_multiply(p.bg_dim));
                }
            }
            prefs::Background::Plain => {}
        }
    }

    // ---------------------------------------------------------------- live console

    fn draw_instance_log(&mut self, ui: &mut egui::Ui, inst: &Instance) {
        let path = instance::instance_mines(&self.root, &inst.name).join("launcher.log");
        if self.log_for.as_deref() != Some(inst.name.as_str()) {
            self.log_lines.clear();
            self.log_offset = 0;
            self.log_for = Some(inst.name.clone());
        }
        let (fresh, restarted) = launch::read_log_tail(&path, &mut self.log_offset, 1 << 20);
        if restarted {
            self.log_lines.clear();
        }
        let got = !fresh.is_empty();
        self.log_lines.extend(fresh);
        if self.log_lines.len() > 3000 {
            let cut = self.log_lines.len() - 3000;
            self.log_lines.drain(..cut);
        }
        let running = launch::is_running(&inst.name);
        if running || got {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        ui.horizontal(|ui| {
            if ui::secondary(ui, None, "Очистить") {
                self.log_lines.clear();
            }
            if ui::secondary(ui, Some(Icon::Folder), "Открыть папку") {
                open_in_explorer(&instance::instance_mines(&self.root, &inst.name));
            }
            let status = if running { "● игра запущена — журнал обновляется сам" } else { "игра не запущена" };
            ui.label(RichText::new(status).color(if running { theme::green() } else { theme::muted() }).size(12.0));
        });
        ui.add_space(6.0);
        let lines = &self.log_lines;
        ui::card().show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(480.0).stick_to_bottom(true).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if lines.is_empty() {
                    ui.label(RichText::new("Журнал пуст — игра ещё не запускалась.").color(theme::muted()));
                } else {
                    for l in lines {
                        let color = if l.contains("ERROR") || l.contains("Exception") {
                            theme::danger()
                        } else if l.contains("WARN") {
                            theme::yellow()
                        } else {
                            theme::text()
                        };
                        ui.label(RichText::new(l).monospace().size(12.0).color(color));
                    }
                }
            });
        });
    }

    // ---------------------------------------------------------------- instance customization

    fn draw_instance_settings_tab(&mut self, ui: &mut egui::Ui, inst: &Instance) {
        // ---- look ----
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::Palette, "Внешний вид");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 72.0), egui::Sense::hover());
                let mut preview = inst.clone();
                preview.tile_color = if self.inst_use_color { Some(self.inst_color) } else { None };
                self.paint_instance_tile(ui, rect, &preview);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if ui::secondary(ui, Some(Icon::Plus), "Своя иконка…") {
                            if let Some(src) = self.pick_image() {
                                self.set_instance_icon(&inst.name, Some(src));
                            }
                        }
                        if inst.icon.is_some() && ui::secondary(ui, Some(Icon::Trash), "Убрать иконку") {
                            self.set_instance_icon(&inst.name, None);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.inst_use_color, "Свой цвет плитки");
                        if self.inst_use_color {
                            ui.color_edit_button_srgb(&mut self.inst_color);
                        }
                    });
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Группа").color(theme::muted()));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.inst_group_input).desired_width(180.0).hint_text("без группы"));
                ui::reg("field:instance-group", resp.rect);
                let groups = self.groups();
                if !groups.is_empty() {
                    egui::ComboBox::from_id_salt("pickGroup").selected_text("выбрать").show_ui(ui, |ui| {
                        for g in groups {
                            if ui.selectable_label(false, &g).clicked() {
                                self.inst_group_input = g;
                            }
                        }
                    });
                }
                ui.checkbox(&mut self.inst_pinned, "Закрепить вверху");
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Ник для этой сборки").color(theme::muted()));
                let names: Vec<String> = self.state.profiles.iter().map(|p| p.username.clone()).collect();
                let current = if self.inst_nick.is_empty() { "как в профиле".to_string() } else { self.inst_nick.clone() };
                egui::ComboBox::from_id_salt("pickNick").selected_text(current).show_ui(ui, |ui| {
                    if ui.selectable_label(self.inst_nick.is_empty(), "как в профиле").clicked() {
                        self.inst_nick.clear();
                    }
                    for n in names {
                        if ui.selectable_label(self.inst_nick == n, &n).clicked() {
                            self.inst_nick = n;
                        }
                    }
                });
                let resp = ui.add(egui::TextEdit::singleline(&mut self.inst_nick).desired_width(140.0).hint_text("или свой ник"));
                ui::reg("field:instance-nick", resp.rect);
            });
        });
        ui.add_space(10.0);

        // ---- launch ----
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::Rocket, "Запуск");
            ui.add_space(4.0);
            ui.label(RichText::new("Пресет памяти").color(theme::muted()).size(12.0));
            ui.horizontal(|ui| {
                for preset in prefs::MEMORY_PRESETS.iter() {
                    if ui::secondary(ui, None, preset.name) {
                        self.inst_mem_input = preset.memory_mb.to_string();
                        self.inst_jvm_input = preset.jvm_args.to_string();
                    }
                }
            });
            ui.label(
                RichText::new(prefs::MEMORY_PRESETS.iter().map(|p| format!("{} — {}", p.name, p.hint)).collect::<Vec<_>>().join("   ·   "))
                    .color(theme::muted())
                    .size(11.5),
            );
            ui.add_space(8.0);
            ui.label(RichText::new("Память (МБ)").family(ui::bold_family()));
            ui.label(RichText::new("Пусто — используется общее значение из «Настроек».").color(theme::muted()).size(12.0));
            ui.add(egui::TextEdit::singleline(&mut self.inst_mem_input).desired_width(160.0).hint_text("напр. 4096"));
            ui.add_space(8.0);
            ui.label(RichText::new("Аргументы JVM").family(ui::bold_family()));
            ui.add(egui::TextEdit::singleline(&mut self.inst_jvm_input).desired_width(520.0).hint_text("напр. -XX:+UseG1GC"));
            ui.add_space(8.0);
            ui.label(RichText::new("Путь к Java").family(ui::bold_family()));
            ui.label(RichText::new("Пусто — Java подберётся автоматически (при необходимости скачается).").color(theme::muted()).size(12.0));
            ui.add(egui::TextEdit::singleline(&mut self.inst_java_input).desired_width(520.0).hint_text(if cfg!(windows) { "C:\\...\\java.exe" } else { "/usr/lib/jvm/.../bin/java" }));
        });
        ui.add_space(12.0);
        if ui::primary(ui, None, "Сохранить") {
            self.save_instance_settings(inst);
        }
        ui.add_space(20.0);
    }

    /// Existing group names, sorted.
    fn groups(&self) -> Vec<String> {
        let mut g: Vec<String> = self.state.instances.iter().filter_map(|i| i.group.clone()).filter(|g| !g.is_empty()).collect();
        g.sort_by_key(|s| s.to_lowercase());
        g.dedup();
        g
    }

    fn save_instance_settings(&mut self, inst: &Instance) {
        let mem = self.inst_mem_input.trim();
        let memory_mb = if mem.is_empty() {
            None
        } else {
            match mem.parse::<u64>() {
                Ok(m) if (512..=65536).contains(&m) => Some(m),
                _ => {
                    self.toast = Some("Память: число от 512 до 65536 МБ".to_string());
                    return;
                }
            }
        };
        let nick = self.inst_nick.trim().to_string();
        if !nick.is_empty() && !account::valid_nickname(&nick) {
            self.toast = Some("Ник сборки: 3–16 символов, латиница, цифры и _".to_string());
            return;
        }
        let nonempty = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(i) = self.state.instances.iter_mut().find(|i| i.name == inst.name) {
            i.memory_mb = memory_mb;
            i.jvm_args = nonempty(&self.inst_jvm_input);
            i.java_path = nonempty(&self.inst_java_input);
            i.group = nonempty(&self.inst_group_input);
            i.pinned = self.inst_pinned;
            i.tile_color = self.inst_use_color.then_some(self.inst_color);
            i.username = Some(nick).filter(|n| !n.is_empty());
        }
        self.save_state();
        self.toast = Some("Настройки сборки сохранены".to_string());
    }

    // ---------------------------------------------------------------- global customization

    /// Look of the launcher: theme, accent, background, zoom, font.
    fn draw_appearance_card(&mut self, ui: &mut egui::Ui) {
        let before = self.state.ui.clone();
        let mut p = before.clone();
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::Palette, "Оформление");
            ui.add_space(6.0);

            ui.label(RichText::new("Тема").color(theme::muted()).size(12.0));
            let modes = [prefs::ThemeMode::Dark, prefs::ThemeMode::Light, prefs::ThemeMode::System];
            let mut sel = modes.iter().position(|m| *m == p.theme).unwrap_or(0);
            if ui::pill_tabs(ui, &[(None, "Тёмная"), (None, "Светлая"), (None, "Как в системе")], &mut sel) {
                p.theme = modes[sel];
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Стиль").color(theme::muted()).size(12.0));
            let styles = [prefs::UiStyle::Pixel, prefs::UiStyle::Soft];
            let mut sel = styles.iter().position(|m| *m == p.style).unwrap_or(0);
            if ui::pill_tabs(ui, &[(None, "Пиксельный"), (None, "Мягкий")], &mut sel) {
                p.style = styles[sel];
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Цвет акцента").color(theme::muted()).size(12.0));
            ui.horizontal_wrapped(|ui| {
                for (name, rgb) in prefs::ACCENT_PRESETS {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                    ui::reg(&format!("accent:{name}"), rect);
                    ui.painter().circle_filled(rect.center(), 13.0, egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                    let ring = ui::anim(ui.ctx(), resp.id.with("ring"), p.accent == rgb && !p.rainbow, 0.18);
                    if ring > 0.01 {
                        ui.painter().circle_stroke(rect.center(), 13.0 + 2.5 * ring, Stroke::new(2.0, theme::text().gamma_multiply(ring)));
                    }
                    if resp.on_hover_text(name).clicked() {
                        p.accent = rgb;
                        p.rainbow = false;
                    }
                }
                // rainbow: a wheel of all hues, turning while it is selected
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
                ui::reg("accent:Радуга", rect);
                let spin = if p.rainbow { (ui.ctx().input(|i| i.time) / 20.0).fract() as f32 } else { 0.0 };
                let (c, r) = (rect.center(), 13.0);
                let n = 12;
                for k in 0..n {
                    let at = |j: f32| {
                        let a = j / n as f32 * std::f32::consts::TAU;
                        c + egui::vec2(a.cos(), a.sin()) * r
                    };
                    let col = theme::hsv_to_rgb(k as f32 / n as f32 + spin, 0.7, 0.95);
                    let pts = vec![c, at(k as f32), at(k as f32 + 0.5), at(k as f32 + 1.0)];
                    ui.painter().add(egui::Shape::convex_polygon(pts, egui::Color32::from_rgb(col[0], col[1], col[2]), Stroke::NONE));
                }
                if p.rainbow {
                    ui.painter().circle_stroke(c, 15.5, Stroke::new(2.0, theme::text()));
                }
                if resp.on_hover_text("Радуга: цвет акцента плавно меняется").clicked() {
                    p.rainbow = !p.rainbow;
                }
                ui.label(RichText::new("свой:").color(theme::muted()).size(12.0));
                if ui.color_edit_button_srgb(&mut p.accent).changed() {
                    p.rainbow = false;
                }
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Фон окна").color(theme::muted()).size(12.0));
            let kinds = [prefs::Background::Plain, prefs::Background::Gradient, prefs::Background::Image];
            let mut sel = kinds.iter().position(|m| *m == p.background).unwrap_or(0);
            if ui::pill_tabs(ui, &[(None, "Однотонный"), (None, "Градиент"), (None, "Своя картинка")], &mut sel) {
                p.background = kinds[sel];
            }
            if p.background == prefs::Background::Image {
                ui.horizontal(|ui| {
                    if ui::secondary(ui, Some(Icon::Plus), "Выбрать картинку…") {
                        if let Some(src) = self.pick_image() {
                            let dest = self.root.join("custom").join("background.png");
                            match images::save_fitted(&src, &dest, 1920) {
                                Ok(()) => {
                                    p.bg_image = Some("background.png".into());
                                    p.bg_luma = images::load_rgba(&dest).ok().map(|(px, _, _)| images::average_luma(&px));
                                    self.bg_tex = None;
                                }
                                Err(e) => self.toast = Some(format!("Не удалось поставить фон: {e}")),
                            }
                        }
                    }
                    ui.label(RichText::new("Затемнение").color(theme::muted()).size(12.0));
                    ui.add(egui::Slider::new(&mut p.bg_dim, 0.0..=0.9).show_value(false));
                });
                if p.bg_image.is_none() {
                    ui.label(RichText::new("Картинка не выбрана — фон будет однотонным.").color(theme::yellow()).size(12.0));
                }
                ui.checkbox(&mut p.auto_text, "Подбирать цвет текста под фон");
                if p.auto_text && p.bg_image.is_some() && p.bg_luma.is_some() {
                    let what = if p.use_dark(true) { "фон тёмный — текст светлый" } else { "фон светлый — текст тёмный" };
                    ui.label(RichText::new(format!("Сейчас: {what}. Тема из списка выше при этом не используется.")).color(theme::muted()).size(12.0));
                }
            }

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut p.animations, "Анимации интерфейса");
                ui.add_enabled(p.animations, egui::Checkbox::new(&mut p.sand, "Песок при прокрутке настроек"));
            });

            ui.add_space(8.0);
            ui.label(RichText::new("Масштаб интерфейса").color(theme::muted()).size(12.0));
            let scales = [0.8f32, 0.9, 1.0, 1.1, 1.25, 1.5];
            let labels: Vec<String> = scales.iter().map(|s| format!("{}%", (s * 100.0).round())).collect();
            let tabs: Vec<(Option<Icon>, &str)> = labels.iter().map(|l| (None, l.as_str())).collect();
            let mut sel = scales.iter().position(|s| (*s - p.scale).abs() < 0.01).unwrap_or(2);
            if ui::pill_tabs(ui, &tabs, &mut sel) {
                p.scale = scales[sel];
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.compact, "Компактный интерфейс");
                ui.add_space(14.0);
                ui.label(RichText::new("Шрифт").color(theme::muted()).size(12.0));
                let fonts = [prefs::FontChoice::Manrope, prefs::FontChoice::System, prefs::FontChoice::Builtin];
                let mut sel = fonts.iter().position(|m| *m == p.font).unwrap_or(0);
                if ui::pill_tabs(ui, &[(None, "Manrope"), (None, "Системный"), (None, "Встроенный")], &mut sel) {
                    p.font = fonts[sel];
                }
            });
            ui.add_space(6.0);
            if ui::secondary(ui, Some(Icon::Refresh), "Сбросить оформление") {
                p.theme = prefs::UiPrefs::default().theme;
                p.style = prefs::UiStyle::default();
                p.accent = prefs::DEFAULT_ACCENT;
                p.background = prefs::Background::Plain;
                p.rainbow = false;
                p.auto_text = true;
                p.animations = true;
                p.sand = true;
                p.scale = 1.0;
                p.compact = false;
                p.font = prefs::FontChoice::default();
            }
        });
        if p != before {
            self.state.ui = p.sanitized();
            Self::apply_prefs(&self.ctx, &self.state.ui);
            self.save_state();
        }
    }

    /// Library view, card size and sorting + game window / after-launch options.
    fn draw_behaviour_card(&mut self, ui: &mut egui::Ui) {
        let before = self.state.ui.clone();
        let mut p = before.clone();
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::Stack, "Библиотека");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let views = [prefs::LibraryView::Grid, prefs::LibraryView::List];
                let mut sel = views.iter().position(|m| *m == p.library_view).unwrap_or(0);
                if ui::pill_tabs(ui, &[(None, "Сетка"), (None, "Список")], &mut sel) {
                    p.library_view = views[sel];
                }
                let sizes = [prefs::CardSize::Small, prefs::CardSize::Medium, prefs::CardSize::Large];
                let mut sel = sizes.iter().position(|m| *m == p.card_size).unwrap_or(1);
                if ui::pill_tabs(ui, &[(None, "Мелкие"), (None, "Средние"), (None, "Крупные")], &mut sel) {
                    p.card_size = sizes[sel];
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Сортировка").color(theme::muted()).size(12.0));
            let sorts = [prefs::SortBy::LastPlayed, prefs::SortBy::Name, prefs::SortBy::Created, prefs::SortBy::Version];
            let mut sel = sorts.iter().position(|m| *m == p.sort).unwrap_or(0);
            if ui::pill_tabs(ui, &[(None, "Недавние"), (None, "По имени"), (None, "По дате создания"), (None, "По версии")], &mut sel) {
                p.sort = sorts[sel];
            }
        });
        ui.add_space(10.0);
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::Rocket, "Запуск игры");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.window.fullscreen, "Во весь экран");
                if !p.window.fullscreen {
                    ui.checkbox(&mut p.window.custom_size, "Свой размер окна");
                    if p.window.custom_size {
                        ui.add(egui::DragValue::new(&mut p.window.width).range(320..=7680).suffix(" px"));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut p.window.height).range(240..=4320).suffix(" px"));
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("После запуска игры").color(theme::muted()).size(12.0));
            let afters = [prefs::AfterLaunch::Keep, prefs::AfterLaunch::Minimize, prefs::AfterLaunch::Close];
            let mut sel = afters.iter().position(|m| *m == p.after_launch).unwrap_or(0);
            if ui::pill_tabs(ui, &[(None, "Оставить открытым"), (None, "Свернуть (вернуть при выходе)"), (None, "Закрыть лаунчер")], &mut sel) {
                p.after_launch = afters[sel];
            }
            ui.add_space(6.0);
            ui.checkbox(&mut p.console_on_launch, "Открывать консоль игры при запуске");
        });
        if p != before {
            self.state.ui = p.sanitized();
            self.save_state();
        }
    }

    /// Nicknames: add, choose the active one, set an avatar, delete.
    fn draw_profiles_card(&mut self, ui: &mut egui::Ui) {
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::User, "Профили (ники)");
            ui.label(
                RichText::new("Активный профиль используется при запуске; у отдельной сборки может быть свой ник.")
                    .color(theme::muted())
                    .size(12.0),
            );
            ui.add_space(6.0);
            let mut activate = None;
            let mut remove = None;
            let mut set_avatar = None;
            for i in 0..self.state.profiles.len() {
                let acc = self.state.profiles[i].clone();
                let active = i == self.state.active_profile;
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::hover());
                    self.paint_profile_avatar(ui, rect, &acc);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&acc.username).family(ui::bold_family()));
                        ui.label(RichText::new(if active { "активный" } else { "" }).color(theme::green()).size(11.5));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.state.profiles.len() > 1 && ui::icon_button(ui, Icon::Trash, 30.0, false, &format!("Удалить «{}»", acc.username)).clicked() {
                            remove = Some(i);
                        }
                        if ui::secondary(ui, None, &format!("Аватар «{}»", acc.username)) {
                            set_avatar = Some(i);
                        }
                        if !active && ui::secondary(ui, None, &format!("Выбрать «{}»", acc.username)) {
                            activate = Some(i);
                        }
                    });
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let resp = ui.add(egui::TextEdit::singleline(&mut self.new_profile_name).desired_width(200.0).hint_text("новый ник"));
                ui::reg("field:new-profile", resp.rect);
                if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Plus), "Добавить профиль", true).clicked() {
                    let name = self.new_profile_name.trim().to_string();
                    if !account::valid_nickname(&name) {
                        self.toast = Some("Ник: 3–16 символов, только латиница, цифры и _".to_string());
                    } else if self.state.profiles.iter().any(|p| p.username == name) {
                        self.toast = Some(format!("Профиль «{name}» уже есть"));
                    } else {
                        self.state.profiles.push(account::Account { username: name, uuid: None, avatar: None, skin: None });
                        self.new_profile_name.clear();
                        self.state.normalize();
                        self.save_state();
                    }
                }
            });
            if let Some(i) = activate {
                self.state.active_profile = i;
                self.state.normalize();
                self.username_input = self.state.profiles[i].username.clone();
                self.save_state();
            }
            if let Some(i) = remove {
                let removed = self.state.profiles.remove(i);
                if self.state.active_profile >= i && self.state.active_profile > 0 {
                    self.state.active_profile -= 1;
                }
                self.state.normalize();
                self.username_input = self.state.active_account().username;
                if let Some(f) = removed.avatar {
                    let _ = std::fs::remove_file(self.root.join("profiles").join(f));
                }
                self.save_state();
            }
            if let Some(i) = set_avatar {
                if let Some(src) = self.pick_image() {
                    let file = format!("{}.png", account::offline_uuid(&self.state.profiles[i].username));
                    match images::save_square(&src, &self.root.join("profiles").join(&file), 128) {
                        Ok(()) => {
                            self.pics.retain(|k, _| !k.starts_with("p:"));
                            self.state.profiles[i].avatar = Some(file);
                            self.state.normalize();
                            self.save_state();
                        }
                        Err(e) => self.toast = Some(format!("Не удалось поставить аватар: {e}")),
                    }
                }
            }
        });
    }

    fn paint_profile_avatar(&mut self, ui: &egui::Ui, rect: egui::Rect, acc: &account::Account) {
        match self.avatar_texture(acc) {
            Some(tex) => ui::paint_picture(ui, rect, &tex),
            None => match acc.skin.as_deref().and_then(|id| self.skin_texture(id)) {
                Some(tex) => paint_skin_head(ui, rect, &tex),
                None => ui::paint_tile(ui, rect, &acc.username),
            },
        }
    }

    fn account_display(&self) -> (String, &'static str) {
        let name = self.state.active_account().username;
        (if name.is_empty() { "Player".to_string() } else { name }, "Офлайн")
    }

    /// Avatar of the active profile (picture or initials tile).
    fn paint_avatar(&mut self, ui: &egui::Ui, rect: egui::Rect, _name: &str) {
        let acc = self.state.active_account();
        self.paint_profile_avatar(ui, rect, &acc);
    }
}

// ------------------------------------------------------------------ Skins (offline)

impl LimeApp {
    /// Texture of a skin of the library (pixels stay sharp).
    fn skin_texture(&mut self, id: &str) -> Option<egui::TextureHandle> {
        let key = format!("s:{id}");
        if let Some(t) = self.pics.get(&key) {
            return t.clone();
        }
        let tex = images::load_rgba(&skins::skin_path(&self.root, id)).ok().map(|(rgba, w, h)| {
            let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            self.ctx.load_texture(format!("skin-{id}"), img, egui::TextureOptions::NEAREST)
        });
        self.pics.insert(key, tex.clone());
        tex
    }

    /// Put a skin into the library (once) and, if asked, give it to the active profile.
    fn add_skin(&mut self, skin: skins::Skin, equip: bool) {
        self.skin_burst = equip;
        match self.state.skins.iter_mut().find(|s| s.id == skin.id) {
            Some(existing) => existing.slim = skin.slim,
            None => self.state.skins.push(skin.clone()),
        }
        self.skin_preview = Some(skin.id.clone());
        if equip {
            self.equip_skin(Some(skin.id));
        } else {
            self.save_state();
        }
    }

    /// Skin of the active profile.
    fn equip_skin(&mut self, id: Option<String>) {
        let i = self.state.active_profile.min(self.state.profiles.len().saturating_sub(1));
        if let Some(p) = self.state.profiles.get_mut(i) {
            p.skin = id;
        }
        self.state.normalize();
        self.save_state();
    }

    fn import_skin_file(&mut self, path: &std::path::Path) {
        match skins::add_file(&self.root, path) {
            Ok(skin) => {
                self.toast = Some(format!("Скин «{}» добавлен", skin.name));
                self.add_skin(skin, true);
            }
            Err(e) => self.toast = Some(format!("Не удалось добавить скин: {e}")),
        }
    }

    fn start_skin_download(&mut self) {
        let nick = self.skin_nick.trim().to_string();
        if !account::valid_nickname(&nick) {
            self.toast = Some("Ник: 3–16 символов, латиница, цифры и _".to_string());
            return;
        }
        let root = self.root.clone();
        let client = install::make_client();
        self.skin_job = Some(start_detached(&client, move |c| {
            let (bytes, slim) = skins::fetch_by_nickname(c, &nick)?;
            skins::add_bytes(&root, &bytes, &nick, Some(slim))
        }));
    }

    fn remove_skin(&mut self, id: &str) {
        self.state.skins.retain(|s| s.id != id);
        skins::remove_file(&self.root, id);
        self.pics.remove(&format!("s:{id}"));
        if self.skin_preview.as_deref() == Some(id) {
            self.skin_preview = None;
        }
        self.state.normalize(); // profiles forget the skin
        self.save_state();
    }

    /// Copy the skin of `account` where the skin mod of `inst` reads it (best effort).
    fn apply_skin(&self, inst: &Instance, account: &account::Account) {
        let Some(id) = account.skin.as_deref() else { return };
        let src = skins::skin_path(&self.root, id);
        if src.is_file() {
            let _ = skins::apply_to_instance(&instance::instance_mines(&self.root, &inst.name), &account.username, &src);
        }
    }

    fn spark_palette() -> [egui::Color32; 5] {
        [theme::green(), theme::green_dark(), egui::Color32::WHITE, egui::Color32::from_rgb(0xf0, 0xd8, 0x84), egui::Color32::from_rgb(0xe8, 0xd2, 0x9a)]
    }

    /// Move and paint the sparks (on top of everything).
    fn paint_sparks(&mut self, ctx: &egui::Context) {
        if self.sparks.is_empty() {
            return;
        }
        let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.1);
        self.sparks.update(dt);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("sparks-fx")));
        self.sparks.paint(&painter, theme::pixel());
        ctx.request_repaint();
    }

    fn draw_skins(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        let active = self.state.active_account();
        ui.horizontal(|ui| {
            ui::heading(ui, "Скины");
            ui.label(RichText::new(format!("профиль «{}»", active.username)).color(theme::muted()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::primary(ui, Some(Icon::Upload), "Загрузить скин") {
                    if let Some(file) = self.pick(Pick::Skin) {
                        self.import_skin_file(&file);
                    }
                }
                if ui::secondary(ui, Some(Icon::Download), "По нику") {
                    self.skin_nick_open = !self.skin_nick_open;
                }
            });
        });
        ui.add_space(8.0);
        if self.skin_nick_open {
            self.draw_skin_by_nick(ui);
            ui.add_space(8.0);
        }
        self.draw_skin_loader_note(ui);
        ui.add_space(12.0);

        let shown = self.skin_preview.clone().or(active.skin.clone());
        ui.horizontal_top(|ui| {
            ui.allocate_ui(egui::vec2(310.0, 540.0), |ui| ui.vertical(|ui| self.draw_skin_preview(ui, shown.as_deref())));
            ui.add_space(14.0);
            ui.vertical(|ui| self.draw_skin_grid(ui, active.skin.as_deref()));
        });
        ui.add_space(20.0);
        self.paint_sparks(ui.ctx());
    }

    fn draw_skin_by_nick(&mut self, ui: &mut egui::Ui) {
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Ник игрока").color(theme::muted()));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.skin_nick).desired_width(220.0).hint_text("например, Notch"));
                ui::reg("field:skin-nick", resp.rect);
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let busy = self.skin_job.is_some();
                let can = !busy && !self.skin_nick.trim().is_empty();
                if (ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Скачать скин", can).clicked() || (enter && can)) && !busy {
                    self.start_skin_download();
                }
                if busy {
                    ui.add(egui::Spinner::new());
                }
            });
            ui.label(
                RichText::new(
                    "Берётся публичный скин игрока с серверов Mojang, входить в аккаунт не нужно. \
                     Если Mojang недоступен из вашей сети, скачайте PNG-файл скина и нажмите «Загрузить скин».",
                )
                .color(theme::muted())
                .size(12.0),
            );
        });
    }

    /// Why a skin may not show up in the game, and whether the mod is there.
    fn draw_skin_loader_note(&mut self, ui: &mut egui::Ui) {
        let status = self.state.instances.get(self.active_index()).map(|inst| {
            let has = skins::has_skin_loader(&instance::instance_mines(&self.root, &inst.name));
            (inst.name.clone(), has)
        });
        let mut find = false;
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                ui::paint_icon(ui, Icon::Info, rect, theme::green_dark());
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 150.0).max(260.0));
                    ui.label(
                        RichText::new(
                            "В офлайне Minecraft сам не показывает свои скины. Скин появится в игре, если в сборке стоит мод CustomSkinLoader \
                             (Fabric, Forge, NeoForge или Quilt): при запуске лаунчер кладёт выбранный скин туда, откуда мод его читает.",
                        )
                        .color(theme::text())
                        .size(13.0),
                    );
                    match &status {
                        Some((name, true)) => {
                            ui.label(RichText::new(format!("В сборке «{name}» мод установлен — скин будет виден.")).color(theme::success()).size(13.0));
                        }
                        Some((name, false)) => {
                            ui.label(RichText::new(format!("В сборке «{name}» мода нет — в игре скин не появится.")).color(theme::yellow()).size(13.0));
                        }
                        None => {
                            ui.label(RichText::new("Сборок пока нет.").color(theme::muted()).size(13.0));
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if status.is_some() && ui::secondary(ui, Some(Icon::Search), "Найти мод") {
                        find = true;
                    }
                });
            });
        });
        if find {
            self.mod_search_query = "CustomSkinLoader".to_string();
            self.content_kind = mods::ContentKind::Mod;
            self.mods_view = ModsView::Catalog;
            self.reset_mod_search();
            self.page = Page::Mods;
            self.search_mods();
        }
    }

    /// Big preview of a skin with the front/back switch.
    fn draw_skin_preview(&mut self, ui: &mut egui::Ui, id: Option<&str>) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(310.0, 400.0), egui::Sense::hover());
        let painter = ui.painter().clone();
        let now = ui.ctx().input(|i| i.time);
        let moving = fx::enabled();
        let sway = if moving { (now * 2.2).sin() as f32 } else { 0.0 };
        let pulse = if moving { 1.0 + 0.03 * (now * 1.4).sin() as f32 } else { 1.0 };
        if theme::pixel() {
            painter.extend(ui::pixel_box_shapes(rect, 4.0, theme::card(), theme::divider(), 1.5));
        } else {
            painter.rect_filled(rect, egui::CornerRadius::same(14), theme::card());
        }
        // soft glow in the accent color behind the figure
        let glow = painter.with_clip_rect(rect.shrink(3.0));
        let center = rect.center() + egui::vec2(0.0, -24.0);
        for k in 0..40 {
            let r = 190.0 * pulse * (1.0 - k as f32 / 40.0);
            glow.circle_filled(center, r, theme::green().gamma_multiply(0.016));
        }
        let skin = id.and_then(|id| self.state.skins.iter().find(|s| s.id == id).cloned());
        let tex = skin.as_ref().and_then(|s| self.skin_texture(&s.id));
        if self.skin_burst {
            self.skin_burst = false;
            if moving {
                self.sparks.burst(rect.center() + egui::vec2(0.0, -30.0), 90, 340.0, &Self::spark_palette());
            }
        }
        // the figure gently bobs up and down
        let area = egui::Rect::from_min_max(rect.left_top() + egui::vec2(20.0, 22.0), rect.right_bottom() - egui::vec2(20.0, 70.0))
            .translate(egui::vec2(0.0, (sway * 2.0).round()));
        // turning around: shrink to a line, swap the side, grow back
        let (shown_view, squash) = match self.skin_flip {
            Some((t0, from)) => {
                let p = fx::progress(now, t0, 0.0, 0.34);
                if p >= 1.0 {
                    self.skin_flip = None;
                    (self.skin_view, 1.0)
                } else {
                    (if p < 0.5 { from } else { self.skin_view }, (std::f32::consts::PI * p).cos().abs().max(0.03))
                }
            }
            None => (self.skin_view, 1.0),
        };
        if moving && skin.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        match (&skin, &tex) {
            (Some(skin), Some(tex)) => paint_skin_figure(ui, area, tex, skin.slim, shown_view, squash),
            _ => {
                painter.text(
                    area.center(),
                    egui::Align2::CENTER_CENTER,
                    if id.is_some() { "Скин не читается" } else { "Скин не выбран" },
                    egui::FontId::new(15.0, ui::bold_family()),
                    theme::muted(),
                );
            }
        }
        // front / back
        let tabs = [(None, "Спереди"), (None, "Сзади")];
        let w = ui::pill_tabs_width(ui, &tabs);
        let strip = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.bottom() - 32.0), egui::vec2(w, 40.0));
        let mut sel = if self.skin_view == skins::View::Front { 0 } else { 1 };
        ui.scope_builder(egui::UiBuilder::new().max_rect(strip).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
            ui::pill_tabs(ui, &tabs, &mut sel);
        });
        let chosen = if sel == 0 { skins::View::Front } else { skins::View::Back };
        if chosen != self.skin_view {
            self.skin_flip = Some((now, self.skin_view));
            self.skin_view = chosen;
        }

        // details of the shown skin
        ui.add_space(8.0);
        if let Some(skin) = skin {
            ui.label(RichText::new(truncate(&skin.name, 30)).family(ui::bold_family()).size(16.0));
            let mut slim = skin.slim;
            if ui.checkbox(&mut slim, "Тонкие руки (модель Alex)").changed() {
                if let Some(s) = self.state.skins.iter_mut().find(|s| s.id == skin.id) {
                    s.slim = slim;
                }
                self.save_state();
            }
            ui.add_space(4.0);
            if ui::button(ui, ui::BtnKind::Danger, Some(Icon::Trash), "Удалить из библиотеки", true).clicked() {
                self.remove_skin(&skin.id);
            }
        } else {
            ui.label(RichText::new("Выберите скин справа или добавьте свой.").color(theme::muted()));
        }
    }

    fn draw_skin_grid(&mut self, ui: &mut egui::Ui, equipped: Option<&str>) {
        ui.label(RichText::new(format!("Библиотека · {}", self.state.skins.len())).family(ui::bold_family()).size(16.0));
        ui.add_space(6.0);
        let list = self.state.skins.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            self.skin_card(ui, None, equipped.is_none(), 0);
            for (i, s) in list.into_iter().enumerate() {
                let on = equipped == Some(s.id.as_str());
                self.skin_card(ui, Some(s), on, i + 1);
            }
        });
        if self.state.skins.is_empty() {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Библиотека пуста. Загрузите PNG-файл скина (64×64) или скачайте скин игрока по нику.")
                    .color(theme::muted()),
            );
        }
    }

    fn skin_card(&mut self, ui: &mut egui::Ui, skin: Option<skins::Skin>, equipped: bool, order: usize) {
        let (hit, resp) = ui.allocate_exact_size(egui::vec2(132.0, 196.0), egui::Sense::click());
        let label = match &skin {
            Some(s) => format!("skin:{}", s.id),
            None => "skin:none".to_string(),
        };
        ui::reg(&label, hit);
        let now = ui.ctx().input(|i| i.time);
        let enter = fx::ease_out_cubic(fx::progress(now, self.page_t0, order as f32 * 0.05, 0.28));
        let pop = ui::anim(ui.ctx(), resp.id.with("pop"), resp.hovered(), 0.14);
        let worn = ui::anim(ui.ctx(), resp.id.with("worn"), equipped, 0.2);
        // flies in from below, lifts when hovered
        let rect = hit.translate(egui::vec2(0.0, (1.0 - enter) * 12.0 - 3.0 * pop));
        let saved_opacity = ui.opacity();
        ui.set_opacity(saved_opacity * enter);
        let border = theme::mix(theme::mix(theme::divider(), theme::muted(), pop), theme::green(), worn);
        let fill = theme::mix(theme::card(), theme::hover_bg(), pop);
        let painter = ui.painter().clone();
        if theme::pixel() {
            painter.extend(ui::pixel_box_shapes(rect, 4.0, fill, border, if equipped { 2.0 } else { 1.5 }));
        } else {
            painter.rect_filled(rect, egui::CornerRadius::same(12), fill);
            painter.rect_stroke(rect, egui::CornerRadius::same(12), Stroke::new(if equipped { 2.0 } else { 1.0 }, border), egui::StrokeKind::Inside);
        }
        let area = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 10.0), egui::vec2(rect.width() - 20.0, 130.0)).expand(3.0 * pop);
        match &skin {
            Some(s) => {
                if let Some(tex) = self.skin_texture(&s.id) {
                    paint_skin_figure(ui, area, &tex, s.slim, skins::View::Front, 1.0);
                }
            }
            None => {
                ui::paint_icon(ui, Icon::Close, egui::Rect::from_center_size(area.center(), egui::vec2(40.0, 40.0)), theme::muted());
            }
        }
        let name = skin.as_ref().map(|s| truncate(&s.name, 14)).unwrap_or_else(|| "Без скина".to_string());
        painter.text(egui::pos2(rect.center().x, rect.bottom() - 46.0), egui::Align2::CENTER_TOP, name, egui::FontId::new(13.5, ui::bold_family()), theme::text());
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() - 25.0),
            egui::Align2::CENTER_TOP,
            if equipped { "используется" } else if skin.is_some() { "надеть" } else { "стандартный вид" },
            egui::FontId::proportional(11.5),
            theme::mix(theme::muted(), theme::green_dark(), worn),
        );
        ui.set_opacity(saved_opacity);
        if resp.clicked() {
            if skin.is_some() && fx::enabled() {
                self.sparks.burst(rect.center(), 70, 320.0, &Self::spark_palette());
            }
            self.skin_preview = skin.as_ref().map(|s| s.id.clone());
            self.equip_skin(skin.map(|s| s.id));
        }
    }
}

/// Flat front or back view of a skin, as large as fits in `area` (whole pixels).
fn paint_skin_figure(ui: &egui::Ui, area: egui::Rect, tex: &egui::TextureHandle, slim: bool, view: skins::View, squash: f32) {
    let [tw, th] = tex.size();
    let unit = (area.width() / 16.0).min(area.height() / 32.0).floor().max(1.0);
    let origin = area.center() - egui::vec2(8.0 * unit, 16.0 * unit);
    // shadow on the "floor"
    ui.painter().add(egui::Shape::ellipse_filled(
        origin + egui::vec2(8.0 * unit, 32.0 * unit + unit * 0.4),
        egui::vec2(6.5 * unit * squash, 0.9 * unit),
        egui::Color32::from_black_alpha(70),
    ));
    let cx = area.center().x;
    for part in skins::parts(view, slim, th == 32) {
        // `squash` < 1 narrows the figure around its middle (turning around)
        let x0 = cx + (part.at[0] - 8.0) * unit * squash;
        let at = egui::Rect::from_min_size(egui::pos2(x0, origin.y + part.at[1] * unit), egui::vec2(part.at[2] * unit * squash, part.at[3] * unit));
        let (mut x0, mut x1) = (part.uv[0] / tw as f32, (part.uv[0] + part.uv[2]) / tw as f32);
        if part.flip {
            std::mem::swap(&mut x0, &mut x1);
        }
        let (y0, y1) = (part.uv[1] / th as f32, (part.uv[1] + part.uv[3]) / th as f32);
        ui.painter().image(tex.id(), at, egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1)), egui::Color32::WHITE);
    }
}

// ------------------------------------------------------------------ Modpacks

impl LimeApp {
    fn draw_modpacks(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if !self.pack_searched && self.pack_search_job.is_none() && self.pack_search_error.is_none() {
            self.search_packs();
        }

        ui.horizontal(|ui| {
            let w = (ui.available_width() - 120.0).max(200.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.pack_search_query)
                    .hint_text("Поиск модпаков (пусто — популярные)")
                    .desired_width(w),
            );
            ui::reg("field:packs-search", resp.rect);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui::primary(ui, Some(Icon::Search), "Найти") || enter {
                self.search_packs();
            }
            if self.pack_search_job.is_some() {
                ui.add(egui::Spinner::new());
            }
        });

        ui.add_space(8.0);
        if let Some(prog) = &self.pack_install_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
            ui.add_space(8.0);
        }
        if let Some(err) = self.pack_search_error.clone() {
            if error_banner(ui, "Не удалось выполнить поиск", &err) {
                self.pack_search_error = None;
                self.search_packs();
            }
            ui.add_space(8.0);
        }

        // install panel for the chosen modpack
        if let Some(hit) = self.selected_pack.and_then(|i| self.pack_results.get(i).cloned()) {
            ui::card().stroke(Stroke::new(1.0, theme::green())).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("Установить «{}»", hit.title)).size(16.0).family(ui::bold_family()));
                ui.label(RichText::new("Имя новой сборки (пусто — как у модпака)").color(theme::muted()).size(12.0));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.pack_install_name)
                            .hint_text(hit.slug.clone().unwrap_or_else(|| hit.project_id.clone()))
                            .desired_width(280.0),
                    );
                    let busy = self.pack_install_job.is_some();
                    if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Скачать и установить", !busy).clicked() {
                        self.start_pack_install(&hit);
                    }
                    if ui::secondary(ui, None, "Отмена") {
                        self.selected_pack = None;
                    }
                });
            });
            ui.add_space(8.0);
        }

        if self.pack_results.is_empty() {
            ui.add_space(30.0);
            let msg = if self.pack_search_job.is_some() {
                "Ищем на Modrinth…"
            } else if self.pack_search_error.is_some() {
                ""
            } else if self.pack_searched {
                "Ничего не найдено. Попробуйте другой запрос."
            } else {
                "Нажмите «Найти» — без запроса покажем популярные модпаки."
            };
            ui.vertical_centered(|ui| {
                if let Some(empty) = &self.assets.empty_state {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 120.0)));
                }
                ui.label(RichText::new(msg).color(theme::muted()));
            });
            return;
        }

        for idx in 0..self.pack_results.len() {
            let hit = self.pack_results[idx].clone();
            let selected = self.selected_pack == Some(idx);
            ui::card()
                .stroke(Stroke::new(1.0, if selected { theme::green() } else { theme::divider() }))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 52.0);
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 160.0).max(200.0));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&hit.title).size(16.0).family(ui::bold_family()));
                                ui.label(RichText::new(format!("от {}", hit.author)).color(theme::muted()).size(12.5));
                            });
                            ui.label(RichText::new(truncate(&hit.description, 140)).color(theme::muted()).size(13.0));
                            ui.label(
                                RichText::new(format!("↓ {} · версий игры: {}", hit.downloads, hit.versions.len()))
                                    .color(theme::muted())
                                    .size(12.0),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let busy = self.pack_install_job.is_some();
                            if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить", !busy).clicked() {
                                self.selected_pack = Some(idx);
                            }
                        });
                    });
                });
            ui.add_space(4.0);
        }
    }

    fn search_packs(&mut self) {
        let query = self.pack_search_query.trim().to_string();
        let client = install::make_client();
        self.pack_search_error = None;
        self.pack_search_job = Some(start_detached(&client, move |c| {
            modrinth::search_filtered(c, &query, "modpack", None, &[], 20, 0)
        }));
    }

    fn start_pack_install(&mut self, hit: &modrinth::SearchHit) {
        let name = self.pack_install_name.trim().to_string();
        let name = if name.is_empty() {
            hit.slug.clone().unwrap_or_else(|| hit.project_id.clone())
        } else {
            name
        };
        let slug = hit.slug.clone().unwrap_or_else(|| hit.project_id.clone());
        let instance = Instance {
            name: name.clone(),
            game_version: "latest".to_string(),
            loader: String::new(),
            loader_version: String::new(),
            version_name: None,
            modpack_slug: Some(slug),
            created: now_string(),
            ..Default::default()
        };
        if self.state.instances.iter().any(|i| instance::sanitize(&i.name) == instance::sanitize(&name)) {
            self.toast = Some(format!("Сборка «{name}» уже существует — выберите другое имя"));
            return;
        }

        let project_id = hit.project_id.clone();
        let icon_url = hit.icon_url.clone().filter(|u| !u.is_empty());
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        let job = start_detached(&client, move |c| {
            // resolve the real .mrpack URL through the API (the CDN
            // `latest.mrpack` alias answers 400), then install it.
            let pack = modrinth::latest_mrpack_url(c, &project_id)?;
            let mut inst = install::install_mrpack(c, &root, &instance, &pack, Some(&prog_ref))?;
            // the modpack's icon becomes the instance's icon (best effort)
            if let Some(url) = icon_url {
                if let Ok((rgba, w, h)) = images::cached_icon(c, &url, &root.join("cache").join("icons"), 256) {
                    let dest = instance::instance_dir(&root, &inst.name).join("icon.png");
                    if images::save_rgba_png(&rgba, w, h, &dest).is_ok() {
                        inst.icon = Some("icon.png".into());
                    }
                }
            }
            Ok(inst)
        });
        self.pack_install_prog = Some(prog);
        self.pack_install_job = Some(job);
    }
}

// ------------------------------------------------------------------ Mods

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} МБ", bytes as f64 / 1048576.0)
    } else {
        format!("{} КБ", (bytes / 1024).max(1))
    }
}

impl LimeApp {
    /// Вкладка «Каталог»: каталог, установленное и модпаки.
    fn draw_mods(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui::heading(ui, "Каталог Modrinth");
        ui.add_space(8.0);
        let views = [ModsView::Catalog, ModsView::Installed, ModsView::Modpacks];
        let mut sel = views.iter().position(|v| *v == self.mods_view).unwrap_or(0);
        let tabs = [(Some(Icon::Compass), "Каталог"), (Some(Icon::Cube), "Установленные"), (Some(Icon::Download), "Модпаки")];
        if ui::pill_tabs(ui, &tabs, &mut sel) {
            self.mods_view = views[sel];
        }
        ui.add_space(10.0);
        match self.mods_view {
            ModsView::Modpacks => self.draw_modpacks(ui),
            ModsView::Catalog => {
                self.draw_content_header(ui);
                self.draw_mods_catalog(ui);
            }
            ModsView::Installed => {
                self.draw_content_header(ui);
                self.draw_installed(ui);
            }
        }
    }

    /// Целевая сборка + тип содержимого (моды / ресурспаки / шейдеры).
    fn draw_content_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Сборка").color(theme::muted()));
            if self.state.instances.is_empty() {
                ui.label(RichText::new("нет — создайте на вкладке «Сборки»").color(theme::muted()));
            } else {
                let names: Vec<String> = self.state.instances.iter().map(|i| i.name.clone()).collect();
                let mut sel = self.active_index();
                egui::ComboBox::from_id_salt("modTargetInstance")
                    .selected_text(names[sel].clone())
                    .show_ui(ui, |ui| {
                        for (idx, n) in names.iter().enumerate() {
                            if ui.selectable_label(idx == sel, n.clone()).clicked() {
                                sel = idx;
                            }
                        }
                    });
                if sel != self.active_instance {
                    self.active_instance = sel;
                    self.reset_mod_search();
                }
            }
            ui.add_space(12.0);
            let kinds = mods::ContentKind::ALL;
            let mut ksel = kinds.iter().position(|k| *k == self.content_kind).unwrap_or(0);
            let labels: Vec<(Option<Icon>, &str)> = kinds.iter().map(|k| (None, k.label())).collect();
            if ui::pill_tabs(ui, &labels, &mut ksel) {
                self.content_kind = kinds[ksel];
                self.reset_mod_search();
            }
        });
        if let Some(inst) = self.state.instances.get(self.active_index()) {
            if self.content_kind == mods::ContentKind::Mod && inst.loader.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("У этой сборки нет загрузчика (vanilla): моды в ней работать не будут. Создайте сборку с Fabric, Quilt, Forge или NeoForge.")
                        .color(theme::yellow())
                        .size(12.5),
                );
            }
        }
        ui.add_space(8.0);
    }

    fn draw_mods_catalog(&mut self, ui: &mut egui::Ui) {
        let kind = self.content_kind;
        // First open (or after switching instance/type): show popular items right away.
        if !self.mod_searched && self.mod_search_job.is_none() && self.mod_search_error.is_none() {
            self.search_mods();
        }
        let busy = self.mod_install_job.is_some();
        ui.horizontal(|ui| {
            let hint = match kind {
                mods::ContentKind::Mod => "Поиск модов (пусто — популярные)",
                mods::ContentKind::ResourcePack => "Поиск ресурспаков",
                mods::ContentKind::Shader => "Поиск шейдеров",
            };
            let w = (ui.available_width() - 330.0).max(180.0);
            let resp = ui.add(egui::TextEdit::singleline(&mut self.mod_search_query).hint_text(hint).desired_width(w));
            ui::reg("field:mods-search", resp.rect);
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui::primary(ui, Some(Icon::Search), "Найти") || enter {
                self.search_mods();
            }
            ui.checkbox(&mut self.only_compatible, "Только совместимые");
            if self.mod_search_job.is_some() {
                ui.add(egui::Spinner::new());
            }
        });

        ui.add_space(8.0);
        if let Some(prog) = &self.mod_install_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
            ui.add_space(8.0);
        }
        if let Some(report) = &self.mod_install_report {
            ui.label(RichText::new(report).color(theme::success()));
            ui.add_space(6.0);
        }

        if let Some(err) = self.mod_search_error.clone() {
            if error_banner(ui, "Не удалось выполнить поиск", &err) {
                self.mod_search_error = None;
                self.search_mods();
            }
            ui.add_space(8.0);
        }

        if self.mod_results.is_empty() {
            ui.add_space(30.0);
            let msg = if self.mod_search_job.is_some() {
                "Ищем на Modrinth…"
            } else if self.mod_search_error.is_some() {
                ""
            } else if self.mod_searched {
                "Ничего не найдено. Попробуйте другой запрос или снимите «Только совместимые»."
            } else {
                "Нажмите «Найти» — без запроса покажем популярное."
            };
            ui.vertical_centered(|ui| {
                if let Some(empty) = &self.assets.empty_state {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 120.0)));
                }
                ui.label(RichText::new(msg).color(theme::muted()));
            });
            return;
        }

        for idx in 0..self.mod_results.len() {
            let hit = self.mod_results[idx].clone();
            let selected = self.selected_mod == Some(idx);
            let frame = ui::card().stroke(Stroke::new(1.0, if selected { theme::green() } else { theme::divider() }));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 52.0);
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 200.0).max(200.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&hit.title).size(16.0).family(ui::bold_family()));
                            ui.label(RichText::new(format!("от {}", hit.author)).color(theme::muted()).size(12.5));
                        });
                        ui.label(RichText::new(truncate(&hit.description, 140)).color(theme::muted()).size(13.0));
                        let is_release = |v: &&String| v.chars().all(|c| c.is_ascii_digit() || c == '.');
                        let latest: Vec<&str> = hit.versions.iter().filter(is_release).rev().take(4).map(|s| s.as_str()).collect();
                        ui.label(
                            RichText::new(format!("↓ {} · для {}", hit.downloads, latest.join(", ")))
                                .color(theme::muted())
                                .size(12.0),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить", !busy).clicked() {
                            self.quick_install(&hit);
                        }
                        if ui::secondary(ui, None, "Версии") {
                            self.selected_mod = Some(idx);
                            self.load_mod_versions(&hit);
                        }
                    });
                });
            });
            ui.add_space(4.0);
        }

        // version picker for the project chosen with «Версии»
        if let Some(hit) = self.selected_mod.and_then(|i| self.mod_results.get(i).cloned()) {
            ui.add_space(8.0);
            ui::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Версии «{}»", hit.title)).size(16.0).family(ui::bold_family()));
                    if self.mod_versions_job.is_some() {
                        ui.add(egui::Spinner::new());
                    }
                });
                if !self.mod_versions.is_empty() {
                    let mut sel = self.selected_mod_version.unwrap_or(0).min(self.mod_versions.len() - 1);
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("modVersion")
                            .width(520.0)
                            .selected_text(self.mod_versions.get(sel).map(version_label).unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (vidx, v) in self.mod_versions.iter().enumerate() {
                                    if ui.selectable_label(vidx == sel, version_label(v)).clicked() {
                                        sel = vidx;
                                    }
                                }
                            });
                        if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить выбранную", !busy).clicked() {
                            self.selected_mod_version = Some(sel);
                            self.start_mod_install();
                        }
                    });
                    self.selected_mod_version = Some(sel);
                } else if self.mod_versions_job.is_none() {
                    ui.label(
                        RichText::new("Нет версий, подходящих этой сборке. Снимите «Только совместимые» или выберите другую сборку.")
                            .color(theme::muted())
                            .size(13.0),
                    );
                }
            });
        }
    }

    /// One-click install: newest version compatible with the target instance.
    fn quick_install(&mut self, hit: &modrinth::SearchHit) {
        let Some(target) = self.state.instances.get(self.active_index()).cloned() else {
            self.toast = Some("Сначала создайте сборку".to_string());
            return;
        };
        let (id, kind, only) = (hit.project_id.clone(), self.content_kind, self.only_compatible);
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        self.mod_install_job = Some(start_detached(&client, move |c| {
            let versions = if !only || target.game_version.is_empty() || target.game_version == "latest" {
                modrinth::get_project_versions(c, &id)?
            } else {
                let loaders = if kind.uses_loader() { mods::mod_loaders(&target) } else { Vec::new() };
                modrinth::get_versions_for(c, &id, &target.game_version, &loaders)?
            };
            let version = versions
                .into_iter()
                .next()
                .ok_or_else(|| format!("нет версии под Minecraft {} для этой сборки", target.game_version))?;
            mods::install(c, &root, &target, kind, &version, Some(&prog_ref))
        }));
        self.mod_install_prog = Some(prog);
        self.mod_install_report = None;
    }

    fn search_mods(&mut self) {
        let query = self.mod_search_query.trim().to_string();
        let kind = self.content_kind;
        let target = self.state.instances.get(self.active_index()).cloned();
        let client = install::make_client();
        let only = self.only_compatible;
        self.mod_search_job = Some(start_detached(&client, move |c| {
            let (game, loaders) = match (&target, only) {
                (Some(t), true) => (
                    Some(t.game_version.clone()),
                    if kind.uses_loader() { mods::mod_loaders(t) } else { Vec::new() },
                ),
                _ => (None, Vec::new()),
            };
            modrinth::search_filtered(c, &query, kind.project_type(), game.as_deref(), &loaders, 20, 0)
        }));
    }

    fn load_mod_versions(&mut self, hit: &modrinth::SearchHit) {
        let Some(target) = self.state.instances.get(self.active_index()).cloned() else {
            self.mod_versions.clear();
            self.selected_mod_version = None;
            return;
        };
        let id = hit.project_id.clone();
        let kind = self.content_kind;
        let only = self.only_compatible;
        let client = install::make_client();
        self.mod_versions_job = Some(start_detached(&client, move |c| {
            if !only || target.game_version.is_empty() || target.game_version == "latest" {
                return modrinth::get_project_versions(c, &id);
            }
            let loaders = if kind.uses_loader() { mods::mod_loaders(&target) } else { Vec::new() };
            modrinth::get_versions_for(c, &id, &target.game_version, &loaders)
        }));
        self.mod_versions.clear();
        self.selected_mod_version = None;
        self.mod_install_report = None;
    }

    fn start_mod_install(&mut self) {
        let Some(version) = self.selected_mod_version.and_then(|i| self.mod_versions.get(i).cloned()) else {
            self.toast = Some("Сначала выберите версию".to_string());
            return;
        };
        let Some(target) = self.state.instances.get(self.active_index()).cloned() else {
            self.toast = Some("Нет сборки для установки".to_string());
            return;
        };
        let kind = self.content_kind;
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        self.mod_install_job = Some(start_detached(&client, move |c| {
            mods::install(c, &root, &target, kind, &version, Some(&prog_ref))
        }));
        self.mod_install_prog = Some(prog);
        self.mod_install_report = None;
    }

    // ---- «Установленные»: список, включить/выключить, удалить, обновить ----

    fn load_installed(&mut self, inst: &Instance) {
        let kind = self.content_kind;
        self.installed_for = Some((inst.name.clone(), kind));
        self.updates.clear();
        let root = self.root.clone();
        let name = inst.name.clone();
        let client = install::make_client();
        self.installed_job = Some(start_detached(&client, move |c| {
            let mut items = mods::scan(&root, &name, kind)?;
            let _ = mods::identify(c, &mut items); // offline is fine: names only
            Ok(items)
        }));
    }

    fn check_updates(&mut self, inst: &Instance) {
        let items = self.installed_items.clone();
        let inst = inst.clone();
        let kind = self.content_kind;
        let client = install::make_client();
        self.updates_job = Some(start_detached(&client, move |c| mods::check_updates(c, &items, &inst, kind)));
    }

    /// Update one file (`only = Some(file_name)`) or everything that has an update.
    fn apply_updates(&mut self, inst: &Instance, only: Option<&str>) {
        let pairs: Vec<(mods::InstalledItem, modrinth::Version)> = self
            .updates
            .iter()
            .filter(|u| only.map(|o| o == u.file_name).unwrap_or(true))
            .filter_map(|u| {
                self.installed_items
                    .iter()
                    .find(|i| i.file_name == u.file_name)
                    .map(|i| (i.clone(), u.new_version.clone()))
            })
            .collect();
        if pairs.is_empty() {
            return;
        }
        let (inst, kind, root) = (inst.clone(), self.content_kind, self.root.clone());
        let client = install::make_client();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        self.update_job = Some(start_detached(&client, move |c| {
            let mut n = 0;
            for (item, version) in &pairs {
                mods::apply_update(c, &root, &inst, kind, item, version, Some(&prog_ref))?;
                n += 1;
            }
            Ok(format!("Обновлено файлов: {n}"))
        }));
        self.mod_install_prog = Some(prog);
    }

    fn toggle_item(&mut self, idx: usize, enabled: bool) {
        let Some(item) = self.installed_items.get(idx).cloned() else { return };
        match mods::set_enabled(&item, enabled) {
            Ok(path) => {
                let new_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                for u in self.updates.iter_mut().filter(|u| u.file_name == item.file_name) {
                    u.file_name = new_name.clone();
                }
                let it = &mut self.installed_items[idx];
                it.path = path;
                it.file_name = new_name;
                it.enabled = enabled;
            }
            Err(e) => self.toast = Some(format!("Не удалось переключить: {e}")),
        }
    }

    fn remove_item(&mut self, idx: usize) {
        let Some(item) = self.installed_items.get(idx).cloned() else { return };
        match mods::remove(&item) {
            Ok(()) => {
                self.installed_items.remove(idx);
                self.updates.retain(|u| u.file_name != item.file_name);
                self.toast = Some(format!("Удалён {}", item.file_name));
            }
            Err(e) => self.toast = Some(format!("Не удалось удалить: {e}")),
        }
    }

    fn draw_installed(&mut self, ui: &mut egui::Ui) {
        let Some(inst) = self.state.instances.get(self.active_index()).cloned() else {
            ui.label(RichText::new("Сначала создайте сборку на вкладке «Сборки».").color(theme::muted()));
            return;
        };
        let key = (inst.name.clone(), self.content_kind);
        if self.installed_for.as_ref() != Some(&key) && self.installed_job.is_none() {
            self.load_installed(&inst);
        }

        ui.horizontal(|ui| {
            if ui::secondary(ui, Some(Icon::Refresh), "Обновить список") && self.installed_job.is_none() {
                self.load_installed(&inst);
            }
            let can_check = !self.installed_items.is_empty() && self.updates_job.is_none() && self.update_job.is_none();
            if ui::button(ui, ui::BtnKind::Secondary, Some(Icon::Up), "Проверить обновления", can_check).clicked() {
                self.check_updates(&inst);
            }
            if !self.updates.is_empty() && self.update_job.is_none() {
                if ui::primary(ui, Some(Icon::Download), &format!("Обновить всё ({})", self.updates.len())) {
                    self.apply_updates(&inst, None);
                }
            }
            if self.installed_job.is_some() || self.updates_job.is_some() || self.update_job.is_some() {
                ui.add(egui::Spinner::new());
            }
        });
        if self.update_job.is_some() {
            if let Some(prog) = &self.mod_install_prog {
                ui.add_space(6.0);
                self.install_progress_bar(ui, prog, Some(&prog.cancelled));
            }
        }
        ui.add_space(8.0);

        if self.installed_items.is_empty() {
            if self.installed_job.is_none() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    if let Some(empty) = &self.assets.empty_state {
                        ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 120.0)));
                    }
                    ui.label(
                        RichText::new(format!("В сборке «{}» пока нет файлов ({}).", inst.name, self.content_kind.label()))
                            .color(theme::muted()),
                    );
                });
            }
            return;
        }

        for idx in 0..self.installed_items.len() {
            let item = self.installed_items[idx].clone();
            let update = self.updates.iter().find(|u| u.file_name == item.file_name).cloned();
            ui::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut en = item.enabled;
                    if ui.checkbox(&mut en, "").on_hover_text("Включён / выключен").changed() {
                        self.toggle_item(idx, en);
                    }
                    let tile_name = item.title.clone().unwrap_or_else(|| item.file_name.clone());
                    self.project_icon(ui, &tile_name, item.icon_url.as_deref(), 40.0);
                    ui.vertical(|ui| {
                        let color = if item.enabled { theme::text() } else { theme::muted() };
                        ui.label(RichText::new(truncate(&tile_name, 60)).size(15.0).family(ui::bold_family()).color(color));
                        let mut meta = vec![format_size(item.size)];
                        if item.title.is_some() {
                            meta.insert(0, truncate(&item.file_name, 48));
                        }
                        if let Some(v) = &item.version_number {
                            meta.insert(1, format!("v{v}"));
                        }
                        if item.title.is_none() {
                            meta.push("не найден на Modrinth".to_string());
                        }
                        ui.label(RichText::new(meta.join(" · ")).color(theme::muted()).size(12.0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(ui, Icon::Trash, 34.0, false, "Удалить файл").clicked() {
                            self.remove_item(idx);
                        }
                        if let Some(u) = &update {
                            let busy = self.update_job.is_some();
                            if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Up), &u.new_version.version_number, !busy).clicked() {
                                self.apply_updates(&inst, Some(&u.file_name));
                            }
                        }
                    });
                });
            });
            ui.add_space(4.0);
        }
    }
}

fn version_label(v: &modrinth::Version) -> String {
    let games: Vec<&str> = v.game_versions.iter().rev().take(3).map(|s| s.as_str()).collect();
    let loaders = if v.loaders.is_empty() { "any".to_string() } else { v.loaders.join("/") };
    format!("{} · {} · {} · {}", v.version_number, v.version_type, games.join(","), loaders)
}

// ------------------------------------------------------------------ Settings

impl LimeApp {
    /// Вкладка «Другое» (ресурспаки, шейдеры, глобальные настройки).
    fn draw_other(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui::heading(ui, "Настройки");
        ui.label(RichText::new("Профили, оформление, запуск игры, Java и память.").color(theme::muted()).size(13.0));
        ui.add_space(12.0);
        self.draw_profiles_card(ui);
        ui.add_space(12.0);
        self.draw_appearance_card(ui);
        ui.add_space(12.0);
        self.draw_behaviour_card(ui);
        ui.add_space(12.0);

        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Папка данных").family(ui::bold_family()));
            ui.label(RichText::new(self.root.display().to_string()).color(theme::muted()).size(12.5));
            ui.add_space(6.0);
            if ui::secondary(ui, Some(Icon::Folder), "Открыть папку") {
                open_in_explorer(&self.root);
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new("Ресурспаки и шейдеры устанавливаются на странице сборки или в каталоге Modrinth.")
                    .color(theme::muted())
                    .size(12.5),
            );
        });
        ui.add_space(12.0);

        ui::card()
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui::section(ui, Icon::Memory, "Ник, Java и память");
                ui.add_space(6.0);

                ui.label(RichText::new("Ник активного профиля").color(theme::muted()));
                let resp = ui.add(egui::TextEdit::singleline(&mut self.username_input).desired_width(220.0).hint_text("Player"));
                ui::reg("field:nickname", resp.rect);
                let typed = self.username_input.trim().to_string();
                if !typed.is_empty() && !account::valid_nickname(&typed) {
                    ui.label(RichText::new("3–16 символов: латиница, цифры и _").color(theme::yellow()).size(12.0));
                }
                ui.label(
                    RichText::new(format!("UUID для этого ника: {}", account::offline_uuid(&typed)))
                        .color(theme::muted())
                        .size(12.0),
                );
                ui.label(
                    RichText::new(
                        "Офлайн-аккаунт: одиночная игра, LAN и серверы с online-mode=false. \
                         Серверы с проверкой лицензии такой аккаунт не пустят.",
                    )
                    .color(theme::muted())
                    .size(12.0),
                );

                ui.add_space(14.0);
                ui.label(RichText::new((if cfg!(windows) { "Путь к Java (java.exe)" } else { "Путь к Java (java)" })).family(ui::bold_family()).color(theme::text()));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.java_input).desired_width(380.0).hint_text(if cfg!(windows) { "C:\\...\\java.exe" } else { "/usr/lib/jvm/.../bin/java" }));
                    if ui::secondary(ui, None, "Автоопределить") {
                        match launch::find_java_path(None, None) {
                            Ok(java) => {
                                self.java_input = java.display().to_string();
                                self.toast = Some(format!("Найдено: {}", self.java_input));
                            }
                            Err(_) => self.toast = Some("Java не найдена автоматически".to_string()),
                        }
                    }
                });

                ui.add_space(14.0);
                ui.label(RichText::new("Память по умолчанию (MB)").family(ui::bold_family()).color(theme::text()));
                ui.horizontal(|ui| {
                    for preset in prefs::MEMORY_PRESETS.iter() {
                        if ui::secondary(ui, None, &format!("{} · {} ГБ", preset.name, preset.memory_mb / 1024)) {
                            self.ram_input = preset.memory_mb.to_string();
                        }
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut self.ram_input).desired_width(160.0).hint_text("4096"));

                ui.add_space(16.0);
                if ui::primary(ui, None, "Сохранить") {
                    self.toast = None;
                    self.apply_settings_to_state();
                    match instance::save_state(&self.root, &self.state) {
                        Ok(()) => {
                            if self.toast.is_none() {
                                self.toast = Some("Настройки сохранены".to_string());
                            }
                        }
                        Err(e) => self.toast = Some(format!("Ошибка сохранения: {e}")),
                    }
                }
            });

        ui.add_space(12.0);
        self.draw_network_card(ui);
        ui.add_space(16.0);
        ui.label(RichText::new(format!("LimeCraft {}", env!("CARGO_PKG_VERSION"))).color(theme::muted()).size(12.0));
        ui.add_space(24.0);
    }

    fn apply_settings_to_state(&mut self) {
        let java = self.java_input.trim();
        self.state.java_path = if java.is_empty() { None } else { Some(java.to_string()) };
        match self.ram_input.trim().parse::<u64>() {
            Ok(mb) if (512..=65536).contains(&mb) => self.state.ram_mb = mb,
            _ => {
                self.toast = Some("Память: введите число от 512 до 65536 МБ (оставлено прежнее значение)".to_string());
                if self.state.ram_mb == 0 {
                    self.state.ram_mb = 4096;
                }
            }
        }
        let name = self.username_input.trim().to_string();
        let active = self.state.active_profile;
        if account::valid_nickname(&name) {
            if let Some(acc) = self.state.profiles.get_mut(active) {
                acc.username = name;
            }
            self.state.normalize();
        } else {
            self.toast = Some("Ник: 3–16 символов, только латиница, цифры и _ (прежний ник сохранён)".to_string());
        }
    }
}

// ------------------------------------------------------------------ New instance dialog

impl LimeApp {
    fn open_new_instance_dialog(&mut self) {
        self.open_new_instance_tab(0);
    }

    /// Open the "new build" dialog on a tab: 0 new, 1 folder, 2 file, 3 other launchers.
    fn open_new_instance_tab(&mut self, tab: usize) {
        self.show_new_instance = true;
        self.new_tab = tab;
        self.new_instance_name = self.unique_name("Моя сборка");
        if self.versions_list.is_empty() && self.versions_job.is_none() {
            let client = install::make_client();
            self.versions_job = Some(start_detached(&client, |c| {
                mojang::fetch_manifest(c).map(|m| m.versions)
            }));
        }
        self.new_instance_version = if self.selected_version == "latest" {
            String::new()
        } else {
            self.selected_version.clone()
        };
        self.new_instance_loader = String::new();
        self.new_instance_prog = None;
        self.new_instance_job = None;
        self.new_instance_result = None;
        self.reset_import();
        self.launchers_scanned = false;
    }

    fn reset_import(&mut self) {
        self.import_src = None;
        self.import_info = None;
        self.import_error = None;
    }

    /// Game version and loader pickers shared by "new" and "from folder".
    fn draw_version_loader(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Версия Minecraft").color(theme::muted()));
        ui.checkbox(&mut self.show_snapshots, "Показывать снапшоты");
        if self.versions_job.is_some() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Загрузка списка версий…").color(theme::muted()));
            });
        }
        if !self.versions_list.is_empty() {
            let mut v = self.new_instance_version.clone();
            egui::ComboBox::from_label("Версия для инстанса")
                .selected_text(v.clone())
                .show_ui(ui, |ui| {
                    for entry in mojang::filter_versions(&self.versions_list, self.show_snapshots) {
                        if ui.selectable_label(entry.id == v, format!("{} ▸ {}", entry.id, entry.kind)).clicked() {
                            v = entry.id.clone();
                        }
                    }
                });
            self.new_instance_version = v;
        } else {
            ui.add(egui::TextEdit::singleline(&mut self.new_instance_version).hint_text("Например 1.21.6").desired_width(300.0));
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Мод-лоадер").color(theme::muted()));
        let loaders = [
            ("", "None — чистая vanilla"),
            ("fabric", "Fabric"),
            ("quilt", "Quilt"),
            ("forge", "Forge"),
            ("neoforge", "NeoForge"),
        ];
        let cur = self.new_instance_loader.clone();
        egui::ComboBox::from_label("Лоадер")
            .selected_text(loaders.iter().find(|(v, _)| *v == cur).map(|(_, l)| *l).unwrap_or("None — чистая vanilla"))
            .show_ui(ui, |ui| {
                for (val, label) in loaders {
                    if ui.selectable_label(cur == val, label).clicked() {
                        self.new_instance_loader = val.to_string();
                    }
                }
            });
        if self.new_instance_loader == "forge" || self.new_instance_loader == "neoforge" {
            ui.label(RichText::new("Установится через официальный installer (FML-профиль).").color(theme::yellow()).size(12.0));
        }
    }

    /// Primary action + "Отмена" at the bottom of a dialog tab.
    fn dialog_actions(&mut self, ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> bool {
        let mut go = false;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            go = ui::button(ui, ui::BtnKind::Primary, Some(icon), label, enabled).clicked();
            if ui::secondary(ui, None, "Отмена") {
                self.show_new_instance = false;
            }
        });
        go
    }

    fn draw_new_instance_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Новая сборка")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(480.0)
            .show(ctx, |ui| {
                let busy = self.new_instance_job.is_some();
                if !busy && self.new_instance_result.is_none() {
                    let mut tab = self.new_tab;
                    let tabs = [(Some(Icon::Plus), "Новая"), (Some(Icon::Folder), "Из папки"), (Some(Icon::Import), "Из файла"), (Some(Icon::Stack), "Из лаунчеров")];
                    if ui::pill_tabs(ui, &tabs, &mut tab) {
                        self.new_tab = tab;
                        self.reset_import();
                        if tab == 0 || self.new_instance_name.trim().is_empty() {
                            self.new_instance_name = self.unique_name("Моя сборка");
                        }
                    }
                    ui.add_space(10.0);
                    match self.new_tab {
                        0 => self.draw_tab_new(ui),
                        1 => self.draw_tab_folder(ui),
                        2 => self.draw_tab_file(ui),
                        _ => self.draw_tab_launchers(ui),
                    }
                }

                if let Some(prog) = &self.new_instance_prog {
                    ui.add_space(8.0);
                    self.install_progress_bar(ui, prog, Some(&prog.cancelled));
                }
                if busy {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(RichText::new("Установка…").color(theme::muted()));
                    });
                }

                match &self.new_instance_result {
                    Some(Ok(())) => {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Сборка добавлена ✓").color(theme::success()));
                        if ui::primary(ui, Some(Icon::Check), "Готово") {
                            self.show_new_instance = false;
                            self.new_instance_result = None;
                        }
                    }
                    Some(Err(e)) => {
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("Ошибка: {e}")).color(theme::danger()));
                        if ui::secondary(ui, None, "Закрыть") {
                            self.show_new_instance = false;
                            self.new_instance_result = None;
                        }
                    }
                    None => {}
                }
            });
    }

    fn draw_tab_new(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Имя сборки").color(theme::muted()));
        ui.add(egui::TextEdit::singleline(&mut self.new_instance_name).desired_width(300.0));
        ui.add_space(6.0);
        self.draw_version_loader(ui);
        if self.dialog_actions(ui, Icon::Download, "Установить", true) {
            self.start_new_instance();
        }
    }

    fn draw_tab_folder(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Папка с игрой: .minecraft, папка сборки из любого лаунчера или любая другая, где лежат mods, config, saves.").color(theme::muted()).size(12.5));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui::secondary(ui, Some(Icon::FolderOpen), "Выбрать папку…") {
                if let Some(dir) = self.pick(Pick::Folder) {
                    self.set_import_folder(dir);
                }
            }
            let shown = self.import_src.as_ref().map(|p| truncate(&p.display().to_string(), 44)).unwrap_or_else(|| "папка не выбрана".into());
            ui.label(RichText::new(shown).color(theme::muted()));
        });
        if self.import_src.is_some() {
            ui.add_space(6.0);
            ui.label(RichText::new("Имя сборки").color(theme::muted()));
            let resp = ui.add(egui::TextEdit::singleline(&mut self.new_instance_name).desired_width(300.0));
            ui::reg("field:import-name", resp.rect);
            ui.add_space(6.0);
            self.draw_version_loader(ui);
            ui.add_space(4.0);
            ui.checkbox(&mut self.import_worlds, "Копировать миры (saves)");
            ui.label(
                RichText::new("Копируются моды, настройки, ресурспаки, шейдеры, options.txt и servers.dat. Игру и загрузчик лаунчер скачает сам; исходная папка не меняется.")
                    .color(theme::muted())
                    .size(12.0),
            );
        }
        let ready = self.import_src.is_some();
        if self.dialog_actions(ui, Icon::Import, "Импортировать", ready) {
            self.start_import_folder();
        }
    }

    fn set_import_folder(&mut self, dir: PathBuf) {
        // `.minecraft` says nothing: name the build after the folder above
        let mut named = dir.clone();
        if dir.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false) {
            if let Some(parent) = dir.parent() {
                named = parent.to_path_buf();
            }
        }
        let base = named.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Импорт".into());
        self.new_instance_name = self.unique_name(&base);
        self.import_src = Some(dir);
    }

    fn draw_tab_file(&mut self, ui: &mut egui::Ui) {
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.iter().next().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.set_import_archive(path);
        }
        ui.horizontal(|ui| {
            if ui::secondary(ui, Some(Icon::Import), "Выбрать файл…") {
                if let Some(file) = self.pick(Pick::Archive) {
                    self.set_import_archive(file);
                }
            }
            ui.label(RichText::new("или перетащите файл в окно").color(theme::muted()));
        });
        ui.label(RichText::new("Подходят: сборки Modrinth (.mrpack) и экспорт из Prism / MultiMC (.zip).").color(theme::muted()).size(12.5));
        if let Some(e) = &self.import_error {
            ui.add_space(6.0);
            ui.label(RichText::new(e).color(theme::danger()));
        }
        if let Some(info) = self.import_info.clone() {
            ui.add_space(8.0);
            let kind = match info.kind {
                import::ArchiveKind::Mrpack => "Сборка Modrinth",
                import::ArchiveKind::Mmc => "Экспорт Prism / MultiMC",
            };
            let loader = if info.loader.is_empty() { String::new() } else { format!(" · {}", loader_title(&info.loader)) };
            let game = if info.game_version.is_empty() { "версия из сборки".to_string() } else { format!("Minecraft {}", info.game_version) };
            ui::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(kind).family(ui::bold_family()));
                ui.label(RichText::new(format!("{game}{loader}")).color(theme::muted()));
            });
            ui.add_space(6.0);
            ui.label(RichText::new("Имя сборки").color(theme::muted()));
            let resp = ui.add(egui::TextEdit::singleline(&mut self.new_instance_name).desired_width(300.0));
            ui::reg("field:import-name", resp.rect);
            if info.kind == import::ArchiveKind::Mmc {
                ui.checkbox(&mut self.import_worlds, "Копировать миры (saves)");
            }
        }
        let ready = self.import_info.is_some();
        if self.dialog_actions(ui, Icon::Import, "Импортировать", ready) {
            self.start_import_archive();
        }
    }

    fn set_import_archive(&mut self, path: PathBuf) {
        match import::inspect_archive(&path) {
            Ok(info) => {
                self.new_instance_name = self.unique_name(&info.name);
                self.import_src = Some(path);
                self.import_info = Some(info);
                self.import_error = None;
            }
            Err(e) => {
                self.import_src = None;
                self.import_info = None;
                self.import_error = Some(e);
            }
        }
    }

    fn draw_tab_launchers(&mut self, ui: &mut egui::Ui) {
        if !self.launchers_scanned {
            let mut found = import::scan_launchers();
            if let Some(path) = import::official_minecraft() {
                found.push(import::Found {
                    source: "Minecraft".into(),
                    name: "Официальный лаунчер".into(),
                    path,
                    game_version: String::new(),
                    loader: String::new(),
                    loader_version: String::new(),
                });
            }
            self.launcher_found = found;
            self.launchers_scanned = true;
        }
        ui.horizontal(|ui| {
            if ui::secondary(ui, Some(Icon::Refresh), "Найти снова") {
                self.launchers_scanned = false;
            }
            ui.checkbox(&mut self.import_worlds, "Копировать миры");
        });
        ui.add_space(6.0);
        if self.launcher_found.is_empty() {
            ui.label(
                RichText::new(
                    "Ничего не найдено. Лаунчер ищет Prism, PolyMC, CurseForge и Modrinth App в обычных папках. \
                     Сборка лежит в другом месте — откройте «Из папки». MultiMC: экспортируйте сборку в .zip и откройте «Из файла».",
                )
                .color(theme::muted()),
            );
        }
        let items = self.launcher_found.clone();
        let mut chosen: Option<usize> = None;
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            ui.set_width(440.0);
            for (i, f) in items.iter().enumerate() {
                ui::card().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(truncate(&f.name, 34)).family(ui::bold_family()));
                            let about = if f.is_complete() {
                                let loader = if f.loader.is_empty() { String::new() } else { format!(" · {}", loader_title(&f.loader)) };
                                format!("{} · Minecraft {}{loader}", f.source, f.game_version)
                            } else {
                                format!("{} · версию нужно выбрать", f.source)
                            };
                            ui.label(RichText::new(about).color(theme::muted()).size(12.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (icon, text) = if f.is_complete() { (Icon::Import, "Импортировать") } else { (Icon::Folder, "Выбрать версию…") };
                            if ui::secondary(ui, Some(icon), text) {
                                chosen = Some(i);
                            }
                        });
                    });
                });
                ui.add_space(4.0);
            }
        });
        if let Some(i) = chosen {
            let f = items[i].clone();
            if f.is_complete() {
                self.start_import_found(&f);
            } else {
                // the version is unknown: continue on the "folder" tab
                self.new_tab = 1;
                self.set_import_folder(f.path);
            }
        }
        ui.add_space(8.0);
        if ui::secondary(ui, None, "Отмена") {
            self.show_new_instance = false;
        }
    }

    /// Run `f` on a worker and report into the dialog.
    fn start_instance_job(
        &mut self,
        f: impl FnOnce(&reqwest::blocking::Client, &std::path::Path, &Arc<install::InstallProgress>) -> Result<Instance, String> + Send + 'static,
    ) {
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        let job = start_detached(&client, move |c| f(c, &root, &prog_ref));
        self.new_instance_prog = Some(prog);
        self.new_instance_job = Some(job);
        self.new_instance_result = None;
    }

    /// Checks shared by every way of making a build; says why when it fails.
    fn name_is_usable(&mut self, name: &str) -> bool {
        if name.is_empty() {
            self.toast = Some("Укажите имя сборки".to_string());
            return false;
        }
        if self.state.instances.iter().any(|i| instance::sanitize(&i.name) == instance::sanitize(name)) {
            self.toast = Some(format!("Сборка «{name}» уже существует — выберите другое имя"));
            return false;
        }
        true
    }

    fn chosen_version(&mut self) -> String {
        if self.new_instance_version.trim().is_empty() {
            self.new_instance_version = self.versions_list.first().map(|v| v.id.clone()).unwrap_or_else(|| "latest".to_string());
        }
        self.new_instance_version.trim().to_string()
    }

    fn start_import_folder(&mut self) {
        let Some(src) = self.import_src.clone() else { return };
        let name = self.new_instance_name.trim().to_string();
        if !self.name_is_usable(&name) {
            return;
        }
        let inst = Instance {
            name,
            game_version: self.chosen_version(),
            loader: self.new_instance_loader.clone(),
            created: now_string(),
            ..Default::default()
        };
        let worlds = self.import_worlds;
        self.start_instance_job(move |c, root, prog| import::import_folder(c, root, &inst, &src, worlds, Some(prog)));
    }

    fn start_import_archive(&mut self) {
        let (Some(path), Some(info)) = (self.import_src.clone(), self.import_info.clone()) else { return };
        let name = self.new_instance_name.trim().to_string();
        if !self.name_is_usable(&name) {
            return;
        }
        let worlds = self.import_worlds;
        self.start_instance_job(move |c, root, prog| {
            let mut inst = import::import_archive(c, root, &path, &info, &name, worlds, Some(prog))?;
            inst.created = now_string();
            Ok(inst)
        });
    }

    fn start_import_found(&mut self, f: &import::Found) {
        let name = self.unique_name(&f.name);
        let inst = Instance {
            name,
            game_version: f.game_version.clone(),
            loader: f.loader.clone(),
            loader_version: f.loader_version.clone(),
            created: now_string(),
            ..Default::default()
        };
        let (src, worlds) = (f.path.clone(), self.import_worlds);
        self.start_instance_job(move |c, root, prog| import::import_folder(c, root, &inst, &src, worlds, Some(prog)));
    }

    fn start_new_instance(&mut self) {
        let name = self.new_instance_name.trim().to_string();
        if name.is_empty() {
            self.toast = Some("Укажите имя инстанса".to_string());
            return;
        }
        if self.state.instances.iter().any(|i| instance::sanitize(&i.name) == instance::sanitize(&name)) {
            self.toast = Some(format!("Сборка «{name}» уже существует — выберите другое имя"));
            return;
        }
        if self.new_instance_version.trim().is_empty() {
            let version = self
                .versions_list
                .first()
                .map(|v| v.id.clone())
                .unwrap_or_else(|| "latest".to_string());
            self.new_instance_version = version;
        }
        let version = self.new_instance_version.trim().to_string();
        let loader = self.new_instance_loader.clone();
        let instance = Instance {
            name: name.clone(),
            game_version: version,
            loader,
            loader_version: String::new(),
            version_name: None,
            modpack_slug: None,
            created: now_string(),
            ..Default::default()
        };

        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        let job = start_detached(&client, move |c| install::install_instance(c, &root, &instance, Some(&prog_ref)));
        self.new_instance_prog = Some(prog);
        self.new_instance_job = Some(job);
        self.new_instance_result = None;
    }
}

// ------------------------------------------------------------------ Account (offline)

impl LimeApp {
    /// (name, kind) of the player.
    /// "Играет как …" chip (top right of the home page): painted by hand so it
    /// keeps its size inside right-to-left layouts.
    fn account_chip(&mut self, ui: &mut egui::Ui, compact: bool) {
        let (name, kind) = self.account_display();
        let name = truncate(&name, 18);
        let font = egui::FontId::new(14.0, ui::bold_family());
        let name_w = ui.painter().layout_no_wrap(name.clone(), font.clone(), theme::text()).size().x;
        let width = if compact { 46.0 } else { 10.0 + 30.0 + 10.0 + name_w.max(60.0) + 14.0 };
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 46.0), egui::Sense::click());
        ui::reg("Аккаунт", rect);
        self.chip_rect = Some(rect);
        let hovered = resp.hovered();
        if theme::pixel() {
            let face = if hovered { theme::hover_bg() } else { theme::button() };
            ui::paint_key(ui.painter(), rect, face, ui::darken(theme::button(), if theme::palette().dark { 0.5 } else { 0.14 }), resp.is_pointer_button_down_on(), 0.05);
        } else {
            ui.painter().rect_filled(rect, egui::CornerRadius::same(12), if hovered { theme::hover_bg() } else { theme::card() });
            ui.painter().rect_stroke(rect, egui::CornerRadius::same(12), Stroke::new(1.0, theme::divider()), egui::StrokeKind::Inside);
        }
        let avatar = egui::Rect::from_min_size(rect.min + egui::vec2(10.0, 6.0), egui::vec2(30.0, 30.0));
        self.paint_avatar(ui, avatar, &name);
        let x = avatar.right() + 10.0;
        if compact {
            if resp.on_hover_text(format!("{name} · {kind}")).clicked() {
                self.profile_menu = self.state.profiles.len() > 1 && !self.profile_menu;
                if self.state.profiles.len() <= 1 {
                    self.page = Page::Other;
                }
            }
            return;
        }
        ui.painter().text(egui::pos2(x, rect.top() + 6.0), egui::Align2::LEFT_TOP, &name, font, theme::text());
        ui.painter().text(egui::pos2(x, rect.top() + 23.0), egui::Align2::LEFT_TOP, kind, egui::FontId::proportional(11.5), theme::muted());
        if resp.on_hover_text("Сменить профиль").clicked() {
            if self.state.profiles.len() > 1 {
                self.profile_menu = !self.profile_menu;
            } else {
                self.page = Page::Other;
            }
        }
    }

    // ---- network check ----

    fn draw_network_card(&mut self, ui: &mut egui::Ui) {
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui::section(ui, Icon::Wifi, "Сеть");
            ui.label(
                RichText::new("Если поиск, установка или вход не работают, проверьте, какие серверы доступны из вашей сети (VPN, прокси, блокировки).")
                    .color(theme::muted())
                    .size(12.5),
            );
            ui.add_space(4.0);
            let mut mirrors = !self.state.disable_mirrors;
            if ui
                .checkbox(&mut mirrors, "Использовать запасные серверы (зеркала), если основной недоступен")
                .changed()
            {
                self.state.disable_mirrors = !mirrors;
                download::set_mirrors_enabled(mirrors);
                self.save_state();
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let idle = self.net_job.is_none();
                if ui::button(ui, ui::BtnKind::Secondary, Some(Icon::Refresh), "Проверить сеть", idle).clicked() {
                    let client = install::make_client();
                    self.net_job = Some(start_detached(&client, |c| Ok(netcheck::check_all(c))));
                }
                if self.net_job.is_some() {
                    ui.add(egui::Spinner::new());
                    ui.label(RichText::new("Проверяем…").color(theme::muted()));
                }
            });
            if !self.net_results.is_empty() {
                ui.add_space(6.0);
                for r in &self.net_results {
                    ui.horizontal(|ui| {
                        let (dot, color) = match (r.reachable, r.status) {
                            (true, Some(s)) if s < 500 => ("●", theme::green()),
                            (true, _) => ("●", theme::yellow()),
                            _ => ("●", theme::danger()),
                        };
                        ui.label(RichText::new(dot).color(color));
                        ui.label(RichText::new(&r.name).family(ui::bold_family()));
                        ui.label(RichText::new(&r.purpose).color(theme::muted()).size(12.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let text = match r.status {
                                Some(s) => format!("HTTP {s} · {} мс", r.millis),
                                None => truncate(&r.detail, 70),
                            };
                            ui.label(RichText::new(text).color(if r.reachable { theme::muted() } else { theme::danger() }).size(12.0));
                        });
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
