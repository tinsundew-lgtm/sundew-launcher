//! LimeCraft launcher UI built on eframe/egui.
//!
//! All blocking core operations (Modrinth / Mojang / installs / launch) run on worker
//! threads; the app only holds shared result slots (`Job<T>`) which are drained each
//! frame so the UI never blocks.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, RichText, Stroke};

use crate::assets::Assets;
use crate::theme;
use crate::core::{account, install, instance, launch, modrinth, mojang};
use crate::core::instance::{Instance, LauncherState};

/// Shared result slot filled by a worker thread: `None` = still running.
type Job<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Активная вкладка (State). Три раздела по ТЗ:
///  - `Instances` — «Сборки» (главная, по умолчанию): список установленных
///    чистых клиентов (Vanilla/Fabric/Forge/NeoForge) + создание новых.
///  - `Mods`     — «Моды»: каталог и управление модами (+ модпаки Modrinth).
///  - `Other`    — «Другое»: ресурспаки, шейдеры, глобальные настройки.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Instances,
    Mods,
    Other,
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
        *slot.lock().unwrap() = Some(f(&client).map_err(|e| e.to_string()));
    });
    job
}

/// Drain a finished job slot, if finished.
fn drain<T>(job: &Job<T>) -> Option<Result<T, String>> {
    job.lock().unwrap().take()
}

/// `(fraction 0..=1, current file label)` from an InstallProgress.
fn progress_parts(prog: &install::InstallProgress) -> (f32, String) {
    let cur = prog.current.load(Ordering::Relaxed);
    let tot = prog.total.load(Ordering::Relaxed);
    let file = prog.current_file.lock().map(|g| g.clone()).unwrap_or_default();
    let frac = if tot > 0 { (cur as f32 / tot as f32).clamp(0.0, 1.0) } else { 0.0 };
    (frac, file)
}

/// Set up UI fonts. Tries a Windows system font first (nice for Cyrillic),
/// otherwise falls back to egui embedded fonts (which also support Cyrillic).
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut installed = false;
    for path in [
        "C:\\Windows\\Fonts\\segoeui.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "C:\\Windows\\Fonts\\DejaVuSans.ttf",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("ui".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                list.insert(0, "ui".to_owned());
            }
            installed = true;
            break;
        }
    }
    if installed {
        ctx.set_fonts(fonts);
    }
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

    // ---- Пункт 4: окно «Настройки» конкретной сборки ----
    instance_settings: Option<usize>,

    /// Под-раздел вкладки «Моды»: false = каталог модов, true = модпаки Modrinth.
    mods_show_modpacks: bool,

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
    mod_install_job: Option<Job<()>>,
    mod_install_done: bool,

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
}

impl LimeApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        let root = install::launcher_root().unwrap_or_else(|e| {
            // Корневая папка недоступна для записи — вместо тихого сбоя
            // установки показываем понятную ошибку через panic-хук.
            eprintln!("LimeCraft: {e}");
            let msg = format!("Нет доступа к папке данных: {e}.");
            panic!("{msg}");
        });
        let mut state = instance::load_state(&root);
        if state.account.is_none() {
            state.account = Some(account::Account::offline_default());
        }
        let selected_version = state
            .instances
            .first()
            .map(|i| i.game_version.clone())
            .unwrap_or_else(|| "latest".to_string());
        Self {
            root,
            state: state.clone(),
            page: Page::Instances,
            assets: Assets::load(&cc.egui_ctx),
            toast: None,
            active_instance: 0,
            versions_job: None,
            versions_list: Vec::new(),
            selected_version,
            show_snapshots: false,
            manifest_requested: false,
            instance_settings: None,
            mods_show_modpacks: false,
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
            mod_install_done: false,
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
        }
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
                .desired_width(ui.available_width())
                .text(RichText::new(label).color(theme::GREEN_DARK)),
        );
        if let Some(c) = cancel {
            if ui.button("Отмена").clicked() {
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
        ui.ctx().request_repaint();

        // 1) drain finished background jobs
        self.poll_jobs();

        // 2) nav on the left + content in the middle
        egui::Panel::left("nav_panel").show(ui, |ui| self.draw_nav(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Instances => self.draw_instances(ui),
            Page::Mods => self.draw_mods(ui),
            Page::Other => self.draw_other(ui),
        });

        // transient toast window (top-right)
        if let Some(msg) = self.toast.clone() {
            egui::Window::new("limecraft_toast")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 12.0))
                .show(ui.ctx(), |ui| {
                    ui.colored_label(theme::DANGER, msg);
                });
        }

        if self.show_new_instance {
            self.draw_new_instance_dialog(ui.ctx());
        }

        // Пункт 4: окно «Настройки сборки» (заглушка) для выбранного инстанса
        if let Some(idx) = self.instance_settings {
            self.draw_instance_settings(ui.ctx(), idx);
        }
    }
}

