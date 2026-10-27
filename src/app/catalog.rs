//! Catalog 2.0: searching Modrinth with sorting, categories and endless
//! scrolling, the "already installed" marks and favourite projects.

use std::collections::HashSet;

use eframe::egui::{self, RichText, Stroke};

use super::project::{self, ProjectKind};
use super::{Job, LimeApp, ModsView, Page, catalog_card, drain, error_banner, start_detached, truncate};
use crate::core::{install, modrinth, mods};
use crate::theme;
use crate::ui::{self, Icon};

/// Results per page.
pub(super) const PAGE: u32 = 20;

/// Sort order of a catalog search.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sort {
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
}

impl Sort {
    pub const ALL: [Sort; 5] = [Sort::Relevance, Sort::Downloads, Sort::Follows, Sort::Newest, Sort::Updated];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Relevance => "Подходящие",
            Sort::Downloads => "Популярные",
            Sort::Follows => "По подписчикам",
            Sort::Newest => "Новые",
            Sort::Updated => "Обновлённые",
        }
    }

    /// Modrinth `index`.
    pub fn index(self) -> &'static str {
        match self {
            Sort::Relevance => "relevance",
            Sort::Downloads => "downloads",
            Sort::Follows => "follows",
            Sort::Newest => "newest",
            Sort::Updated => "updated",
        }
    }
}

/// What a search is limited to: the project type and the compatibility filters
/// of the moment it started (the next pages keep them).
#[derive(Clone, Debug)]
pub(super) struct Scope {
    pub project_type: &'static str,
    pub game: Option<String>,
    pub loaders: Vec<&'static str>,
}

/// One request, owned, to send to a worker thread.
struct Request {
    query: String,
    scope: Scope,
    categories: Vec<String>,
    index: &'static str,
    offset: u32,
}

impl Request {
    fn run(&self, c: &reqwest::blocking::Client) -> Result<modrinth::SearchResponse, String> {
        modrinth::search_page(
            c,
            &modrinth::SearchQuery {
                query: &self.query,
                project_type: self.scope.project_type,
                game_version: self.scope.game.as_deref(),
                loaders: &self.scope.loaders,
                categories: &self.categories,
                index: self.index,
                limit: PAGE,
                offset: self.offset,
            },
        )
    }
}

/// One searchable list of the catalog (content or modpacks): what is asked and what came back.
pub(super) struct Browse {
    pub query: String,
    pub sort: Sort,
    /// Chosen categories (all must match).
    pub categories: Vec<String>,
    /// Project type the chosen categories belong to.
    pub categories_type: Option<&'static str>,
    pub results: Vec<modrinth::SearchHit>,
    /// How many projects match on the server.
    pub total: u64,
    pub searched: bool,
    pub error: Option<String>,
    pub job: Option<Job<modrinth::SearchResponse>>,
    pub more_job: Option<Job<modrinth::SearchResponse>>,
    pub more_error: Option<String>,
    /// The category panel is open.
    pub filters_open: bool,
    scope: Option<Scope>,
}

