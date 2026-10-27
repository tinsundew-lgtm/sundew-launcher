//! The project card: everything about one project from the Modrinth catalog
//! (a mod, resource pack, shader or modpack) — header, description, versions
//! with filters and the picture gallery.

use std::sync::Arc;

use eframe::egui::{self, RichText, Stroke};

use super::{Job, LimeApp, Page, drain, loader_title, start_detached, truncate};
use crate::core::{images, install, instance::Instance, modrinth, mods};
use crate::markdown::{self, Block, Span};
use crate::ui::{self, Icon};
use crate::{fx, theme};

/// What kind of project a card shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ProjectKind {
    /// Goes into an existing instance.
    Content(mods::ContentKind),
    /// Becomes a new instance.
    Modpack,
}

impl ProjectKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            ProjectKind::Content(mods::ContentKind::Mod) => "Мод",
            ProjectKind::Content(mods::ContentKind::ResourcePack) => "Ресурспак",
            ProjectKind::Content(mods::ContentKind::Shader) => "Шейдер",
            ProjectKind::Modpack => "Модпак",
        }
    }

    /// Versions are tied to a mod loader (shaders and resource packs are not).
    fn has_loaders(self) -> bool {
        match self {
            ProjectKind::Content(k) => k.uses_loader(),
            ProjectKind::Modpack => true,
        }
    }
}

/// Card tabs.
pub(super) const TAB_ABOUT: usize = 0;
pub(super) const TAB_VERSIONS: usize = 1;
pub(super) const TAB_DEPS: usize = 2;
pub(super) const TAB_GALLERY: usize = 3;

/// How many version rows are shown before «Показать ещё».
const VERSIONS_PAGE: usize = 40;

/// Preview and full size of gallery pictures, size of pictures in descriptions.
const THUMB_SIDE: u32 = 480;
const FULL_SIDE: u32 = 1920;
const BODY_SIDE: u32 = 1100;

pub(super) struct ProjectView {
    pub hit: modrinth::SearchHit,
    pub kind: ProjectKind,
    pub tab: usize,
    pub project: Option<modrinth::Project>,
    pub body: Vec<Block>,
    pub gallery: Vec<modrinth::GalleryImage>,
    pub project_job: Option<Job<modrinth::Project>>,
    pub project_error: Option<String>,
    pub versions: Vec<modrinth::Version>,
    pub versions_job: Option<Job<Vec<modrinth::Version>>>,
    pub versions_error: Option<String>,
    pub filter_game: Option<String>,
    pub filter_loader: Option<String>,
    pub only_fitting: bool,
    pub versions_shown: usize,
    /// Gallery picture opened large.
    pub lightbox: Option<usize>,
    /// When the large picture changed (it fades in).
    pub lightbox_t0: f64,
    /// Versions whose list of changes is open, and the changes read as Markdown.
    pub expanded: std::collections::HashSet<String>,
    pub changelogs: std::collections::HashMap<String, Vec<Block>>,
    /// Dependencies tab: the version looked at, what was found, the request.
    pub deps_version: Option<String>,
    pub deps: Option<super::deps::Loaded>,
    pub deps_job: Option<Job<super::deps::Loaded>>,
    pub deps_error: Option<String>,
}

/// State of a remote picture.
pub(super) enum Pic {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

// ---------------------------------------------------------------- formatting

/// 950, 11,4 тыс, 73,6 млн.
pub(super) fn format_count(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        let s = if v >= 100.0 { format!("{v:.0}") } else { format!("{v:.1}").replace('.', ",") };
        format!("{} {unit}", s.trim_end_matches(",0"))
    };
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => short(n as f64 / 1e3, "тыс"),
        1_000_000..=999_999_999 => short(n as f64 / 1e6, "млн"),
        _ => short(n as f64 / 1e9, "млрд"),
    }
}

/// `2024-05-01T10:00:00Z` -> `01.05.2024`.
pub(super) fn format_date(iso: &str) -> String {
    let d = iso.get(..10).unwrap_or("");
    let p: Vec<&str> = d.split('-').collect();
    if p.len() == 3 && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit())) {
        format!("{}.{}.{}", p[2], p[1], p[0])
    } else {
        String::new()
    }
}