// ------------------------------------------------------------------ nav

impl LimeApp {
    fn draw_nav(&mut self, ui: &mut egui::Ui) {
        ui.set_width(212.0);
        ui.add_space(18.0);
        if let Some(badge) = &self.assets.badge {
            ui.centered_and_justified(|ui| {
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::from_handle(badge))
                        .max_size(egui::vec2(112.0, 112.0)),
                );
            });
        }
        ui.add_space(2.0);
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("LimeCraft").size(24.0).color(theme::GREEN_DARK).strong());
        });
        ui.add_space(18.0);

        let items = [
            (Page::Instances, "Сборки"),
            (Page::Mods, "Моды"),
            (Page::Other, "Другое"),
        ];
        for (page, label) in items {
            let selected = self.page == page;
            let text = RichText::new(label).size(16.0);
            if ui.selectable_label(selected, text).clicked() {
                self.page = page;
            }
        }

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new("ИНСТАНСЫ").color(theme::MUTED).size(12.0));
        if self.state.instances.is_empty() {
            ui.label(RichText::new("Пусто").color(theme::MUTED).size(13.0));
        } else {
            for (idx, inst) in self.state.instances.iter().enumerate() {
                let selected = idx == self.active_index();
                let name_rt = RichText::new(&inst.name).size(14.0);
                if ui.selectable_label(selected, name_rt).clicked() {
                    self.active_instance = idx;
                }
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("LimeCraft · eframe/egui").color(theme::MUTED).size(11.0));
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
                self.new_instance_result = Some(res.map(|_| ()));
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
                    }
                    Err(e) => self.toast = Some(format!("Ошибка поиска: {e}")),
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
                        self.toast = Some(format!("Ошибка установки модпака: {e}"));
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
                    }
                    Err(e) => self.toast = Some(format!("Ошибка поиска: {e}")),
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
                    Ok(()) => self.mod_install_done = true,
                    Err(e) => self.toast = Some(format!("Ошибка установки мода: {e}")),
                }
            }
        }

        // launch
        if let Some(job) = &self.launch_job {
            if let Some(res) = drain(job) {
                self.launch_job = None;
                match res {
                    Ok(msg) => {
                        self.launch_status = Some(msg.clone());
                        self.toast = Some(msg);
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
    /// Главная вкладка «Сборки»: список установленных чистых клиентов
    /// (Vanilla/Fabric/Forge/NeoForge) + создание новой из списка версий.
    fn draw_instances(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading(RichText::new("Сборки").size(26.0).color(theme::GREEN_DARK));
        ui.label(RichText::new("Чистые клиенты Minecraft с загрузчиком (без модов).").color(theme::MUTED).size(13.0));
        ui.add_space(10.0);

        // ---- Установка новой сборки из списка версий (Пункт 2) ----
        egui::Frame::new()
            .fill(theme::CARD)
            .stroke(Stroke::new(1.0, theme::DIVIDER))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.label(RichText::new("Новая сборка").size(16.0).strong().color(theme::TEXT));
                ui.add_space(6.0);
                // Устанавливаю внутренний UI сборочного мастера
                self.draw_new_build_wizard(ui);
            });

        ui.add_space(16.0);
        ui.separator();
        ui.label(RichText::new("Установленные сборки").color(theme::MUTED).size(13.0));
        ui.add_space(6.0);
        if self.state.instances.is_empty() {
            if let Some(empty) = &self.assets.empty_state {
                ui.add_space(24.0);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(140.0, 140.0)));
                });
            }
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Пока нет сборок — создайте первую выше.").color(theme::MUTED));
            });
        } else {
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for idx in 0..self.state.instances.len() {
                    let inst = self.state.instances[idx].clone();
                    let selected = idx == self.active_index();
                    let frame = egui::Frame::new()
                        .fill(if selected { theme::GREEN_PALE } else { theme::CARD })
                        .stroke(if selected { Stroke::new(1.5, theme::GREEN) } else { Stroke::new(1.0, theme::DIVIDER) })
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(12));
                    frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&inst.name).size(16.0).strong().color(theme::TEXT));
                                    let loader = if inst.loader.is_empty() { "vanilla" } else { &inst.loader };
                                    ui.label(RichText::new(format!("MC {} · {}", inst.game_version, loader)).color(theme::YELLOW_DARK).size(12.0));
                                    if let Some(mb) = inst.memory_mb {
                                        ui.label(RichText::new(format!("RAM {mb} МБ")).color(theme::MUTED).size(12.0));
                                    }
                                });
                                if inst.modpack_slug.is_some() {
                                    ui.label(RichText::new("модпак (Modrinth)").color(theme::MUTED).size(12.0));
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add(egui::Button::new(RichText::new("⚙").size(20.0).color(theme::MUTED)))
                                    .on_hover_text("Настройки сборки")
                                    .clicked()
                                {
                                    self.instance_settings = Some(idx);
                                }
                                if ui
                                    .add(egui::Button::new(RichText::new("🗑").size(18.0).color(theme::DANGER)))
                                    .on_hover_text("Удалить сборку")
                                    .clicked()
                                {
                                    self.remove_instance(idx);
                                }
                                let play_btn = egui::Button::new(RichText::new("▶  Играть").size(16.0).color(Color32::WHITE).strong())
                                    .fill(theme::GREEN)
                                    .corner_radius(egui::CornerRadius::same(10))
                                    .min_size(egui::vec2(120.0, 34.0));
                                if ui.add(play_btn).clicked() {
                                    self.active_instance = idx;
                                    self.launch_selected();
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
            });
            if let Some(status) = &self.launch_status {
                ui.add_space(6.0);
                ui.label(RichText::new(status).size(13.0).color(theme::MUTED));
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("＋ Создать сборку по-шагово").size(15.0).color(theme::GREEN_DARK)))
                .clicked()
            {
                self.open_new_instance_dialog();
            }
        });
    }

    fn remove_instance(&mut self, idx: usize) {
        if idx < self.state.instances.len() {
            self.state.instances.remove(idx);
            if self.active_instance >= self.state.instances.len() && !self.state.instances.is_empty() {
                self.active_instance = self.state.instances.len() - 1;
            }
            self.save_state();
            self.toast = Some("Сборка удалена".to_string());
        }
    }

    fn launch_selected(&mut self) {
        let Some(inst) = self.state.instances.get(self.active_index()).cloned() else {
            self.toast = Some("Выберите инстанс".to_string());
            return;
        };
        let java_custom = self.state.java_path.clone();
        let ram_mb = std::cmp::max(self.state.ram_mb, 1024);
        let username = self
            .state
            .account
            .as_ref()
            .map(|a| a.username.clone())
            .unwrap_or_else(|| "Player".to_string());
        let uuid = self
            .state
            .account
            .as_ref()
            .map(|a| a.resolved_uuid())
            .unwrap_or_else(|| account::offline_uuid(&username));
        let token = self
            .state
            .account
            .as_ref()
            .map(|a| a.resolved_token())
            .unwrap_or_else(|| "0".to_string());
        let root = self.root.clone();
        let job: Job<String> = Arc::new(Mutex::new(None));
        let slot = job.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let java = launch::find_java_path(java_custom.as_deref())
                    .ok_or_else(|| "Java не найдена — задайте путь в «Настройки»".to_string())?;
                let launch = launch::build_launch_command(&java, &root, &inst, &username, &uuid, &token, ram_mb)?;
                let game_dir = launch.game_dir.clone();
                let _child = launch::spawn_game(&java, &launch.command, &game_dir)?;
                Ok(format!("Игра запущена в {}", game_dir.display()))
            })();
            *slot.lock().unwrap() = Some(result);
        });
        self.launch_job = Some(job);
        self.launch_status = Some("Запуск…".to_string());
    }
}