impl Browse {
    pub fn new() -> Self {
        Browse {
            query: String::new(),
            sort: Sort::Relevance,
            categories: Vec::new(),
            categories_type: None,
            results: Vec::new(),
            total: 0,
            searched: false,
            error: None,
            job: None,
            more_job: None,
            more_error: None,
            filters_open: false,
            scope: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some() || self.more_job.is_some()
    }

    /// More results wait on the server.
    pub fn has_more(&self) -> bool {
        self.searched && (self.results.len() as u64) < self.total
    }

    /// Forget the results (the instance or the content type changed).
    pub fn reset(&mut self) {
        self.results.clear();
        self.total = 0;
        self.searched = false;
        self.error = None;
        self.more_error = None;
        self.job = None;
        self.more_job = None;
    }

    fn request(&self, scope: &Scope, offset: u32) -> Request {
        Request { query: self.query.trim().to_string(), scope: scope.clone(), categories: self.categories.clone(), index: self.sort.index(), offset }
    }

    /// Start a new search: the first page.
    pub fn search(&mut self, scope: Scope) {
        // categories of another project type mean nothing here
        if self.categories_type.is_some_and(|t| t != scope.project_type) {
            self.categories.clear();
            self.categories_type = None;
        }
        self.error = None;
        self.more_error = None;
        self.more_job = None;
        let req = self.request(&scope, 0);
        self.scope = Some(scope);
        let client = install::make_client();
        self.job = Some(start_detached(&client, move |c| req.run(c)));
    }

    /// Ask for the next page of the last search.
    pub fn load_more(&mut self) {
        let Some(scope) = self.scope.clone() else { return };
        if self.busy() || !self.has_more() {
            return;
        }
        self.more_error = None;
        let req = self.request(&scope, self.results.len() as u32);
        let client = install::make_client();
        self.more_job = Some(start_detached(&client, move |c| req.run(c)));
    }

    /// Take in finished searches. True when a new first page arrived.
    pub fn poll(&mut self) -> bool {
        let mut fresh = false;
        if let Some(res) = self.job.as_ref().and_then(drain) {
            self.job = None;
            match res {
                Ok(r) => {
                    self.total = r.total_hits.max(r.hits.len() as u64);
                    self.results = r.hits;
                    self.searched = true;
                    self.error = None;
                    fresh = true;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(res) = self.more_job.as_ref().and_then(drain) {
            self.more_job = None;
            match res {
                Ok(r) => {
                    let before = self.results.len();
                    for h in r.hits {
                        // the list may shift between pages: skip what is already shown
                        if !self.results.iter().any(|x| x.project_id == h.project_id) {
                            self.results.push(h);
                        }
                    }
                    self.total = r.total_hits;
                    if self.results.len() == before {
                        // nothing new came: that is the end of the list
                        self.total = self.results.len() as u64;
                    }
                }
                Err(e) => self.more_error = Some(e),
            }
        }
        fresh
    }
}

// ---------------------------------------------------------------- categories

/// Category list of Modrinth, loaded once when the filter panel is first opened.
#[derive(Default)]
pub(super) struct Tags {
    list: Option<Vec<modrinth::CategoryTag>>,
    job: Option<Job<Vec<modrinth::CategoryTag>>>,
    error: Option<String>,
}

enum TagsState {
    Loading,
    Failed(String),
    Ready(Vec<(String, Vec<String>)>),
}

impl Tags {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn state(&mut self, project_type: &str) -> TagsState {
        if let Some(res) = self.job.as_ref().and_then(drain) {
            self.job = None;
            match res {
                Ok(list) => self.list = Some(list),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(list) = &self.list {
            return TagsState::Ready(group_tags(list, project_type));
        }
        if let Some(e) = &self.error {
            return TagsState::Failed(e.clone());
        }
        if self.job.is_none() {
            let client = install::make_client();
            self.job = Some(start_detached(&client, modrinth::category_tags));
        }
        TagsState::Loading
    }
}

/// Categories of one project type in groups: (group title, names), in a friendly order.
pub(super) fn group_tags(tags: &[modrinth::CategoryTag], project_type: &str) -> Vec<(String, Vec<String>)> {
    const ORDER: [&str; 4] = ["categories", "features", "resolutions", "performance impact"];
    const PERFORMANCE: [&str; 5] = ["potato", "low", "medium", "high", "screenshot"];
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for t in tags.iter().filter(|t| t.project_type == project_type) {
        match groups.iter_mut().find(|g| g.0 == t.header) {
            Some(g) => g.1.push(t.name.clone()),
            None => groups.push((t.header.clone(), vec![t.name.clone()])),
        }
    }
    groups.sort_by_key(|g| ORDER.iter().position(|o| *o == g.0).unwrap_or(ORDER.len()));
    for (header, names) in &mut groups {
        match header.as_str() {
            "performance impact" => names.sort_by_key(|n| PERFORMANCE.iter().position(|p| p == n).unwrap_or(PERFORMANCE.len())),
            "resolutions" => names.sort_by_key(|n| n.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u32>().unwrap_or(u32::MAX)),
            _ => names.sort_by_key(|n| category_label(n).to_lowercase()),
        }
    }
    groups.into_iter().map(|(h, n)| (header_label(&h), n)).collect()
}

fn header_label(h: &str) -> String {
    match h {
        "categories" => "Категории".into(),
        "features" => "Особенности".into(),
        "resolutions" => "Разрешение".into(),
        "performance impact" => "Нагрузка на компьютер".into(),
        other => capitalize(other),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Russian name of a Modrinth category (unknown ones are shown as they are).
pub(super) fn category_label(name: &str) -> String {
    let ru = match name {
        "adventure" => "Приключения",
        "cursed" => "Проклятое",
        "decoration" => "Декор",
        "economy" => "Экономика",
        "equipment" => "Снаряжение",
        "food" => "Еда",
        "game-mechanics" => "Игровые механики",
        "library" => "Библиотеки",
        "magic" => "Магия",
        "management" => "Управление",
        "minigame" => "Мини-игры",
        "mobs" => "Мобы",
        "optimization" => "Оптимизация",
        "social" => "Общение",
        "storage" => "Хранение",
        "technology" => "Технологии",
        "transportation" => "Транспорт",
        "utility" => "Утилиты",
        "worldgen" => "Генерация мира",
        "challenging" => "Испытания",
        "combat" => "Сражения",
        "kitchen-sink" => "Всё и сразу",
        "lightweight" => "Лёгкие",
        "multiplayer" => "Мультиплеер",
        "quests" => "Квесты",
        "audio" => "Звуки",
        "blocks" => "Блоки",
        "core-shaders" => "Шейдеры ядра",
        "entities" => "Существа",
        "environment" => "Окружение",
        "fonts" => "Шрифты",
        "gui" => "Интерфейс",
        "items" => "Предметы",
        "locale" => "Языки",
        "modded" => "Для модов",
        "models" => "Модели",
        "realistic" => "Реалистичные",
        "simplistic" => "Простые",
        "themed" => "Тематические",
        "tweaks" => "Доработки",
        "vanilla-like" => "Как в ванилле",
        "8x-" => "8x и меньше",
        "512x+" => "512x и больше",
        "cartoon" => "Мультяшные",
        "fantasy" => "Фэнтези",
        "semi-realistic" => "Полуреалистичные",
        "atmosphere" => "Атмосфера",
        "bloom" => "Свечение",
        "colored-lighting" => "Цветной свет",
        "foliage" => "Листва",
        "path-tracing" => "Трассировка пути",
        "pbr" => "PBR",
        "reflections" => "Отражения",
        "shadows" => "Тени",
        "potato" => "Очень слабый",
        "low" => "Слабый",
        "medium" => "Средний",
        "high" => "Мощный",
        "screenshot" => "Для скриншотов",
        _ => return capitalize(&name.replace('-', " ")),
    };
    ru.to_string()
}

/// 73581300 -> "73 581 300".
pub(super) fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Kind of a project by its Modrinth type.
pub(super) fn kind_of(project_type: &str) -> ProjectKind {
    match project_type {
        "modpack" => ProjectKind::Modpack,
        "resourcepack" => ProjectKind::Content(mods::ContentKind::ResourcePack),
        "shader" => ProjectKind::Content(mods::ContentKind::Shader),
        _ => ProjectKind::Content(mods::ContentKind::Mod),
    }
}

// ---------------------------------------------------------------- shared widgets

/// Search field, sort order and categories of a list; `extra` adds more controls
/// to the filter row (it returns true when they changed). Returns true when a new
/// search is needed.
fn toolbar(
    ui: &mut egui::Ui,
    b: &mut Browse,
    tags: &mut Tags,
    field: &str,
    hint: &str,
    project_type: &'static str,
    busy_spinner: bool,
    extra: impl FnOnce(&mut egui::Ui) -> bool,
) -> bool {
    let mut search = false;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 130.0).max(200.0);
        let resp = ui.add(egui::TextEdit::singleline(&mut b.query).hint_text(hint).desired_width(w));
        ui::reg(field, resp.rect);
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui::primary(ui, Some(Icon::Search), "Найти") || enter {
            search = true;
        }
        if busy_spinner {
            ui.add(egui::Spinner::new());
        }
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new("Сортировка").color(theme::muted()));
        let before = b.sort;
        let combo = egui::ComboBox::from_id_salt(("catalog-sort", field)).selected_text(b.sort.label()).show_ui(ui, |ui| {
            for s in Sort::ALL {
                let r = ui.selectable_value(&mut b.sort, s, s.label());
                ui::reg(&format!("sort:{}", s.label()), r.rect);
            }
        });
        ui::reg("sort", combo.response.rect);
        if b.sort != before {
            search = true;
        }
        let n = b.categories.len();
        let label = if n == 0 { "Категории".to_string() } else { format!("Категории: {n}") };
        let kind = if b.filters_open { ui::BtnKind::Primary } else { ui::BtnKind::Secondary };
        let toggle = ui::button(ui, kind, Some(Icon::Funnel), &label, true);
        ui::reg("categories:toggle", toggle.rect);
        if toggle.clicked() {
            b.filters_open = !b.filters_open;
        }
        if extra(ui) {
            search = true;
        }
    });

    // chosen categories: a click takes one away
    if !b.categories.is_empty() {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            let mut remove = None;
            for (i, c) in b.categories.iter().enumerate() {
                let r = ui::toggle_chip(ui, &category_label(c), true, Some(Icon::Close)).on_hover_text("Убрать фильтр");
                ui::reg(&format!("selected:{c}"), r.rect);
                if r.clicked() {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                b.categories.remove(i);
                search = true;
            }
            let reset = ui.add(egui::Label::new(RichText::new("Сбросить").color(theme::green_dark()).size(13.0)).sense(egui::Sense::click()));
            ui::reg("categories:reset", reset.rect);
            if reset.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                b.categories.clear();
                search = true;
            }
        });
    }

    if b.filters_open {
        ui.add_space(8.0);
        ui::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match tags.state(project_type) {
                TagsState::Loading => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(RichText::new("Загружаем категории…").color(theme::muted()));
                    });
                }
                TagsState::Failed(e) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Не удалось загрузить категории: {}", truncate(&e, 120))).color(theme::danger()).size(13.0));
                        if ui::secondary(ui, Some(Icon::Refresh), "Повторить") {
                            tags.error = None;
                        }
                    });
                }
                TagsState::Ready(groups) => {
                    for (gi, (title, names)) in groups.iter().enumerate() {
                        if gi > 0 {
                            ui.add_space(8.0);
                        }
                        ui.label(ui::bold(title, 13.5).color(theme::muted()));
                        ui.add_space(2.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for name in names {
                                let on = b.categories.contains(name);
                                let r = ui::toggle_chip(ui, &category_label(name), on, None);
                                ui::reg(&format!("category:{name}"), r.rect);
                                if r.clicked() {
                                    if on {
                                        b.categories.retain(|c| c != name);
                                    } else {
                                        b.categories.push(name.clone());
                                    }
                                    b.categories_type = Some(project_type);
                                    search = true;
                                }
                            }
                        });
                    }
                }
            }
        });
    }
    if search {
        b.categories_type = Some(project_type);
    }
    search
}

