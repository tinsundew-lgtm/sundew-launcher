//! The «Зависимости» tab of a project card: what a version needs, recommends or
//! cannot work with, what of it is already in the instance, and installing the rest.

use std::collections::HashMap;

use eframe::egui::{self, RichText};

use super::catalog::kind_of;
use super::project::{ProjectKind, ProjectView, version_fits};
use super::{LimeApp, drain, start_detached, truncate};
use crate::core::{install, modrinth, mods};
use crate::theme;
use crate::ui::{self, Icon};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DepKind {
    Required,
    Optional,
    Embedded,
    Incompatible,
}

impl DepKind {
    fn of(s: Option<&str>) -> DepKind {
        match s {
            Some("optional") => DepKind::Optional,
            Some("embedded") => DepKind::Embedded,
            Some("incompatible") => DepKind::Incompatible,
            _ => DepKind::Required,
        }
    }

    fn title(self) -> &'static str {
        match self {
            DepKind::Required => "Обязательные",
            DepKind::Optional => "Необязательные",
            DepKind::Embedded => "Встроены в файл",
            DepKind::Incompatible => "Несовместимы",
        }
    }

    fn note(self) -> &'static str {
        match self {
            DepKind::Required => "Без них проект не заработает. При установке они ставятся сами.",
            DepKind::Optional => "Добавляют возможности, но можно обойтись.",
            DepKind::Embedded => "Уже внутри файла, ставить отдельно не нужно.",
            DepKind::Incompatible => "С этими проектами возникают ошибки: не ставьте их вместе.",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct DepItem {
    pub hit: modrinth::SearchHit,
    pub kind: DepKind,
    /// Version number the author pinned, if any.
    pub pinned: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct Loaded {
    pub version_id: String,
    pub items: Vec<DepItem>,
}

/// Read the dependencies of a version with the names of the projects.
pub(super) fn load(client: &reqwest::blocking::Client, version: &modrinth::Version) -> Result<Loaded, String> {
    // (project id, kind, pinned version number); a dependency may only name a version
    let mut rows: Vec<(String, DepKind, Option<String>)> = Vec::new();
    for d in &version.dependencies {
        let kind = DepKind::of(d.dependency_type.as_deref());
        let (pid, pinned) = match (&d.project_id, &d.version_id) {
            (Some(p), None) => (p.clone(), None),
            (p, Some(vid)) => match modrinth::get_version(client, vid) {
                Ok(v) => (p.clone().unwrap_or(v.project_id.clone()), Some(v.version_number)),
                Err(_) => match p {
                    Some(p) => (p.clone(), None),
                    None => continue,
                },
            },
            (None, None) => continue,
        };
        if !rows.iter().any(|r| r.0 == pid) {
            rows.push((pid, kind, pinned));
        }
    }
    let ids: Vec<String> = rows.iter().map(|r| r.0.clone()).collect();
    let projects: HashMap<String, modrinth::Project> = modrinth::get_projects(client, &ids)?.into_iter().map(|p| (p.id.clone(), p)).collect();
    let mut items: Vec<DepItem> = rows
        .into_iter()
        .map(|(pid, kind, pinned)| {
            let hit = match projects.get(&pid) {
                Some(p) => modrinth::SearchHit {
                    project_id: p.id.clone(),
                    project_type: p.project_type.clone(),
                    slug: p.slug.clone(),
                    author: String::new(),
                    title: p.title.clone(),
                    description: p.description.clone(),
                    downloads: p.downloads.unwrap_or(0),
                    follows: p.follows.unwrap_or(0),
                    icon_url: p.icon_url.clone(),
                    versions: Vec::new(),
                    categories: p.categories.clone(),
                },
                None => modrinth::SearchHit {
                    project_id: pid.clone(),
                    project_type: "mod".into(),
                    slug: None,
                    author: String::new(),
                    title: pid,
                    description: String::new(),
                    downloads: 0,
                    follows: 0,
                    icon_url: None,
                    versions: Vec::new(),
                    categories: Vec::new(),
                },
            };
            DepItem { hit, kind, pinned }
        })
        .collect();
    items.sort_by_key(|i| (i.kind as u8, i.hit.title.to_lowercase()));
    Ok(Loaded { version_id: version.id.clone(), items })
}

impl LimeApp {
    /// The version whose dependencies are shown: the chosen one, else the newest that
    /// fits the target instance, else the newest.
    pub(super) fn deps_version<'a>(&self, view: &'a ProjectView) -> Option<&'a modrinth::Version> {
        if let Some(id) = &view.deps_version {
            if let Some(v) = view.versions.iter().find(|v| &v.id == id) {
                return Some(v);
            }
        }
        let target = self.state.instances.get(self.active_index());
        view.versions.iter().find(|v| version_fits(v, view.kind, target)).or_else(|| view.versions.first())
    }

    /// Start (or take in) the dependency request of the open card.
    pub(super) fn poll_deps(&mut self, view: &mut ProjectView) {
        if let Some(res) = view.deps_job.as_ref().and_then(drain) {
            view.deps_job = None;
            match res {
                Ok(l) => view.deps = Some(l),
                Err(e) => view.deps_error = Some(e),
            }
        }
    }

    fn ensure_deps(&mut self, view: &mut ProjectView) {
        self.poll_deps(view);
        let Some(v) = self.deps_version(view).cloned() else { return };
        let have = view.deps.as_ref().is_some_and(|d| d.version_id == v.id);
        if have || view.deps_job.is_some() || view.deps_error.is_some() {
            return;
        }
        let client = install::make_client();
        view.deps_job = Some(start_detached(&client, move |c| load(c, &v)));
    }

    pub(super) fn draw_project_deps(&mut self, ui: &mut egui::Ui, view: &mut ProjectView) {
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
        self.ensure_deps(view);
        let Some(current) = self.deps_version(view).cloned() else { return };
        let target = self.state.instances.get(self.active_index()).cloned();

        ui.horizontal(|ui| {
            ui.label(RichText::new("Зависимости версии").color(theme::muted()));
            let label = |v: &modrinth::Version| format!("{}{}", truncate(&v.version_number, 40), if version_fits(v, view.kind, target.as_ref()) { " · подходит" } else { "" });
            let mut chosen = current.id.clone();
            egui::ComboBox::from_id_salt("depsVersion").selected_text(label(&current)).height(320.0).show_ui(ui, |ui| {
                for v in view.versions.iter().take(80) {
                    ui.selectable_value(&mut chosen, v.id.clone(), label(v));
                }
            });
            if chosen != current.id {
                view.deps_version = Some(chosen);
                view.deps = None;
                view.deps_error = None;
            }
        });
        ui.add_space(8.0);

        if let Some(err) = view.deps_error.clone() {
            if super::error_banner(ui, "Не удалось загрузить зависимости", &err) {
                view.deps_error = None;
            }
            return;
        }
        let Some(loaded) = view.deps.clone().filter(|d| d.version_id == current.id) else {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Загружаем зависимости…").color(theme::muted()));
            });
            return;
        };
        if loaded.items.is_empty() {
            ui.label(RichText::new("Эта версия не требует других проектов.").color(theme::muted()));
            return;
        }

        let installed = self.installed_project_ids();
        let known_kind = match view.kind {
            ProjectKind::Content(k) => Some(k),
            ProjectKind::Modpack => None,
        };
        let is_installed = |it: &DepItem| known_kind.is_some_and(|k| kind_of(&it.hit.project_type) == ProjectKind::Content(k)) && installed.contains(&it.hit.project_id);
        let busy = self.mod_install_job.is_some();
        let missing: Vec<&DepItem> = loaded.items.iter().filter(|i| i.kind == DepKind::Required && !is_installed(i)).collect();
        let mut wants: Vec<mods::Want> = Vec::new();
        if !missing.is_empty() {
            ui.horizontal(|ui| {
                let label = format!("Установить недостающие обязательные ({})", missing.len());
                if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), &label, !busy).clicked() {
                    wants = missing.iter().map(|i| want_of(i)).collect();
                }
                ui.label(RichText::new("Главный проект при установке подтягивает их сам.").color(theme::muted()).size(12.5));
            });
            ui.add_space(8.0);
        }

        let mut open: Option<DepItem> = None;
        for kind in [DepKind::Required, DepKind::Optional, DepKind::Embedded, DepKind::Incompatible] {
            let group: Vec<&DepItem> = loaded.items.iter().filter(|i| i.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            ui.label(ui::bold(&format!("{} · {}", kind.title(), group.len()), 15.0).color(theme::text()));
            ui.label(RichText::new(kind.note()).color(theme::muted()).size(12.5));
            ui.add_space(4.0);
            for it in group {
                let here = is_installed(it);
                let opened = super::catalog_card(ui, &format!("dep-{}", it.hit.project_id), false, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        self.project_icon(ui, &it.hit.title, it.hit.icon_url.as_deref(), 40.0);
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 190.0).max(200.0));
                            ui.horizontal_wrapped(|ui| {
                                ui.label(ui::bold(&it.hit.title, 15.0).color(theme::text()));
                                ui::chip(ui, kind_label(&it.hit.project_type), theme::muted());
                                if let Some(p) = &it.pinned {
                                    ui::chip(ui, &format!("версия {}", truncate(p, 24)), theme::muted());
                                }
                                if here {
                                    ui::chip(ui, if kind == DepKind::Incompatible { "стоит в сборке!" } else { "установлено" }, if kind == DepKind::Incompatible { theme::danger() } else { theme::green_dark() });
                                }
                            });
                            if !it.hit.description.is_empty() {
                                ui.label(RichText::new(truncate(&it.hit.description, 130)).color(theme::muted()).size(12.5));
                            }
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if matches!(kind, DepKind::Required | DepKind::Optional) && !here {
                                let resp = ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), "Установить", !busy);
                                ui::reg(&format!("dep-install:{}", it.hit.project_id), resp.rect);
                                if resp.clicked() {
                                    wants = vec![want_of(it)];
                                }
                            }
                        });
                    });
                });
                if opened {
                    open = Some((*it).clone());
                }
                ui.add_space(4.0);
            }
            ui.add_space(8.0);
        }
        if !wants.is_empty() {
            self.install_wants(wants, false);
        }
        if let Some(it) = open {
            self.open_project(&it.hit, kind_of(&it.hit.project_type), super::project::TAB_ABOUT);
        }
    }
}