// ------------------------------------------------------------------ Versions

impl LimeApp {
    /// Встроенный мастер создания новой сборки: список версий из манифеста
    /// Mojang (релизы по умолчанию + чекбокс снапшотов), выбор лоадера, запуск
    /// установки. Рендерится внутри главной вкладки «Сборки».
    fn draw_new_build_wizard(&mut self, ui: &mut egui::Ui) {
        // Автозагрузка манифеста при первом открытии — новичку не нужно
        // знать версии наизусть.
        if !self.manifest_requested && self.versions_job.is_none() && self.versions_list.is_empty() {
            self.manifest_requested = true;
            let client = install::make_client();
            self.versions_job = Some(start_detached(&client, |c| {
                mojang::fetch_manifest(c).map(|m| m.versions).map_err(|e| e.to_string())
            }));
        }

        ui.horizontal(|ui| {
            if ui.button("🔃 Обновить список").clicked() {
                let client = install::make_client();
                self.versions_job = Some(start_detached(&client, |c| {
                    mojang::fetch_manifest(c).map(|m| m.versions).map_err(|e| e.to_string())
                }));
            }
            if self.versions_job.is_some() {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Загрузка манифеста…").color(theme::MUTED));
            }
        });

        if self.versions_list.is_empty() {
            ui.label(RichText::new("Список версий загружается…").color(theme::MUTED).size(13.0));
            return;
        }

        // ---- Пункт 2: фильтр только релизов + чекбокс снапшотов ----
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_snapshots, "Показывать снапшоты");
            if self.show_snapshots {
                ui.label(RichText::new("(релизы всегда отображаются)").color(theme::MUTED).size(12.0));
            }
        });

        let filtered = mojang::filter_versions(&self.versions_list, self.show_snapshots);
        if filtered.is_empty() {
            ui.label(RichText::new("В манифесте нет релизов. Нажмите «Обновить список».").color(theme::MUTED));
            return;
        }
        if !filtered.iter().any(|v| v.id == self.selected_version) {
            self.selected_version = filtered[0].id.clone();
        }

        // Выбор версии (ComboBox)
        let mut version = self.selected_version.clone();
        egui::ComboBox::from_label("Версия Minecraft")
            .selected_text(RichText::new(&version).color(theme::TEXT).size(15.0))
            .show_ui(ui, |ui| {
                for entry in filtered {
                    let label = if entry.kind == "release" {
                        format!("{} (релиз)", entry.id)
                    } else {
                        format!("{} (снапшот)", entry.id)
                    };
                    if ui.selectable_label(entry.id == version, label).clicked() {
                        version = entry.id.clone();
                    }
                }
            });
        self.selected_version = version;

        // Выбор лоадера
        let loaders = [
            ("", "Vanilla (чистый клиент)"),
            ("fabric", "Fabric"),
            ("quilt", "Quilt"),
            ("forge", "Forge"),
            ("neoforge", "NeoForge"),
        ];
        let cur = self.new_instance_loader.clone();
        egui::ComboBox::from_label("Загрузчик")
            .selected_text(loaders.iter().find(|(v, _)| *v == cur).map(|(_, l)| *l).unwrap_or("Vanilla (чистый клиент)"))
            .show_ui(ui, |ui| {
                for (val, label) in loaders {
                    if ui.selectable_label(cur == val, label).clicked() {
                        self.new_instance_loader = val.to_string();
                    }
                }
            });

        // Кнопка установки + прогресс
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if self.new_instance_job.is_none() {
                if ui
                    .add(egui::Button::new(RichText::new("⬇ Установить сборку").size(15.0).color(theme::GREEN_DARK)))
                    .clicked()
                {
                    self.start_wizard_install();
                }
            } else {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Установка…").color(theme::MUTED));
            }
            match &self.new_instance_result {
                Some(Ok(())) => {
                    ui.label(RichText::new("✓ Сборка установлена").color(theme::SUCCESS));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(format!("Ошибка: {e}")).color(theme::DANGER));
                }
                None => {}
            }
        });
        if let Some(prog) = &self.new_instance_prog {
            ui.add_space(8.0);
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
        }
    }

    fn start_wizard_install(&mut self) {
        let version = self.selected_version.clone();
        if version.is_empty() || version == "latest" {
            self.toast = Some("Сначала выберите версию".to_string());
            return;
        }
        self.new_instance_name = version.clone();
        self.new_instance_version = version.clone();
        let instance = Instance {
            name: self.new_instance_name.clone(),
            game_version: self.new_instance_version.clone(),
            loader: self.new_instance_loader.clone(),
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

// ------------------------------------------------------------------ Modpacks

impl LimeApp {
    fn draw_modpacks(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading(RichText::new("Сборки (Modrinth)").size(26.0).color(theme::GREEN_DARK));
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.pack_search_query)
                    .hint_text("напр. skyblock, create, vanilla+")
                    .desired_width(320.0),
            );
            if ui.button("🔍 Поиск").clicked() {
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

        if self.pack_results.is_empty() {
            if let Some(empty) = &self.assets.empty_state {
                ui.add_space(24.0);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 160.0)));
                });
            }
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Введите запрос и нажмите «Поиск» — найдём модпаки на Modrinth.").color(theme::MUTED));
            });
            return;
        }

        egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
            for idx in 0..self.pack_results.len() {
                let hit = self.pack_results[idx].clone();
                let selected = self.selected_pack == Some(idx);
                let frame = egui::Frame::new()
                    .fill(if selected { theme::GREEN_PALE } else { theme::CARD })
                    .stroke(if selected {
                        Stroke::new(1.5, theme::GREEN)
                    } else {
                        Stroke::new(1.0, theme::DIVIDER)
                    })
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(12));
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if let Some(icon) = &self.assets.modpack_icon {
                            ui.add(
                                egui::Image::new(egui::load::SizedTexture::from_handle(icon)).max_size(egui::vec2(48.0, 48.0)),
                            );
                        }
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&hit.title).size(16.0).strong().color(theme::TEXT));
                                ui.label(RichText::new(format!("⬇ {}", hit.downloads)).color(theme::MUTED).size(12.0));
                            });
                            ui.label(RichText::new(&hit.description).color(theme::MUTED).size(13.0));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("автор: {}", hit.author)).color(theme::MUTED).size(12.0));
                                ui.label(RichText::new(format!("☰ {}", hit.versions.len())).color(theme::MUTED).size(12.0));
                                if ui.small_button("Выбрать").clicked() {
                                    self.selected_pack = Some(idx);
                                }
                            });
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });

        // install bar
        if let Some(idx) = self.selected_pack {
            let hit = self.pack_results[idx].clone();
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Установить модпак в новый инстанс:").size(15.0));
                let mut name = self.pack_install_name.clone();
                ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .hint_text(hit.slug.clone().unwrap_or_else(|| hit.project_id.clone()))
                        .desired_width(220.0),
                );
                self.pack_install_name = name;
                if ui
                    .add(egui::Button::new(RichText::new("⬇ Скачать и установить").size(15.0).color(theme::GREEN_DARK)))
                    .clicked()
                {
                    self.start_pack_install(&hit);
                }
            });
        }
    }

    fn search_packs(&mut self) {
        let query = self.pack_search_query.trim().to_string();
        if query.is_empty() {
            self.toast = Some("Введите поисковый запрос".to_string());
            return;
        }
        let client = install::make_client();
        self.pack_search_job = Some(start_detached(&client, move |c| modrinth::search(c, &query, "modpack", 12)));
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
        self.upsert_instance(instance.clone());

        let project_id = hit.project_id.clone();
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        let job = start_detached(&client, move |c| {
            // resolve the real .mrpack URL through the API (the CDN
            // `latest.mrpack` alias answers 400), then install it.
            let url = modrinth::latest_mrpack_url(c, &project_id)?;
            install::install_mrpack(c, &root, &instance, &url, Some(&prog_ref))
        });
        self.pack_install_prog = Some(prog);
        self.pack_install_job = Some(job);
    }
}