/// "Найдено: N" above a list.
fn found_line(ui: &mut egui::Ui, b: &Browse) {
    if b.searched && !b.results.is_empty() {
        ui.label(RichText::new(format!("Найдено: {}", group_digits(b.total))).color(theme::muted()).size(12.5));
        ui.add_space(4.0);
    }
}

/// The end of a result list: it asks for the next page as soon as it scrolls into
/// view. Returns true when it is time to load more.
fn pager(ui: &mut egui::Ui, b: &mut Browse) -> bool {
    ui.add_space(4.0);
    if let Some(err) = b.more_error.clone() {
        if error_banner(ui, "Не удалось загрузить продолжение списка", &err) {
            b.more_error = None;
            return true;
        }
        return false;
    }
    if b.has_more() {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        ui::reg("catalog:more", rect);
        egui::Spinner::new().paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(22.0, 22.0)));
        return b.more_job.is_none() && b.job.is_none() && ui.is_rect_visible(rect);
    }
    if b.searched && b.results.len() > PAGE as usize {
        ui.add_space(4.0);
        ui.vertical_centered(|ui| ui.label(RichText::new("Это всё, что нашлось").color(theme::muted()).size(12.5)));
    }
    false
}

/// Star key of a project. Returns true when clicked.
pub(super) fn favorite_button(ui: &mut egui::Ui, on: bool) -> bool {
    let (icon, tip) = if on { (Icon::StarFill, "Убрать из избранного") } else { (Icon::Star, "В избранное") };
    ui::icon_button(ui, icon, 34.0, on, tip).clicked()
}