fn want_of(it: &DepItem) -> mods::Want {
    let kind = match kind_of(&it.hit.project_type) {
        ProjectKind::Content(k) => k,
        ProjectKind::Modpack => mods::ContentKind::Mod,
    };
    mods::Want { kind, project_id: it.hit.project_id.clone(), title: it.hit.title.clone(), version: None }
}

fn kind_label(project_type: &str) -> &'static str {
    match kind_of(project_type) {
        ProjectKind::Content(mods::ContentKind::Mod) => "Мод",
        ProjectKind::Content(mods::ContentKind::ResourcePack) => "Ресурспак",
        ProjectKind::Content(mods::ContentKind::Shader) => "Шейдер",
        ProjectKind::Modpack => "Модпак",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_kinds_are_read_with_a_safe_default() {
        assert_eq!(DepKind::of(Some("optional")), DepKind::Optional);
        assert_eq!(DepKind::of(Some("incompatible")), DepKind::Incompatible);
        assert_eq!(DepKind::of(Some("embedded")), DepKind::Embedded);
        assert_eq!(DepKind::of(None), DepKind::Required);
        assert_eq!(DepKind::of(Some("whatever")), DepKind::Required);
        assert!((DepKind::Required as u8) < (DepKind::Incompatible as u8), "required ones come first");
    }
}