// ------------------------------------------------------------------ Mods

impl LimeApp {
    /// Вкладка «Моды»: переключатель «Моды / Модпаки (Modrinth)» + контент.
    fn draw_mods(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading(RichText::new("Моды").size(26.0).color(theme::GREEN_DARK));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (val, label) in [(false, "Каталог модов"), (true, "Модпаки (Modrinth)")] {
                if ui.selectable_label(self.mods_show_modpacks == val, label).clicked() {
                    self.mods_show_modpacks = val;
                }
            }
        });
        ui.add_space(8.0);
        if self.mods_show_modpacks {
            self.draw_modpacks(ui);
        } else {
            self.draw_mods_catalog(ui);
        }
    }

    fn draw_mods_catalog(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.mod_search_query)
                    .hint_text("напр. sodium, jei, create")
                    .desired_width(320.0),
            );
            if ui.button("🔍 Поиск").clicked() {
                self.search_mods();
            }
            if self.mod_search_job.is_some() {
                ui.add(egui::Spinner::new());
            }
        });

        // target instance picker
        ui.horizontal(|ui| {
            ui.label(RichText::new("Целевой инстанс:").color(theme::MUTED));
            if self.state.instances.is_empty() {
                ui.label(RichText::new("нет — создайте на «Главной»").color(theme::MUTED));
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
                self.active_instance = sel;
            }
        });

        ui.add_space(8.0);
        if let Some(prog) = &self.mod_install_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
            ui.add_space(8.0);
        }

        if self.mod_results.is_empty() {
            if let Some(empty) = &self.assets.empty_state {
                ui.add_space(24.0);
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 160.0)));
                });
            }
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Введите запрос и нажмите «Поиск» — найдём моды на Modrinth.").color(theme::MUTED));
            });
            return;
        }

        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for idx in 0..self.mod_results.len() {
                let hit = self.mod_results[idx].clone();
                let selected = self.selected_mod == Some(idx);
                let frame = egui::Frame::new()
                    .fill(if selected { theme::GREEN_PALE } else { theme::CARD })
                    .stroke(if selected {
                        Stroke::new(1.5, theme::GREEN)
                    } else {
                        Stroke::new(1.0, theme::DIVIDER)
                    })
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(12, 10));
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&hit.title).size(16.0).strong().color(theme::TEXT));
                                ui.label(RichText::new(format!("⬇ {}", hit.downloads)).color(theme::MUTED).size(12.0));
                            });
                            ui.label(RichText::new(&hit.description).color(theme::MUTED).size(13.0));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("автор: {}", hit.author)).color(theme::MUTED).size(12.0));
                                ui.label(RichText::new(format!("для: {}", hit.versions.join(", "))).color(theme::MUTED).size(12.0));
                                if ui.small_button("Версии…").clicked() {
                                    self.selected_mod = Some(idx);
                                    self.load_mod_versions(&hit);
                                }
                            });
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });

        // version picker + install for selected mod
        if let Some(idx) = self.selected_mod {
            let hit = self.mod_results.get(idx).cloned();
            if let Some(hit) = hit {
                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Версии «{}»", hit.title)).size(16.0).strong().color(theme::TEXT));
                    if self.mod_versions_job.is_some() {
                        ui.add(egui::Spinner::new());
                    }
                });

                if !self.mod_versions.is_empty() {
                    let mut sel = self.selected_mod_version.unwrap_or(0).min(self.mod_versions.len() - 1);
                    egui::ComboBox::from_id_salt("modVersion")
                        .selected_text(
                            self.mod_versions
                                .get(sel)
                                .map(|v| format!("{} · {}", v.version_number, v.version_type))
                                .unwrap_or_default(),
                        )
                        .show_ui(ui, |ui| {
                            for (vidx, v) in self.mod_versions.iter().enumerate() {
                                let label = format!(
                                    "{} · {} · {} · {}",
                                    v.version_number,
                                    v.version_type,
                                    v.game_versions.first().map(|s| s.as_str()).unwrap_or("?"),
                                    v.loaders.first().map(|s| s.as_str()).unwrap_or("any")
                                );
                                if ui.selectable_label(vidx == sel, label).clicked() {
                                    sel = vidx;
                                }
                            }
                        });
                    self.selected_mod_version = Some(sel);

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("⬇ Установить в инстанс").size(15.0).color(theme::GREEN_DARK)))
                            .clicked()
                        {
                            self.start_mod_install();
                        }
                        if self.mod_install_done {
                            ui.label(RichText::new("Мод установлен ✓").color(theme::SUCCESS));
                        }
                    });
                } else if self.mod_versions_job.is_none() && !self.mod_results.is_empty() {
                    ui.label(RichText::new("Нет подходящих версий — попробуйте другой инстанс.").color(theme::MUTED).size(13.0));
                }
            }
        }
    }

    fn search_mods(&mut self) {
        let query = self.mod_search_query.trim().to_string();
        if query.is_empty() {
            self.toast = Some("Введите поисковый запрос".to_string());
            return;
        }
        let client = install::make_client();
        self.mod_search_job = Some(start_detached(&client, move |c| modrinth::search(c, &query, "mod", 12)));
    }

    fn load_mod_versions(&mut self, hit: &modrinth::SearchHit) {
        let Some(target) = self.state.instances.get(self.active_index()).cloned() else {
            self.mod_versions.clear();
            self.selected_mod_version = None;
            return;
        };
        let id = hit.project_id.clone();
        let client = install::make_client();
        let job = start_detached(&client, move |c| {
            if target.game_version.is_empty() || target.game_version == "latest" {
                return modrinth::get_project_versions(c, &id);
            }
            let loaders: Vec<&str> = if target.loader.is_empty() {
                vec!["fabric", "forge", "neoforge", "quilt"]
            } else {
                vec![target.loader.as_str()]
            };
            modrinth::get_versions_for(c, &id, &target.game_version, &loaders)
                .or_else(|_| modrinth::get_project_versions(c, &id))
        });
        self.mod_versions_job = Some(job);
        self.mod_versions.clear();
        self.selected_mod_version = None;
        self.mod_install_done = false;
    }

    fn start_mod_install(&mut self) {
        let Some(mvidx) = self.selected_mod_version else {
            self.toast = Some("Сначала выберите версию мода".to_string());
            return;
        };
        if mvidx >= self.mod_versions.len() {
            self.toast = Some("Сначала выберите версию мода".to_string());
            return;
        }
        let version = self.mod_versions[mvidx].clone();
        let instance_name = self.state.instances.get(self.active_index()).map(|i| i.name.clone());
        let Some(instance_name) = instance_name else {
            self.toast = Some("Нет инстанса для установки мода".to_string());
            return;
        };
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        let job = start_detached(&client, move |c| {
            install::download_mod_version(c, &root, &instance_name, &version, Some(&prog_ref))?;
            install::download_mod_dependencies(c, &root, &instance_name, &version, Some(&prog_ref))?;
            Ok(())
        });
        self.mod_install_prog = Some(prog);
        self.mod_install_job = Some(job);
        self.mod_install_done = false;
    }
}