/// Empty list picture with a line under it.
fn empty_state(app: &LimeApp, ui: &mut egui::Ui, msg: &str) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        if let Some(empty) = &app.assets.empty_state {
            ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 120.0)));
        }
        ui.label(RichText::new(msg).color(theme::muted()));
    });
}

// ---------------------------------------------------------------- the lists

impl LimeApp {
    /// Search the content catalog (mods / resource packs / shaders) from the first page.
    pub(super) fn search_mods(&mut self) {
        let kind = self.content_kind;
        let target = self.state.instances.get(self.active_index());
        let (game, loaders) = match (target, self.only_compatible) {
            (Some(t), true) => (Some(t.game_version.clone()), if kind.uses_loader() { mods::mod_loaders(t) } else { Vec::new() }),
            _ => (None, Vec::new()),
        };
        self.mods.search(Scope { project_type: kind.project_type(), game, loaders });
    }

    pub(super) fn search_packs(&mut self) {
        self.packs.search(Scope { project_type: "modpack", game: None, loaders: Vec::new() });
    }

    /// Make sure the installed files of the target instance are being read (for the marks).
    pub(super) fn ensure_installed_list(&mut self) {
        let Some(inst) = self.state.instances.get(self.active_index()).cloned() else { return };
        if self.installed_for.as_ref() != Some(&(inst.name.clone(), self.content_kind)) && self.installed_job.is_none() {
            self.load_installed(&inst);
        }
    }