/// `fabric` -> `Fabric`, any other Modrinth loader as it is, capitalised.
fn loader_name(l: &str) -> String {
    match l {
        "fabric" | "quilt" | "forge" | "neoforge" => loader_title(l).to_string(),
        "liteloader" => "LiteLoader".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Release-like game version (`1.21.1`, `26.3`), not a snapshot.
fn is_release(v: &str) -> bool {
    !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Newest first: releases by their numbers, then everything else.
fn cmp_game_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |v: &str| (is_release(v), v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>());
    key(b).cmp(&key(a)).then_with(|| a.cmp(b))
}

/// `1.21.4, 1.21.3, 1.21.1 +5` (newest first).
pub(super) fn game_versions_label(list: &[String]) -> String {
    let mut v: Vec<&String> = list.iter().collect();
    v.sort_by(|a, b| cmp_game_versions(a, b));
    let shown: Vec<&str> = v.iter().take(3).map(|s| s.as_str()).collect();
    let more = v.len().saturating_sub(3);
    if more > 0 { format!("{} +{more}", shown.join(", ")) } else { shown.join(", ") }
}

/// Game versions and loaders met in the versions, for the filters.
pub(super) fn filter_options(versions: &[modrinth::Version]) -> (Vec<String>, Vec<String>) {
    let mut games: Vec<String> = Vec::new();
    let mut loaders: Vec<String> = Vec::new();
    for v in versions {
        for g in &v.game_versions {
            if !games.contains(g) {
                games.push(g.clone());
            }
        }
        for l in &v.loaders {
            if !loaders.contains(l) {
                loaders.push(l.clone());
            }
        }
    }
    games.sort_by(|a, b| cmp_game_versions(a, b));
    (games, loaders)
}

/// Does this version work in the instance? (Modpacks make a new instance: never "fits".)
pub(super) fn version_fits(v: &modrinth::Version, kind: ProjectKind, inst: Option<&Instance>) -> bool {
    let (ProjectKind::Content(k), Some(inst)) = (kind, inst) else { return false };
    let any_game = inst.game_version.is_empty() || inst.game_version == "latest";
    let game_ok = any_game || v.game_versions.iter().any(|g| *g == inst.game_version);
    let loader_ok = !k.uses_loader() || mods::mod_loaders(inst).iter().any(|l| v.loaders.iter().any(|x| x == l));
    game_ok && loader_ok
}

/// Modrinth page of a project.
fn modrinth_url(hit: &modrinth::SearchHit) -> String {
    format!("https://modrinth.com/{}/{}", hit.project_type, hit.slug.clone().unwrap_or_else(|| hit.project_id.clone()))
}

fn web_link(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Bright colour of a release type.
fn type_color(t: &str) -> egui::Color32 {
    match t {
        "beta" => theme::yellow(),
        "alpha" => theme::danger(),
        _ => theme::green_dark(),
    }
}

fn type_label(t: &str) -> &str {
    match t {
        "release" => "Релиз",
        "beta" => "Бета",
        "alpha" => "Альфа",
        other => other,
    }
}

// ---------------------------------------------------------------- opening and loading

impl LimeApp {
    /// Open the card of a catalog project (`tab` = which tab to show first).
    pub(super) fn open_project(&mut self, hit: &modrinth::SearchHit, kind: ProjectKind, tab: usize) {
        let now = self.ctx.input(|i| i.time);
        // the card of a resource pack works with resource packs of the target instance, etc.
        if let ProjectKind::Content(k) = kind {
            if k != self.content_kind {
                self.content_kind = k;
                self.reset_mod_search();
            }
        }
        let mut view = ProjectView {
            hit: hit.clone(),
            kind,
            tab,
            project: None,
            body: Vec::new(),
            gallery: Vec::new(),
            project_job: None,
            project_error: None,
            versions: Vec::new(),
            versions_job: None,
            versions_error: None,
            filter_game: None,
            filter_loader: None,
            only_fitting: false,
            versions_shown: VERSIONS_PAGE,
            lightbox: None,
            lightbox_t0: now,
            expanded: Default::default(),
            changelogs: Default::default(),
            deps_version: None,
            deps: None,
            deps_job: None,
            deps_error: None,
        };
        Self::load_project(&mut view);
        Self::load_versions(&mut view);
        self.project_view = Some(view);
        self.mod_install_report = None;
        self.page = Page::Project;
    }

    fn load_project(view: &mut ProjectView) {
        let id = view.hit.project_id.clone();
        let client = install::make_client();
        view.project_error = None;
        view.project_job = Some(start_detached(&client, move |c| modrinth::get_project(c, &id)));
    }

    fn load_versions(view: &mut ProjectView) {
        let id = view.hit.project_id.clone();
        let client = install::make_client();
        view.versions_error = None;
        view.versions_job = Some(start_detached(&client, move |c| modrinth::get_project_versions(c, &id)));
    }

    /// Take in finished requests of the open card.
    pub(super) fn poll_project(&mut self) {
        let Some(view) = self.project_view.as_mut() else { return };
        if let Some(res) = view.project_job.as_ref().and_then(drain) {
            view.project_job = None;
            match res {
                Ok(p) => {
                    view.body = markdown::parse(p.body.as_deref().unwrap_or(""));
                    view.gallery = p.gallery_sorted();
                    view.project = Some(p);
                }
                Err(e) => view.project_error = Some(e),
            }
        }
        if let Some(res) = view.versions_job.as_ref().and_then(drain) {
            view.versions_job = None;
            match res {
                Ok(mut list) => {
                    // newest first, whatever order the server answered in
                    list.sort_by(|a, b| b.date_published.cmp(&a.date_published));
                    view.versions = list;
                }
                Err(e) => view.versions_error = Some(e),
            }
        }
    }

    /// Picture from the internet: from memory, the disk cache or a download in the
    /// background (`start` = allowed to start the download now).
    pub(super) fn remote_picture(&mut self, url: &str, max_side: u32, start: bool) -> Pic {
        let key = format!("g{max_side}:{url}");
        if let Some(t) = self.pics.get(&key) {
            return t.clone().map(Pic::Ready).unwrap_or(Pic::Failed);
        }
        if let Some(job) = self.icon_jobs.get(&key) {
            if let Some(res) = drain(job) {
                self.icon_jobs.remove(&key);
                let tex = res.ok().map(|(rgba, w, h)| {
                    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    self.ctx.load_texture(format!("pic-{key}"), img, egui::TextureOptions::LINEAR)
                });
                self.pics.insert(key, tex.clone());
                return tex.map(Pic::Ready).unwrap_or(Pic::Failed);
            }
            return Pic::Loading;
        }
        if !web_link(url) {
            self.pics.insert(key, None);
            return Pic::Failed;
        }
        if start && self.running_picture_jobs() < 6 {
            let (url_owned, cache) = (url.to_string(), self.root.join("cache").join("pictures"));
            let client = install::make_client();
            let job = start_detached(&client, move |c| images::cached_picture(c, &url_owned, &cache, max_side));
            self.icon_jobs.insert(key, job);
        }
        Pic::Loading
    }
}

// ---------------------------------------------------------------- installing

impl LimeApp {
    /// Install one version of the card's project.
    fn install_from_card(&mut self, view: &ProjectView, version: Option<modrinth::Version>) {
        match view.kind {
            ProjectKind::Modpack => {
                let pack = match &version {
                    Some(v) => match modrinth::mrpack_of(v) {
                        Some(p) => Some(p),
                        None => {
                            self.toast = Some("Не удалось: в этой версии нет файла .mrpack".to_string());
                            return;
                        }
                    },
                    None => None,
                };
                let name = self.unique_name(view.hit.title.trim());
                self.start_pack_install_named(&view.hit, name, pack);
            }
            ProjectKind::Content(kind) => {
                let Some(target) = self.state.instances.get(self.active_index()).cloned() else {
                    self.toast = Some("Сначала создайте сборку на вкладке «Библиотека»".to_string());
                    return;
                };
                let installed = self.installed_item(view.kind, &view.hit.project_id);
                let version = match version {
                    Some(v) => v,
                    None if view.versions_job.is_none() && view.versions_error.is_none() => {
                        match view.versions.iter().find(|v| version_fits(v, view.kind, Some(&target))) {
                            Some(v) => v.clone(),
                            None => {
                                self.toast = Some(format!("Не удалось: нет версии под сборку «{}» ({})", target.name, super::loader_label(&target)));
                                return;
                            }
                        }
                    }
                    // versions are not here yet: let the server pick the newest compatible one
                    None => {
                        let want = mods::Want { kind, project_id: view.hit.project_id.clone(), title: view.hit.title.clone(), version: None };
                        self.install_wants(vec![want], false);
                        return;
                    }
                };
                let client = install::make_client();
                let root = self.root.clone();
                let prog = Arc::new(install::InstallProgress::new());
                let prog_ref = prog.clone();
                self.mod_install_job = Some(start_detached(&client, move |c| match installed {
                    // another version of an installed project replaces it (never two copies side by side)
                    Some(item) => {
                        let path = mods::apply_update(c, &root, &target, kind, &item, &version, Some(&prog_ref))?;
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        Ok(mods::InstallReport { installed: vec![name], warnings: Vec::new() })
                    }
                    None => mods::install(c, &root, &target, kind, &version, Some(&prog_ref)),
                }));
                self.mod_install_prog = Some(prog);
                self.mod_install_report = None;
            }
        }
    }
}

// ---------------------------------------------------------------- the page

impl LimeApp {
    /// The installed file of a project in the target instance, once the list of
    /// that instance (and content type) has been read.
    pub(super) fn installed_item(&self, kind: ProjectKind, project_id: &str) -> Option<mods::InstalledItem> {
        let ProjectKind::Content(k) = kind else { return None };
        let inst = self.state.instances.get(self.active_index())?;
        if self.installed_for.as_ref() != Some(&(inst.name.clone(), k)) {
            return None;
        }
        self.installed_items.iter().find(|i| i.project_id.as_deref() == Some(project_id)).cloned()
    }

    pub(super) fn draw_project_page(&mut self, ui: &mut egui::Ui) {
        let Some(mut view) = self.project_view.take() else {
            self.page = Page::Mods;
            return;
        };
        if matches!(view.kind, ProjectKind::Content(_)) {
            self.ensure_installed_list();
        }
        ui.add_space(12.0);
        self.draw_project_header(ui, &mut view);
        ui.add_space(10.0);

        // a modpack has no dependencies tab; the others keep their order
        let all = [
            (TAB_ABOUT, Icon::Info, "Описание"),
            (TAB_VERSIONS, Icon::Stack, "Версии"),
            (TAB_DEPS, Icon::Tree, "Зависимости"),
            (TAB_GALLERY, Icon::Image, "Галерея"),
        ];
        let shown: Vec<(usize, Icon, &str)> = all.iter().copied().filter(|t| t.0 != TAB_DEPS || view.kind != ProjectKind::Modpack).collect();
        if !shown.iter().any(|t| t.0 == view.tab) {
            view.tab = TAB_ABOUT;
        }
        let tabs: Vec<(Option<Icon>, &str)> = shown.iter().map(|t| (Some(t.1), t.2)).collect();
        let mut sel = shown.iter().position(|t| t.0 == view.tab).unwrap_or(0);
        ui.horizontal(|ui| {
            if ui::pill_tabs(ui, &tabs, &mut sel) {
                view.tab = shown[sel].0;
            }
            ui.add_space(8.0);
            let count = |n: usize, busy: bool| if busy { String::new() } else { n.to_string() };
            let note = match view.tab {
                TAB_DEPS => view.deps.as_ref().map(|d| d.items.len().to_string()).unwrap_or_default(),
                TAB_VERSIONS => count(view.versions.len(), view.versions_job.is_some()),
                TAB_GALLERY => count(view.gallery.len(), view.project_job.is_some()),
                _ => String::new(),
            };
            if !note.is_empty() {
                ui.label(RichText::new(note).color(theme::muted()));
            }
        });
        ui.add_space(10.0);
        match view.tab {
            TAB_VERSIONS => self.draw_project_versions(ui, &mut view),
            TAB_DEPS => self.draw_project_deps(ui, &mut view),
            TAB_GALLERY => self.draw_project_gallery(ui, &mut view),
            _ => self.draw_project_about(ui, &mut view),
        }
        ui.add_space(16.0);
        if self.page == Page::Project {
            self.project_view = Some(view);
        }
    }

    fn draw_project_header(&mut self, ui: &mut egui::Ui, view: &mut ProjectView) {
        let hit = view.hit.clone();
        let p = view.project.clone();
        let busy = self.mod_install_job.is_some() || self.pack_install_job.is_some();
        let installed = match view.kind {
            ProjectKind::Content(_) => self.installed_item(view.kind, &hit.project_id).is_some(),
            ProjectKind::Modpack => self.pack_installed(&hit),
        };
        let fav = self.is_favorite(&hit.project_id);
        let mut category_click: Option<String> = None;
        let mut update_click: Option<modrinth::Version> = None;
        let mut remove_path: Option<std::path::PathBuf> = None;
        ui::card().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    if ui::icon_button(ui, Icon::Back, 36.0, false, "Назад в каталог").clicked() {
                        self.page = Page::Mods;
                    }
                });
                self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 96.0);
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 230.0).max(240.0));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(ui::bold(&hit.title, 24.0).color(theme::text()));
                        ui::chip(ui, view.kind.label(), theme::green_dark());
                        if installed {
                            let mark = if view.kind == ProjectKind::Modpack { "установлен" } else { "установлено" };
                            ui::chip(ui, mark, theme::green_dark());
                        }
                    });
                    ui.label(RichText::new(format!("от {}", hit.author)).color(theme::muted()).size(13.0));
                    ui.add_space(2.0);
                    let desc = p.as_ref().map(|p| p.description.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| hit.description.clone());
                    ui.label(RichText::new(desc).color(theme::text()).size(14.0));
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let stat = |ui: &mut egui::Ui, icon: Icon, text: String, tip: &str| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                            ui::paint_icon(ui, icon, r, theme::muted());
                            ui.label(RichText::new(text).color(theme::muted()).size(13.0)).on_hover_text(tip);
                            ui.add_space(8.0);
                        };
                        let downloads = p.as_ref().and_then(|p| p.downloads).unwrap_or(hit.downloads);
                        stat(ui, Icon::Download, format_count(downloads), "Загрузок");
                        let follows = p.as_ref().and_then(|p| p.follows).unwrap_or(hit.follows);
                        stat(ui, Icon::Heart, format_count(follows), "Подписчиков");
                        if let Some(date) = p.as_ref().and_then(|p| p.updated.as_deref()).map(format_date).filter(|d| !d.is_empty()) {
                            stat(ui, Icon::Calendar, format!("обновлён {date}"), "Последнее обновление");
                        }
                    });
                    let cats: Vec<String> = match &p {
                        Some(p) => p.categories.iter().chain(p.additional_categories.iter()).cloned().collect(),
                        None => hit.categories.clone(),
                    };
                    // loaders are shown in the versions; here only the topics
                    let cats: Vec<String> = cats.into_iter().filter(|c| !["fabric", "quilt", "forge", "neoforge", "minecraft", "iris", "optifine", "canvas", "vanilla"].contains(&c.as_str())).collect();
                    if !cats.is_empty() {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for c in cats.iter().take(14) {
                                // a click shows the catalog filtered by this category
                                let r = ui::toggle_chip(ui, &super::catalog::category_label(c), false, None).on_hover_text("Показать похожие в каталоге");
                                ui::reg(&format!("card-category:{c}"), r.rect);
                                if r.clicked() {
                                    category_click = Some(c.clone());
                                }
                            }
                        });
                    }
                });
                ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                    // a modpack can be installed once more (as another instance); content cannot
                    if installed && view.kind != ProjectKind::Modpack {
                        ui::button(ui, ui::BtnKind::Secondary, Some(Icon::Check), "Установлено", false);
                    } else if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить", !busy).clicked() {
                        self.install_from_card(view, None);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui::secondary(ui, Some(Icon::External), "Modrinth") {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(modrinth_url(&hit)));
                        }
                        if super::catalog::favorite_button(ui, fav) {
                            self.toggle_favorite(&hit);
                        }
                        if matches!(view.kind, ProjectKind::Content(_)) && !installed && self.cart_button(ui, &hit.project_id) {
                            self.toggle_cart(&hit, view.kind);
                        }
                    });
                });
            });

            // an installed project: its version, update and removal
            if let Some(item) = self.installed_item(view.kind, &hit.project_id) {
                ui.add_space(8.0);
                let target = self.state.instances.get(self.active_index()).cloned();
                let current = view.versions.iter().find(|v| v.files.iter().any(|f| f.hashes.get("sha1").is_some_and(|h| h.eq_ignore_ascii_case(&item.sha1))));
                let newest = view.versions.iter().find(|v| version_fits(v, view.kind, target.as_ref()));
                let update = newest.filter(|n| match current {
                    Some(c) => c.id != n.id && n.date_published > c.date_published,
                    None => view.versions_job.is_none() && !view.versions.is_empty(),
                });
                let mut do_update: Option<modrinth::Version> = None;
                let mut do_remove = false;
                ui.horizontal_wrapped(|ui| {
                    let have = current.map(|c| c.version_number.clone()).or(item.version_number.clone()).unwrap_or_else(|| item.file_name.clone());
                    ui.label(RichText::new(format!("В сборке: {}", truncate(&have, 40))).color(theme::text()).size(13.0));
                    if !item.enabled {
                        ui::chip(ui, "выключен", theme::yellow());
                    }
                    match update {
                        Some(n) => {
                            let label = format!("Обновить до {}", truncate(&n.version_number, 30));
                            if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Up), &label, !busy).clicked() {
                                do_update = Some(n.clone());
                            }
                        }
                        None if view.versions_job.is_none() && current.is_some() => {
                            ui.label(RichText::new("· это самая новая версия для сборки").color(theme::muted()).size(13.0));
                        }
                        None => {}
                    }
                    if ui::button(ui, ui::BtnKind::Danger, Some(Icon::Trash), "Удалить из сборки", !busy).clicked() {
                        do_remove = true;
                    }
                });
                if let Some(v) = do_update {
                    update_click = Some(v);
                }
                if do_remove {
                    remove_path = Some(item.path.clone());
                }
            }

            // where it goes
            ui.add_space(8.0);
            match view.kind {
                ProjectKind::Modpack => {
                    ui.label(RichText::new("Модпак установится как новая сборка в «Библиотеке».").color(theme::muted()).size(12.5));
                }
                ProjectKind::Content(kind) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Установка в сборку").color(theme::muted()).size(13.0));
                        if self.state.instances.is_empty() {
                            ui.label(RichText::new("— сборок пока нет, создайте её в «Библиотеке»").color(theme::yellow()).size(13.0));
                        } else {
                            let names: Vec<String> = self.state.instances.iter().map(|i| i.name.clone()).collect();
                            let mut sel = self.active_index();
                            egui::ComboBox::from_id_salt("projectTarget").selected_text(names[sel].clone()).show_ui(ui, |ui| {
                                for (i, n) in names.iter().enumerate() {
                                    if ui.selectable_label(i == sel, n.clone()).clicked() {
                                        sel = i;
                                    }
                                }
                            });
                            if sel != self.active_instance {
                                self.active_instance = sel;
                                self.reset_mod_search();
                            }
                            if let Some(inst) = self.state.instances.get(self.active_index()) {
                                ui.label(RichText::new(super::loader_label(inst)).color(theme::muted()).size(13.0));
                                if kind == mods::ContentKind::Mod && inst.loader.is_empty() {
                                    ui.label(RichText::new("· без загрузчика модов").color(theme::yellow()).size(13.0));
                                }
                            }
                        }
                    });
                }
            }
        });

        if let Some(v) = update_click {
            self.install_from_card(view, Some(v));
        }
        if let Some(path) = remove_path {
            if let Some(idx) = self.installed_items.iter().position(|i| i.path == path) {
                self.remove_item(idx);
            }
        }
        if let Some(c) = category_click {
            self.filter_by_category(view.kind, &c);
            return;
        }
        let prog = self.mod_install_prog.clone().or_else(|| self.pack_install_prog.clone());
        if let Some(prog) = prog {
            ui.add_space(8.0);
            self.install_progress_bar(ui, &prog, Some(&prog.cancelled));
        }
        if let Some(report) = &self.mod_install_report {
            ui.add_space(6.0);
            ui.label(RichText::new(report).color(theme::success()));
        }
    }

    // ---- description ----

    fn draw_project_about(&mut self, ui: &mut egui::Ui, view: &mut ProjectView) {
        if let Some(err) = view.project_error.clone() {
            if super::error_banner(ui, "Не удалось загрузить описание", &err) {
                Self::load_project(view);
            }
            return;
        }
        let Some(p) = view.project.clone() else {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Загружаем описание…").color(theme::muted()));
            });
            return;
        };
        let links: Vec<(&str, String)> = [
            ("Исходный код", p.source_url.clone()),
            ("Ошибки", p.issues_url.clone()),
            ("Вики", p.wiki_url.clone()),
            ("Discord", p.discord_url.clone()),
        ]
        .into_iter()
        .filter_map(|(l, u)| u.filter(|u| web_link(u)).map(|u| (l, u)))
        .collect();
        ui::card().inner_margin(egui::Margin::same(18)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !links.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for (label, url) in &links {
                        ui.hyperlink_to(RichText::new(*label).color(theme::green_dark()).size(13.0), url);
                        ui.add_space(10.0);
                    }
                });
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
            }
            if view.body.is_empty() {
                ui.label(RichText::new("Автор не добавил описание.").color(theme::muted()));
            } else {
                let blocks = std::mem::take(&mut view.body);
                self.draw_markdown(ui, &blocks);
                view.body = blocks;
            }
        });
    }

    pub(super) fn draw_markdown(&mut self, ui: &mut egui::Ui, blocks: &[Block]) {
        ui.spacing_mut().item_spacing.y = 6.0;
        let mut i = 0;
        while i < blocks.len() {
            match &blocks[i] {
                Block::Heading(level, spans) => {
                    let size = match level {
                        1 => 22.0,
                        2 => 19.0,
                        3 => 17.0,
                        _ => 15.5,
                    };
                    ui.add_space(if *level <= 2 { 10.0 } else { 6.0 });
                    spans_ui(ui, spans, size, theme::text(), true);
                }
                Block::Paragraph(spans) => spans_ui(ui, spans, 14.0, theme::text(), false),
                Block::Item(depth, marker, spans) => {
                    ui.horizontal(|ui| {
                        ui.add_space(6.0 + *depth as f32 * 18.0);
                        ui.label(RichText::new(marker).color(theme::green_dark()).size(14.0));
                        ui.vertical(|ui| spans_ui(ui, spans, 14.0, theme::text(), false));
                    });
                }
                Block::Quote(spans) => {
                    let out = ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.vertical(|ui| spans_ui(ui, spans, 14.0, theme::muted(), false));
                    });
                    let r = out.response.rect;
                    ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(3.0, r.height())), 0.0, theme::green());
                }
                Block::Code(code) => {
                    egui::Frame::new().fill(theme::input()).corner_radius(egui::CornerRadius::same(if theme::pixel() { 2 } else { 8 })).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(code).monospace().size(13.0).color(theme::text()));
                    });
                }
                Block::Rule => {
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                }
                Block::Row(_) => {
                    let start = i;
                    while i + 1 < blocks.len() && matches!(blocks[i + 1], Block::Row(_)) {
                        i += 1;
                    }
                    egui::Grid::new(("md-table", start)).striped(true).spacing(egui::vec2(16.0, 6.0)).show(ui, |ui| {
                        for (n, b) in blocks[start..=i].iter().enumerate() {
                            if let Block::Row(cells) = b {
                                for cell in cells {
                                    ui.vertical(|ui| spans_ui(ui, cell, 13.5, theme::text(), n == 0));
                                }
                                ui.end_row();
                            }
                        }
                    });
                }
                Block::Image { .. } => {
                    let start = i;
                    while i + 1 < blocks.len() && matches!(blocks[i + 1], Block::Image { .. }) {
                        i += 1;
                    }
                    ui.horizontal_wrapped(|ui| {
                        for b in &blocks[start..=i] {
                            if let Block::Image { url, alt, link } = b {
                                self.markdown_image(ui, url, alt, link.as_deref());
                            }
                        }
                    });
                }
            }
            i += 1;
        }
    }

    fn markdown_image(&mut self, ui: &mut egui::Ui, url: &str, alt: &str, link: Option<&str>) {
        let avail = ui.available_width().max(60.0);
        // ask for the picture only once it scrolls into view
        let probe = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(avail.min(480.0), 120.0));
        match self.remote_picture(url, BODY_SIDE, ui.is_rect_visible(probe)) {
            Pic::Failed => {}
            Pic::Loading => {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(avail.min(480.0), 120.0), egui::Sense::hover());
                ui::paint_notched(ui.painter(), rect, if theme::pixel() { 4.0 } else { 0.0 }, theme::input());
                egui::Spinner::new().paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)));
            }
            Pic::Ready(tex) => {
                let size = tex.size_vec2();
                let scale = (avail / size.x).min(1.0);
                let sense = if link.is_some_and(web_link) { egui::Sense::click() } else { egui::Sense::hover() };
                let (rect, resp) = ui.allocate_exact_size(size * scale, sense);
                let radius = egui::CornerRadius::same(if theme::pixel() { 2 } else { 6 });
                egui::Image::from_texture(egui::load::SizedTexture::from_handle(&tex)).corner_radius(radius).paint_at(ui, rect);
                let resp = if alt.is_empty() { resp } else { resp.on_hover_text(alt) };
                if let Some(l) = link.filter(|l| web_link(l)) {
                    if resp.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(l));
                    }
                    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                }
            }
        }
    }

    // ---- versions ----

    fn draw_project_versions(&mut self, ui: &mut egui::Ui, view: &mut ProjectView) {
        if let Some(err) = view.versions_error.clone() {
            if super::error_banner(ui, "Не удалось загрузить версии", &err) {
                Self::load_versions(view);
            }
            return;
        }
        if view.versions_job.is_some() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Загружаем версии…").color(theme::muted()));
            });
            return;
        }
        if view.versions.is_empty() {
            ui.label(RichText::new("У проекта нет опубликованных версий.").color(theme::muted()));
            return;
        }
        let target = match view.kind {
            ProjectKind::Content(_) => self.state.instances.get(self.active_index()).cloned(),
            ProjectKind::Modpack => None,
        };
        let (games, loaders) = filter_options(&view.versions);
        let show_loaders = view.kind.has_loaders();

        // filters
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new("Версия игры").color(theme::muted()).size(13.0));
            let all_games = "Все".to_string();
            egui::ComboBox::from_id_salt("versionsGame")
                .selected_text(view.filter_game.clone().unwrap_or(all_games.clone()))
                .height(320.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(view.filter_game.is_none(), all_games).clicked() {
                        view.filter_game = None;
                    }
                    for g in &games {
                        if ui.selectable_label(view.filter_game.as_deref() == Some(g), g).clicked() {
                            view.filter_game = Some(g.clone());
                        }
                    }
                });
            if show_loaders {
                ui.label(RichText::new("Загрузчик").color(theme::muted()).size(13.0));
                egui::ComboBox::from_id_salt("versionsLoader")
                    .selected_text(view.filter_loader.as_deref().map(loader_name).unwrap_or_else(|| "Все".into()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(view.filter_loader.is_none(), "Все").clicked() {
                            view.filter_loader = None;
                        }
                        for l in &loaders {
                            if ui.selectable_label(view.filter_loader.as_deref() == Some(l), loader_name(l)).clicked() {
                                view.filter_loader = Some(l.clone());
                            }
                        }
                    });
            }
            if let Some(inst) = &target {
                let resp = ui.checkbox(&mut view.only_fitting, format!("Только для «{}»", truncate(&inst.name, 28)));
                ui::reg("versions:only-fitting", resp.rect);
            }
        });
        ui.add_space(8.0);

        let list: Vec<&modrinth::Version> = view
            .versions
            .iter()
            .filter(|v| view.filter_game.as_ref().is_none_or(|g| v.game_versions.contains(g)))
            .filter(|v| view.filter_loader.as_ref().is_none_or(|l| v.loaders.contains(l)))
            .filter(|v| !view.only_fitting || version_fits(v, view.kind, target.as_ref()))
            .collect();
        if list.is_empty() {
            ui.label(RichText::new("Нет версий под выбранные фильтры.").color(theme::muted()));
            return;
        }
        let total = list.len();
        let busy = self.mod_install_job.is_some() || self.pack_install_job.is_some();
        let installed_sha = self.installed_item(view.kind, &view.hit.project_id).map(|i| i.sha1);
        let mut install: Option<modrinth::Version> = None;
        let mut toggled: Option<String> = None;
        ui::card().inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (n, v) in list.iter().take(view.versions_shown).enumerate() {
                if n > 0 {
                    let y = ui.cursor().top() - ui.spacing().item_spacing.y / 2.0;
                    let r = ui.max_rect();
                    ui.painter().hline((r.left() + 8.0)..=(r.right() - 8.0), y, Stroke::new(1.0, theme::divider()));
                }
                let fits = version_fits(v, view.kind, target.as_ref());
                let here = installed_sha.as_deref().is_some_and(|s| v.files.iter().any(|f| f.hashes.get("sha1").is_some_and(|h| h.eq_ignore_ascii_case(s))));
                let has_notes = v.changelog.as_deref().is_some_and(|c| !c.trim().is_empty());
                let open = view.expanded.contains(&v.id);
                let act = version_row(ui, v, fits, here, show_loaders, busy, has_notes, open);
                if act.install {
                    install = Some((*v).clone());
                }
                if act.toggle {
                    toggled = Some(v.id.clone());
                }
                if open && has_notes {
                    let blocks = view.changelogs.entry(v.id.clone()).or_insert_with(|| markdown::parse(v.changelog.as_deref().unwrap_or("")));
                    egui::Frame::new()
                        .inner_margin(egui::Margin { left: 18, right: 12, top: 0, bottom: 10 })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.draw_markdown(ui, blocks);
                        });
                }
            }
        });
        if let Some(id) = toggled {
            if !view.expanded.remove(&id) {
                view.expanded.insert(id);
            }
        }
        if total > view.versions_shown {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui::secondary(ui, None, &format!("Показать ещё ({})", total - view.versions_shown)) {
                    view.versions_shown += VERSIONS_PAGE;
                }
            });
        }
        if let Some(v) = install {
            self.install_from_card(view, Some(v));
        }
    }

    // ---- gallery ----

    fn draw_project_gallery(&mut self, ui: &mut egui::Ui, view: &mut ProjectView) {
        if let Some(err) = view.project_error.clone() {
            if super::error_banner(ui, "Не удалось загрузить галерею", &err) {
                Self::load_project(view);
            }
            return;
        }
        if view.project.is_none() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Загружаем галерею…").color(theme::muted()));
            });
            return;
        }
        if view.gallery.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                if let Some(empty) = &self.assets.empty_state {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 120.0)));
                }
                ui.label(RichText::new("Автор не добавил изображений.").color(theme::muted()));
            });
            return;
        }
        let gap = 12.0;
        let avail = ui.available_width();
        let cols = (((avail + gap) / (260.0 + gap)).floor() as usize).clamp(1, 5);
        let w = (avail - gap * (cols as f32 - 1.0)) / cols as f32;
        let h = (w * 9.0 / 16.0).round();
        let gallery = view.gallery.clone();
        for (row, chunk) in gallery.chunks(cols).enumerate() {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (k, img) in chunk.iter().enumerate() {
                    let idx = row * cols + k;
                    ui.vertical(|ui| {
                        ui.set_width(w);
                        if self.gallery_tile(ui, idx, img, egui::vec2(w, h)) {
                            view.lightbox = Some(idx);
                            view.lightbox_t0 = ui.ctx().input(|i| i.time);
                        }
                        if let Some(t) = img.title.as_deref().filter(|t| !t.trim().is_empty()) {
                            ui.label(RichText::new(truncate(t, 60)).size(13.0).color(theme::text()));
                        }
                    });
                }
            });
            ui.add_space(gap);
        }
    }

    /// One preview of the gallery grid. Returns true when clicked.
    fn gallery_tile(&mut self, ui: &mut egui::Ui, idx: usize, img: &modrinth::GalleryImage, size: egui::Vec2) -> bool {
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        ui::reg(&format!("gallery:{idx}"), rect);
        let hov = ui::anim(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.15);
        let step = if theme::pixel() { 4.0 } else { 0.0 };
        let border = theme::mix(theme::divider(), theme::green(), hov);
        if theme::pixel() {
            ui.painter().extend(ui::pixel_box_shapes(rect, step, theme::input(), border, 1.5));
        } else {
            ui.painter().rect(rect, egui::CornerRadius::same(10), theme::input(), Stroke::new(1.0 + hov, border), egui::StrokeKind::Inside);
        }
        let inner = rect.shrink(if theme::pixel() { 3.0 } else { 2.0 });
        match self.remote_picture(&img.url, THUMB_SIDE, ui.is_rect_visible(rect)) {
            Pic::Ready(tex) => {
                // cover the tile; the picture leans in a little under the pointer
                let uv = cover_uv(tex.size_vec2(), inner.size(), 0.04 * hov);
                let radius = if theme::pixel() { egui::CornerRadius::same(2) } else { egui::CornerRadius::same(8) };
                egui::Image::from_texture(egui::load::SizedTexture::from_handle(&tex)).uv(uv).corner_radius(radius).paint_at(ui, inner);
                if img.featured {
                    let r = egui::Rect::from_min_size(inner.min + egui::vec2(8.0, 8.0), egui::vec2(120.0, 22.0));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    ui::chip(&mut child, "главное", theme::green_dark());
                }
            }
            Pic::Loading => {
                egui::Spinner::new().paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(22.0, 22.0)));
            }
            Pic::Failed => {
                ui::paint_icon(ui, Icon::Image, egui::Rect::from_center_size(rect.center(), egui::vec2(28.0, 28.0)), theme::muted());
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ZoomIn);
        }
        resp.clicked()
    }

    /// The gallery picture opened large, over everything, with arrows.
    pub(super) fn draw_lightbox(&mut self, ctx: &egui::Context) {
        let Some(mut view) = self.project_view.take() else { return };
        if let Some(idx) = view.lightbox {
            self.lightbox_ui(ctx, &mut view, idx);
        }
        self.project_view = Some(view);
    }

    fn lightbox_ui(&mut self, ctx: &egui::Context, view: &mut ProjectView, idx: usize) {
        let n = view.gallery.len();
        if n == 0 {
            view.lightbox = None;
            return;
        }
        let idx = idx.min(n - 1);
        let now = ctx.input(|i| i.time);
        let mut go: i64 = 0;
        let mut close = false;
        ctx.input(|i| {
            if i.key_pressed(egui::Key::ArrowLeft) {
                go = -1;
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                go = 1;
            }
            if i.key_pressed(egui::Key::Escape) {
                close = true;
            }
        });
        let screen = ctx.content_rect();
        let appear = if fx::enabled() { fx::ease_out_cubic(fx::progress(now, view.lightbox_t0, 0.0, 0.22)) } else { 1.0 };
        if appear < 1.0 {
            ctx.request_repaint();
        }
        let img = view.gallery[idx].clone();
        egui::Area::new(egui::Id::new("gallery-lightbox"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // the backdrop takes every click under it; a click on it closes the picture
                let back = ui.allocate_rect(screen, egui::Sense::click());
                ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(215));
                ui::reg("lightbox", screen);

                let area = egui::Rect::from_min_max(screen.min + egui::vec2(90.0, 60.0), screen.max - egui::vec2(90.0, 110.0));
                // the large picture; the preview stands in while it loads
                let full = self.remote_picture(img.full_url(), FULL_SIDE, true);
                for k in [idx + 1, idx + n - 1] {
                    let neighbour = view.gallery[k % n].full_url().to_string();
                    let _ = self.remote_picture(&neighbour, FULL_SIDE, true);
                }
                let (tex, loading) = match full {
                    Pic::Ready(t) => (Some(t), false),
                    Pic::Loading => (match self.remote_picture(&img.url, THUMB_SIDE, true) { Pic::Ready(t) => Some(t), _ => None }, true),
                    Pic::Failed => (match self.remote_picture(&img.url, THUMB_SIDE, true) { Pic::Ready(t) => Some(t), _ => None }, false),
                };
                let mut pic_rect = egui::Rect::from_center_size(area.center(), egui::vec2(160.0, 90.0));
                if let Some(tex) = tex {
                    let s = tex.size_vec2();
                    let scale = (area.width() / s.x).min(area.height() / s.y);
                    let size = s * scale * (0.96 + 0.04 * appear);
                    pic_rect = egui::Rect::from_center_size(area.center() + egui::vec2(0.0, 10.0 * (1.0 - appear)), size);
                    egui::Image::from_texture(egui::load::SizedTexture::from_handle(&tex))
                        .tint(egui::Color32::WHITE.gamma_multiply(appear))
                        .paint_at(ui, pic_rect);
                }
                // a click on the picture itself does not close it
                ui.allocate_rect(pic_rect, egui::Sense::click());
                if loading {
                    egui::Spinner::new().color(egui::Color32::WHITE).paint_at(ui, egui::Rect::from_center_size(pic_rect.center(), egui::vec2(28.0, 28.0)));
                }

                // caption
                let mut caption = format!("{} / {n}", idx + 1);
                if let Some(t) = img.title.as_deref().filter(|t| !t.trim().is_empty()) {
                    caption = format!("{caption}  ·  {t}");
                }
                let cap_rect = egui::Rect::from_min_max(egui::pos2(area.left(), area.bottom() + 14.0), egui::pos2(area.right(), screen.bottom() - 10.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(cap_rect).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
                    ui.label(ui::bold(caption, 15.0).color(egui::Color32::WHITE));
                    if let Some(d) = img.description.as_deref().filter(|d| !d.trim().is_empty()) {
                        ui.label(RichText::new(truncate(d, 300)).color(egui::Color32::from_gray(200)).size(13.0));
                    }
                });

                // arrows and close
                let key_at = |ui: &mut egui::Ui, center: egui::Pos2, icon: Icon, tip: &str| {
                    let r = egui::Rect::from_center_size(center, egui::vec2(46.0, 42.0));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                    ui::icon_key(&mut child, icon, ui::BtnKind::Secondary, tip).clicked()
                };
                if n > 1 {
                    if key_at(ui, egui::pos2(screen.left() + 45.0, area.center().y), Icon::CaretLeft, "Предыдущее") {
                        go = -1;
                    }
                    if key_at(ui, egui::pos2(screen.right() - 45.0, area.center().y), Icon::CaretRight, "Следующее") {
                        go = 1;
                    }
                }
                if key_at(ui, egui::pos2(screen.right() - 45.0, screen.top() + 34.0), Icon::Close, "Закрыть просмотр") {
                    close = true;
                }
                if back.clicked() {
                    close = true;
                }
            });
        if close {
            view.lightbox = None;
        } else if go != 0 && n > 1 {
            view.lightbox = Some(((idx as i64 + go).rem_euclid(n as i64)) as usize);
            view.lightbox_t0 = now;
        }
    }
}