// ------------------------------------------------------------------ Settings

impl LimeApp {
    /// Вкладка «Другое» (ресурспаки, шейдеры, глобальные настройки).
    fn draw_other(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.heading(RichText::new("Другое").size(26.0).color(theme::GREEN_DARK));
        ui.label(RichText::new("Ресурспаки, шейдеры и глобальные настройки.").color(theme::MUTED).size(13.0));
        ui.add_space(12.0);

        // ---- Задел: ресурспаки / шейдеры (заглушки) ----
        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.label(RichText::new("Ресурспаки").strong().color(theme::TEXT));
                ui.label(RichText::new("Управление ресурспаками появится в следующем обновлении.").color(theme::MUTED).size(13.0));
                ui.add_space(10.0);
                ui.label(RichText::new("Шейдеры").strong().color(theme::TEXT));
                ui.label(RichText::new("Управление шейдерами появится в следующем обновлении.").color(theme::MUTED).size(13.0));
            });

        ui.add_space(16.0);

        egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.label(RichText::new("Учётная запись").strong().color(theme::TEXT));
                ui.add_space(6.0);

                let mut provider = self
                    .state
                    .account
                    .as_ref()
                    .map(|a| a.provider)
                    .unwrap_or_default();
                let providers = [
                    (account::AuthProvider::Offline, "Офлайн (по нику)"),
                    (account::AuthProvider::Microsoft, "Microsoft (скоро)"),
                ];
                egui::ComboBox::from_label("Способ входа")
                    .selected_text(providers.iter().find(|(p, _)| *p == provider).map(|(_, l)| *l).unwrap_or("Офлайн"))
                    .show_ui(ui, |ui| {
                        for (p, label) in providers {
                            if ui.selectable_label(provider == p, label).clicked() {
                                provider = p;
                            }
                        }
                    });
                if let Some(account) = self.state.account.as_mut() {
                    account.provider = provider;
                }

                ui.add_space(6.0);
                ui.label(RichText::new("Ник (для офлайн-запуска)").color(theme::MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.username_input).desired_width(220.0).hint_text("Player"));
                if let Some(account) = &self.state.account {
                    if account.provider == account::AuthProvider::Offline {
                        let uuid = account::offline_uuid(&self.username_input.trim());
                        ui.label(
                            RichText::new(format!("UUID для этого ника: {uuid}"))
                                .color(theme::MUTED)
                                .size(12.0),
                        );
                    } else {
                        ui.label(
                            RichText::new("Авторизация Microsoft появится в следующем обновлении.")
                                .color(theme::YELLOW_DARK)
                                .size(12.0),
                        );
                    }
                }

                ui.add_space(14.0);
                ui.label(RichText::new("Путь к Java (java.exe)").strong().color(theme::TEXT));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.java_input).desired_width(380.0).hint_text("C:\\...\\java.exe"));
                    if ui.button("Автоопределить").clicked() {
                        match launch::find_java_path(None) {
                            Some(java) => {
                                self.java_input = java.display().to_string();
                                self.toast = Some(format!("Найдено: {}", self.java_input));
                            }
                            None => self.toast = Some("Java не найдена автоматически".to_string()),
                        }
                    }
                });

                ui.add_space(14.0);
                ui.label(RichText::new("Память (MB)").strong().color(theme::TEXT));
                ui.add(egui::TextEdit::singleline(&mut self.ram_input).desired_width(160.0).hint_text("4096"));

                ui.add_space(16.0);
                if ui.button("💾 Сохранить").clicked() {
                    self.apply_settings_to_state();
                    match instance::save_state(&self.root, &self.state) {
                        Ok(()) => self.toast = Some("Настройки сохранены".to_string()),
                        Err(e) => self.toast = Some(format!("Ошибка сохранения: {e}")),
                    }
                }
            });

        ui.add_space(16.0);
        ui.separator();
        ui.label(RichText::new("Установленные инстансы:").color(theme::MUTED).size(13.0));
        for inst in &self.state.instances {
            let loader = if inst.loader.is_empty() { "vanilla" } else { &inst.loader };
            ui.label(RichText::new(format!("• {} — MC {} ({})", inst.name, inst.game_version, loader)).size(14.0));
        }
    }

    fn apply_settings_to_state(&mut self) {
        let java = self.java_input.trim();
        self.state.java_path = if java.is_empty() { None } else { Some(java.to_string()) };
        self.state.ram_mb = self.ram_input.trim().parse().unwrap_or(4096);
        // Обновить ник офлайн-аккаунта.
        let name = self.username_input.trim();
        if self.state.account.is_none() {
            self.state.account = Some(account::Account::offline_default());
        }
        if let Some(acc) = self.state.account.as_mut() {
            if !name.is_empty() {
                acc.username = name.to_string();
            }
        }
    }
}