    /// Projects installed in the target instance (current content type), as far as known.
    pub(super) fn installed_project_ids(&self) -> HashSet<String> {
        let Some(inst) = self.state.instances.get(self.active_index()) else { return HashSet::new() };
        if self.installed_for.as_ref() != Some(&(inst.name.clone(), self.content_kind)) {
            return HashSet::new();
        }
        self.installed_items.iter().filter_map(|i| i.project_id.clone()).collect()
    }

    /// Is this modpack already installed as an instance?
    pub(super) fn pack_installed(&self, hit: &modrinth::SearchHit) -> bool {
        let slug = hit.slug.clone().unwrap_or_else(|| hit.project_id.clone());
        self.state.instances.iter().any(|i| i.modpack_slug.as_deref() == Some(slug.as_str()) || i.modpack_slug.as_deref() == Some(hit.project_id.as_str()))
    }

    pub(super) fn is_favorite(&self, project_id: &str) -> bool {
        self.state.favorites.iter().any(|f| f.project_id == project_id)
    }

    pub(super) fn toggle_favorite(&mut self, hit: &modrinth::SearchHit) {
        if let Some(i) = self.state.favorites.iter().position(|f| f.project_id == hit.project_id) {
            self.state.favorites.remove(i);
        } else {
            self.state.favorites.push(hit.clone());
        }
        self.save_state();
    }