/// One row of the versions list. Returns true when its install key was pressed.
/// `installed`: this very version is in the target instance.
/// What a click in a version row asked for.
#[derive(Default)]
struct RowAction {
    install: bool,
    toggle: bool,
}

#[allow(clippy::too_many_arguments)]
fn version_row(ui: &mut egui::Ui, v: &modrinth::Version, fits: bool, installed: bool, show_loaders: bool, busy: bool, has_notes: bool, notes_open: bool) -> RowAction {
    let mut act = RowAction::default();
    let fill = if fits { theme::mix(theme::card(), theme::green(), 0.10) } else { egui::Color32::TRANSPARENT };
    let out = egui::Frame::new()
        .fill(fill)
        .corner_radius(egui::CornerRadius::same(if theme::pixel() { 0 } else { 8 }))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 250.0).max(220.0));
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(ui::bold(truncate(&v.version_number, 40), 15.0).color(theme::text()));
                        ui::chip(ui, type_label(&v.version_type), type_color(&v.version_type));
                        if installed {
                            ui::chip(ui, "установлена", theme::green_dark());
                        } else if fits {
                            ui::chip(ui, "подходит", theme::green_dark());
                        }
                        if !v.name.is_empty() && v.name != v.version_number {
                            ui.label(RichText::new(truncate(&v.name, 60)).color(theme::muted()).size(12.5));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if show_loaders {
                            for l in &v.loaders {
                                ui::chip(ui, &loader_name(l), theme::text());
                            }
                        }
                        if has_notes {
                            let icon = if notes_open { Icon::CaretUp } else { Icon::CaretDown };
                            let r = ui::toggle_chip(ui, "Изменения", notes_open, Some(icon));
                            ui::reg(&format!("changes:{}", v.id), r.rect);
                            if r.clicked() {
                                act.toggle = true;
                            }
                        }
                        let games = game_versions_label(&v.game_versions);
                        if !games.is_empty() {
                            ui.label(RichText::new(format!("Minecraft {games}")).color(theme::muted()).size(12.5))
                                .on_hover_text(v.game_versions.join(", "));
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let kind = if fits { ui::BtnKind::Primary } else { ui::BtnKind::Secondary };
                    let resp = if installed {
                        ui::button(ui, ui::BtnKind::Secondary, Some(Icon::Check), "", false).on_hover_text("Эта версия уже установлена")
                    } else if busy {
                        ui::button(ui, ui::BtnKind::Secondary, Some(Icon::Download), "", false)
                    } else {
                        ui::icon_key(ui, Icon::Download, kind, "Установить эту версию")
                    };
                    act.install = resp.clicked();
                    ui.add_space(6.0);
                    let size = v.primary_file().map(|f| f.size).filter(|s| *s > 0).map(super::format_size).unwrap_or_default();
                    ui.vertical(|ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                            let date = format_date(&v.date_published);
                            if !date.is_empty() {
                                ui.label(RichText::new(date).color(theme::text()).size(13.0));
                            }
                            let mut meta = Vec::new();
                            if !size.is_empty() {
                                meta.push(size);
                            }
                            if v.downloads > 0 {
                                meta.push(format!("↓ {}", format_count(v.downloads)));
                            }
                            ui.label(RichText::new(meta.join(" · ")).color(theme::muted()).size(12.0));
                        });
                    });
                });
            });
        });
    if fits {
        let r = out.response.rect;
        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(3.0, r.height())), 0.0, theme::green());
    }
    act
}