// ------------------------------------------------------------------ Instance settings dialog (Пункт 4)

impl LimeApp {
    /// Окно-заглушка «Настройки сборки». Здесь будет логика RAM/JVM/Java/иконки
    /// для конкретной сборки; сейчас заложен каркас (`Option`-поля в Instance)
    /// и простая наглядная панель.
    fn draw_instance_settings(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(inst) = self.state.instances.get(idx).cloned() else {
            self.instance_settings = None;
            return;
        };
        egui::Window::new(format!("⚙ Настройки — {}", inst.name))
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                let loader = if inst.loader.is_empty() { "vanilla" } else { &inst.loader };
                ui.label(
                    RichText::new(format!("Minecraft {} · {}", inst.game_version, loader))
                        .color(theme::MUTED)
                        .size(13.0),
                );
                ui.add_space(10.0);
                ui.label(RichText::new("Индивидуальные настройки сборки").strong().color(theme::TEXT));
                ui.label(
                    RichText::new("RAM, JVM-аргументы, Java и иконка появятся в следующем обновлении. Каркас уже заложен в профиль сборки.")
                        .color(theme::MUTED)
                        .size(13.0),
                );

                ui.add_space(10.0);
                // Инфо-поля, чтобы было видно задел (Option-поля в Instance)
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Память (МБ):").color(theme::MUTED));
                    ui.label(RichText::new(inst.memory_mb.map(|m| m.to_string()).unwrap_or_else(|| "глобальная".into())).size(13.0));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("JVM-аргументы:").color(theme::MUTED));
                    ui.label(RichText::new(inst.jvm_args.clone().unwrap_or_else(|| "—".into())).size(13.0));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Java:").color(theme::MUTED));
                    ui.label(RichText::new(inst.java_path.clone().unwrap_or_else(|| "глобальная".into())).size(13.0));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Иконка:").color(theme::MUTED));
                    ui.label(RichText::new(inst.icon.clone().unwrap_or_else(|| "—".into())).size(13.0));
                });

                ui.add_space(12.0);
                if ui.button("Закрыть").clicked() {
                    self.instance_settings = None;
                }
            });
    }
}