    /// Show the catalog filtered by one category (a click on a category of a project card).
    pub(super) fn filter_by_category(&mut self, kind: ProjectKind, name: &str) {
        match kind {
            ProjectKind::Modpack => {
                self.packs.categories = vec![name.to_string()];
                self.packs.categories_type = Some("modpack");
                self.mods_view = ModsView::Modpacks;
                self.search_packs();
            }
            ProjectKind::Content(k) => {
                if k != self.content_kind {
                    self.content_kind = k;
                    self.reset_mod_search();
                }
                self.mods.categories = vec![name.to_string()];
                self.mods.categories_type = Some(k.project_type());
                self.mods_view = ModsView::Catalog;
                self.search_mods();
            }
        }
        self.page = Page::Mods;
    }

    /// «Каталог»: mods, resource packs or shaders for the target instance.
    pub(super) fn draw_mods_catalog(&mut self, ui: &mut egui::Ui) {
        let kind = self.content_kind;
        // first open (or after switching instance/type): show popular items right away
        if !self.mods.searched && !self.mods.busy() && self.mods.error.is_none() {
            self.search_mods();
        }
        let hint = match kind {
            mods::ContentKind::Mod => "Поиск модов (пусто — популярные)",
            mods::ContentKind::ResourcePack => "Поиск ресурспаков",
            mods::ContentKind::Shader => "Поиск шейдеров",
        };
        let spinner = self.mods.job.is_some();
        let only = &mut self.only_compatible;
        let search = toolbar(ui, &mut self.mods, &mut self.tags, "field:mods-search", hint, kind.project_type(), spinner, |ui| {
            ui.checkbox(only, "Только совместимые").changed()
        });
        if search {
            self.search_mods();
        }
        self.ensure_installed_list();

        ui.add_space(8.0);
        if let Some(prog) = &self.mod_install_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
            ui.add_space(8.0);
        }
        if let Some(report) = &self.mod_install_report {
            ui.label(RichText::new(report).color(theme::success()));
            ui.add_space(6.0);
        }
        if let Some(err) = self.mods.error.clone() {
            if error_banner(ui, "Не удалось выполнить поиск", &err) {
                self.mods.error = None;
                self.search_mods();
            }
            ui.add_space(8.0);
        }
        if self.mods.results.is_empty() {
            let msg = if self.mods.job.is_some() {
                "Ищем на Modrinth…"
            } else if self.mods.error.is_some() {
                ""
            } else if self.mods.searched {
                "Ничего не найдено. Попробуйте другой запрос, снимите фильтры или «Только совместимые»."
            } else {
                "Нажмите «Найти» — без запроса покажем популярное."
            };
            empty_state(self, ui, msg);
            return;
        }
        found_line(ui, &self.mods);