/// Inline text of a description: labels and links that wrap together.
fn spans_ui(ui: &mut egui::Ui, spans: &[Span], size: f32, color: egui::Color32, bold_all: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);
        for s in spans {
            let mut rt = RichText::new(&s.text).size(size).color(color);
            if s.bold || bold_all {
                rt = rt.family(ui::bold_family());
            }
            if s.italic {
                rt = rt.italics();
            }
            if s.code {
                rt = rt.code();
            }
            if s.strike {
                rt = rt.strikethrough();
            }
            match s.link.as_deref().filter(|l| web_link(l)) {
                Some(url) => {
                    ui.hyperlink_to(rt.color(theme::green_dark()), url);
                }
                None => {
                    ui.label(rt);
                }
            }
        }
    });
}

/// UV rectangle that fills `target` with a picture of `pic` size (cropping the
/// overflow), zoomed in by `zoom` (0 = none).
fn cover_uv(pic: egui::Vec2, target: egui::Vec2, zoom: f32) -> egui::Rect {
    let (pa, ta) = (pic.x / pic.y.max(1.0), target.x / target.y.max(1.0));
    let (mut w, mut h) = if pa > ta { (ta / pa, 1.0) } else { (1.0, pa / ta) };
    w *= 1.0 - zoom;
    h *= 1.0 - zoom;
    egui::Rect::from_center_size(egui::pos2(0.5, 0.5), egui::vec2(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(games: &[&str], loaders: &[&str]) -> modrinth::Version {
        serde_json::from_value(serde_json::json!({
            "id": "v", "project_id": "p", "name": "n", "version_number": "1", "version_type": "release",
            "game_versions": games, "loaders": loaders,
        }))
        .unwrap()
    }

    #[test]
    fn counts_and_dates_read_well() {
        assert_eq!(format_count(950), "950");
        assert_eq!(format_count(11_376), "11,4 тыс");
        assert_eq!(format_count(73_581_300), "73,6 млн");
        assert_eq!(format_count(2_000_000), "2 млн");
        assert_eq!(format_count(150_000), "150 тыс");
        assert_eq!(format_date("2024-05-01T10:00:00Z"), "01.05.2024");
        assert_eq!(format_date("garbage"), "");
    }

    #[test]
    fn game_versions_are_newest_first() {
        let list: Vec<String> = ["1.20.1", "1.21", "26.3", "1.21.4", "24w10a"].iter().map(|s| s.to_string()).collect();
        assert_eq!(game_versions_label(&list), "26.3, 1.21.4, 1.21 +2");
        let (games, loaders) = filter_options(&[version(&["1.20.1"], &["fabric"]), version(&["1.21.1", "1.20.1"], &["quilt", "fabric"])]);
        assert_eq!(games, ["1.21.1", "1.20.1"]);
        assert_eq!(loaders, ["fabric", "quilt"]);
    }

    #[test]
    fn fitting_versions_follow_the_instance() {
        let inst = Instance { name: "q".into(), game_version: "1.21.1".into(), loader: "quilt".into(), ..Default::default() };
        let mod_kind = ProjectKind::Content(mods::ContentKind::Mod);
        assert!(version_fits(&version(&["1.21.1"], &["fabric"]), mod_kind, Some(&inst)), "Quilt runs Fabric mods");
        assert!(!version_fits(&version(&["1.21.1"], &["forge"]), mod_kind, Some(&inst)));
        assert!(!version_fits(&version(&["1.20.1"], &["quilt"]), mod_kind, Some(&inst)));
        let shader = ProjectKind::Content(mods::ContentKind::Shader);
        assert!(version_fits(&version(&["1.21.1"], &["iris"]), shader, Some(&inst)), "shaders do not care about the loader");
        assert!(!version_fits(&version(&["1.21.1"], &["fabric"]), ProjectKind::Modpack, Some(&inst)));
        assert!(!version_fits(&version(&["1.21.1"], &["fabric"]), mod_kind, None));
    }

    #[test]
    fn cover_crops_the_long_side() {
        let uv = cover_uv(egui::vec2(200.0, 100.0), egui::vec2(100.0, 100.0), 0.0);
        assert!((uv.width() - 0.5).abs() < 1e-4 && (uv.height() - 1.0).abs() < 1e-4);
        let uv = cover_uv(egui::vec2(100.0, 200.0), egui::vec2(160.0, 90.0), 0.0);
        assert!((uv.width() - 1.0).abs() < 1e-4 && uv.height() < 0.3);
    }
}