// ------------------------------------------------------------------ New instance dialog

impl LimeApp {
    fn open_new_instance_dialog(&mut self) {
        self.show_new_instance = true;
        self.new_instance_name = "Мой инстанс".to_string();
        self.new_instance_version = if self.selected_version == "latest" {
            String::new()
        } else {
            self.selected_version.clone()
        };
        self.new_instance_loader = String::new();
        self.new_instance_prog = None;
        self.new_instance_job = None;
        self.new_instance_result = None;
    }

    fn draw_new_instance_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Создать инстанс")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(RichText::new("Имя инстанса").color(theme::MUTED));
                ui.add(egui::TextEdit::singleline(&mut self.new_instance_name).desired_width(300.0));
                ui.add_space(6.0);
                ui.label(RichText::new("Версия Minecraft").color(theme::MUTED));
                if !self.versions_list.is_empty() {
                    let mut v = self.new_instance_version.clone();
                    egui::ComboBox::from_label("Версия для инстанса")
                        .selected_text(v.clone())
                        .show_ui(ui, |ui| {
                            for entry in &self.versions_list {
                                if ui
                                    .selectable_label(entry.id == v, format!("{} ▸ {}", entry.id, entry.kind))
                                    .clicked()
                                {
                                    v = entry.id.clone();
                                }
                            }
                        });
                    self.new_instance_version = v;
                } else {
                    ui.add(egui::TextEdit::singleline(&mut self.new_instance_version).hint_text("Например 1.21.6").desired_width(300.0));
                }