        let busy = self.mod_install_job.is_some();
        let installed = self.installed_project_ids();
        let pkind = ProjectKind::Content(kind);
        for idx in 0..self.mods.results.len() {
            let hit = self.mods.results[idx].clone();
            let is_installed = installed.contains(&hit.project_id);
            let fav = self.is_favorite(&hit.project_id);
            let opened = catalog_card(ui, &hit.project_id, false, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 52.0);
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 290.0).max(200.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&hit.title).size(16.0).family(ui::bold_family()));
                            ui.label(RichText::new(format!("от {}", hit.author)).color(theme::muted()).size(12.5));
                            if is_installed {
                                ui::chip(ui, "установлено", theme::green_dark());
                            }
                        });
                        ui.label(RichText::new(truncate(&hit.description, 140)).color(theme::muted()).size(13.0));
                        let is_release = |v: &&String| v.chars().all(|c| c.is_ascii_digit() || c == '.');
                        let latest: Vec<&str> = hit.versions.iter().filter(is_release).rev().take(4).map(|s| s.as_str()).collect();
                        ui.label(
                            RichText::new(format!("↓ {} · для {}", project::format_count(hit.downloads), latest.join(", ")))
                                .color(theme::muted())
                                .size(12.0),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if is_installed {
                            ui::button(ui, ui::BtnKind::Secondary, Some(Icon::Check), "Установлено", false);
                        } else if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить", !busy).clicked() {
                            self.quick_install(&hit);
                        }
                        if ui::secondary(ui, None, "Версии") {
                            self.open_project(&hit, pkind, project::TAB_VERSIONS);
                        }
                        if favorite_button(ui, fav) {
                            self.toggle_favorite(&hit);
                        }
                    });
                });
            });
            if opened && self.page == Page::Mods {
                self.open_project(&hit, pkind, project::TAB_ABOUT);
            }
            ui.add_space(4.0);
        }
        if pager(ui, &mut self.mods) {
            self.mods.load_more();
        }
    }

    /// «Модпаки»: every pack becomes a new instance.
    pub(super) fn draw_modpacks(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if !self.packs.searched && !self.packs.busy() && self.packs.error.is_none() {
            self.search_packs();
        }
        let spinner = self.packs.job.is_some();
        if toolbar(ui, &mut self.packs, &mut self.tags, "field:packs-search", "Поиск модпаков (пусто — популярные)", "modpack", spinner, |_| false) {
            self.search_packs();
        }

        ui.add_space(8.0);
        if let Some(prog) = &self.pack_install_prog {
            self.install_progress_bar(ui, prog, Some(&prog.cancelled));
            ui.add_space(8.0);
        }
        if let Some(err) = self.packs.error.clone() {
            if error_banner(ui, "Не удалось выполнить поиск", &err) {
                self.packs.error = None;
                self.search_packs();
            }
            ui.add_space(8.0);
        }

        // install panel for the chosen modpack
        if let Some(hit) = self.selected_pack.and_then(|i| self.packs.results.get(i).cloned()) {
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

        if self.packs.results.is_empty() {
            let msg = if self.packs.job.is_some() {
                "Ищем на Modrinth…"
            } else if self.packs.error.is_some() {
                ""
            } else if self.packs.searched {
                "Ничего не найдено. Попробуйте другой запрос или снимите фильтры."
            } else {
                "Нажмите «Найти» — без запроса покажем популярные модпаки."
            };
            empty_state(self, ui, msg);
            return;
        }
        found_line(ui, &self.packs);

        for idx in 0..self.packs.results.len() {
            let hit = self.packs.results[idx].clone();
            let selected = self.selected_pack == Some(idx);
            let installed = self.pack_installed(&hit);
            let fav = self.is_favorite(&hit.project_id);
            let opened = catalog_card(ui, &hit.project_id, selected, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 52.0);
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 290.0).max(200.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&hit.title).size(16.0).family(ui::bold_family()));
                            ui.label(RichText::new(format!("от {}", hit.author)).color(theme::muted()).size(12.5));
                            if installed {
                                ui::chip(ui, "установлен", theme::green_dark());
                            }
                        });
                        ui.label(RichText::new(truncate(&hit.description, 140)).color(theme::muted()).size(13.0));
                        ui.label(
                            RichText::new(format!("↓ {} · версий игры: {}", project::format_count(hit.downloads), hit.versions.len()))
                                .color(theme::muted())
                                .size(12.0),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let busy = self.pack_install_job.is_some();
                        if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить", !busy).clicked() {
                            self.selected_pack = Some(idx);
                        }
                        if ui::secondary(ui, None, "Версии") {
                            self.open_project(&hit, ProjectKind::Modpack, project::TAB_VERSIONS);
                        }
                        if favorite_button(ui, fav) {
                            self.toggle_favorite(&hit);
                        }
                    });
                });
            });
            if opened && self.page == Page::Mods {
                self.open_project(&hit, ProjectKind::Modpack, project::TAB_ABOUT);
            }
            ui.add_space(4.0);
        }
        if pager(ui, &mut self.packs) {
            self.packs.load_more();
        }
    }

    /// «Избранное»: starred projects of every type.
    pub(super) fn draw_favorites(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if self.state.favorites.is_empty() {
            empty_state(self, ui, "Отмечайте проекты звёздочкой в каталоге — они соберутся здесь.");
            return;
        }
        let groups = [
            (ProjectKind::Content(mods::ContentKind::Mod), Icon::Puzzle, "Моды"),
            (ProjectKind::Content(mods::ContentKind::ResourcePack), Icon::Image, "Ресурспаки"),
            (ProjectKind::Content(mods::ContentKind::Shader), Icon::Palette, "Шейдеры"),
            (ProjectKind::Modpack, Icon::Package, "Модпаки"),
        ];
        for (kind, icon, title) in groups {
            let list: Vec<modrinth::SearchHit> = self.state.favorites.iter().filter(|f| kind_of(&f.project_type) == kind).cloned().collect();
            if list.is_empty() {
                continue;
            }
            ui.add_space(6.0);
            ui::section(ui, icon, &format!("{title} · {}", list.len()));
            ui.add_space(4.0);
            for hit in list {
                let opened = catalog_card(ui, &hit.project_id, false, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 44.0);
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 160.0).max(200.0));
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&hit.title).size(15.5).family(ui::bold_family()));
                                ui.label(RichText::new(format!("от {}", hit.author)).color(theme::muted()).size(12.5));
                            });
                            ui.label(RichText::new(truncate(&hit.description, 140)).color(theme::muted()).size(13.0));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::secondary(ui, None, "Версии") {
                                self.open_project(&hit, kind, project::TAB_VERSIONS);
                            }
                            if favorite_button(ui, true) {
                                self.toggle_favorite(&hit);
                            }
                        });
                    });
                });
                if opened && self.page == Page::Mods {
                    self.open_project(&hit, kind, project::TAB_ABOUT);
                }
                ui.add_space(4.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str, t: &str, header: &str) -> modrinth::CategoryTag {
        modrinth::CategoryTag { name: name.into(), project_type: t.into(), header: header.into() }
    }

    #[test]
    fn categories_are_grouped_and_ordered() {
        let tags = [
            tag("shadows", "shader", "features"),
            tag("high", "shader", "performance impact"),
            tag("potato", "shader", "performance impact"),
            tag("cartoon", "shader", "categories"),
            tag("magic", "mod", "categories"),
            tag("low", "shader", "performance impact"),
        ];
        let g = group_tags(&tags, "shader");
        let titles: Vec<&str> = g.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(titles, ["Категории", "Особенности", "Нагрузка на компьютер"]);
        assert_eq!(g[2].1, ["potato", "low", "high"], "weakest computer first");
        assert!(g.iter().all(|x| !x.1.contains(&"magic".to_string())), "only the project type asked for");
        let res = group_tags(&[tag("512x+", "resourcepack", "resolutions"), tag("16x", "resourcepack", "resolutions"), tag("8x-", "resourcepack", "resolutions")], "resourcepack");
        assert_eq!(res[0].1, ["8x-", "16x", "512x+"]);
    }

    #[test]
    fn labels_and_numbers_read_well() {
        assert_eq!(category_label("game-mechanics"), "Игровые механики");
        assert_eq!(category_label("brand-new-thing"), "Brand new thing");
        assert_eq!(group_digits(73_581_300), "73 581 300");
        assert_eq!(group_digits(950), "950");
        assert_eq!(kind_of("shader"), ProjectKind::Content(mods::ContentKind::Shader));
        assert_eq!(kind_of("modpack"), ProjectKind::Modpack);
        assert_eq!(kind_of("datapack"), ProjectKind::Content(mods::ContentKind::Mod));
    }
}