                // Loader selector
                ui.add_space(6.0);
                ui.label(RichText::new("Мод-лоадер").color(theme::MUTED));
                let loaders = [
                    ("", "None — чистая vanilla"),
                    ("fabric", "Fabric"),
                    ("quilt", "Quilt"),
                    ("forge", "Forge"),
                    ("neoforge", "NeoForge"),
                ];
                let cur = self.new_instance_loader.clone();
                egui::ComboBox::from_label("Лоадер")
                    .selected_text(
                        loaders
                            .iter()
                            .find(|(v, _)| *v == cur)
                            .map(|(_, l)| *l)
                            .unwrap_or("None — чистая vanilla"),
                    )
                    .show_ui(ui, |ui| {
                        for (val, label) in loaders {
                            if ui
                                .selectable_label(cur == val, format!("{label}"))
                                .clicked()
                            {
                                self.new_instance_loader = val.to_string();
                            }
                        }
                    });
                if self.new_instance_loader == "forge" || self.new_instance_loader == "neoforge" {
                    ui.label(
                        RichText::new("Установится через официальный installer (FML-профиль).")
                            .color(theme::YELLOW_DARK)
                            .size(12.0),
                    );
                }
                if let Some(prog) = &self.new_instance_prog {
                    ui.add_space(8.0);
                    self.install_progress_bar(ui, prog, Some(&prog.cancelled));
                }

                if self.new_instance_job.is_some() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(RichText::new("Установка…").color(theme::MUTED));
                    });
                }

                match &self.new_instance_result {
                    Some(Ok(())) => {
                        ui.add_space(6.0);
                        ui.label(RichText::new("Инстанс установлен ✓").color(theme::SUCCESS));
                        if ui.button("Готово").clicked() {
                            self.show_new_instance = false;
                            self.new_instance_result = None;
                        }
                    }
                    Some(Err(e)) => {
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("Ошибка: {e}")).color(theme::DANGER));
                        if ui.button("Закрыть").clicked() {
                            self.show_new_instance = false;
                            self.new_instance_result = None;
                        }
                    }
                    None => {
                        if self.new_instance_job.is_none() {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add(egui::Button::new(RichText::new("⬇ Установить").size(15.0).color(theme::GREEN_DARK)))
                                    .clicked()
                                {
                                    self.start_new_instance();
                                }
                                if ui.button("Отмена").clicked() {
                                    self.show_new_instance = false;
                                }
                            });
                        }
                    }
                }
            });
    }

    fn start_new_instance(&mut self) {
        let name = self.new_instance_name.trim().to_string();
        if name.is_empty() {
            self.toast = Some("Укажите имя инстанса".to_string());
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
