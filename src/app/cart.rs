//! The install cart: collect several mods, resource packs and shaders from the
//! catalog, see which of them fit the target instance and put them in with one button.

use std::sync::Arc;

use eframe::egui::{self, RichText};

use super::project::{self, ProjectKind};
use super::{Job, LimeApp, ModsView, Page, drain, start_detached, truncate};
use crate::core::{install, modrinth, mods};
use crate::theme;
use crate::ui::{self, Icon};

pub(super) struct CartItem {
    pub hit: modrinth::SearchHit,
    pub kind: mods::ContentKind,
}

/// Compatibility answers for the instance they were asked about.
#[derive(Default)]
pub(super) struct Cart {
    pub items: Vec<CartItem>,
    /// project id -> newest version that fits (None: there is none); absent while asking.
    pub fit: std::collections::HashMap<String, Option<modrinth::Version>>,
    pub fit_for: Option<String>,
    pub job: Option<Job<Vec<(String, Option<modrinth::Version>)>>>,
    /// The running install is the cart's (the fitting items leave the cart when it ends).
    pub installing: bool,
}

impl Cart {
    pub fn has(&self, project_id: &str) -> bool {
        self.items.iter().any(|i| i.hit.project_id == project_id)
    }
}

impl LimeApp {
    pub(super) fn toggle_cart(&mut self, hit: &modrinth::SearchHit, kind: ProjectKind) {
        let ProjectKind::Content(k) = kind else { return };
        if let Some(i) = self.cart.items.iter().position(|c| c.hit.project_id == hit.project_id) {
            self.cart.items.remove(i);
        } else {
            self.cart.items.push(CartItem { hit: hit.clone(), kind: k });
        }
    }

    /// Ask for versions of the cart's projects that fit the target instance
    /// (again when the instance changes).
    pub(super) fn poll_cart(&mut self) {
        let Some(inst) = self.state.instances.get(self.active_index()).cloned() else { return };
        if let Some(res) = self.cart.job.as_ref().and_then(drain) {
            self.cart.job = None;
            if self.cart.fit_for.as_deref() == Some(inst.name.as_str()) {
                if let Ok(list) = res {
                    self.cart.fit.extend(list);
                }
            }
        }
        if self.cart.items.is_empty() || self.cart.job.is_some() {
            return;
        }
        if self.cart.fit_for.as_deref() != Some(inst.name.as_str()) {
            self.cart.fit.clear();
            self.cart.fit_for = Some(inst.name.clone());
        }
        let unknown: Vec<(String, mods::ContentKind)> =
            self.cart.items.iter().filter(|i| !self.cart.fit.contains_key(&i.hit.project_id)).map(|i| (i.hit.project_id.clone(), i.kind)).collect();
        if unknown.is_empty() {
            return;
        }
        let client = install::make_client();
        self.cart.job = Some(start_detached(&client, move |c| {
            Ok(unknown.into_iter().map(|(id, kind)| (id.clone(), mods::newest_fitting(c, &inst, kind, &id).ok().flatten())).collect())
        }));
    }

    /// Put projects into the target instance (a job like any other install).
    pub(super) fn install_wants(&mut self, wants: Vec<mods::Want>, from_cart: bool) {
        let Some(target) = self.state.instances.get(self.active_index()).cloned() else {
            self.toast = Some("Сначала создайте сборку на вкладке «Библиотека»".to_string());
            return;
        };
        let client = install::make_client();
        let root = self.root.clone();
        let prog = Arc::new(install::InstallProgress::new());
        let prog_ref = prog.clone();
        self.mod_install_job = Some(start_detached(&client, move |c| mods::install_many(c, &root, &target, &wants, Some(&prog_ref))));
        self.mod_install_prog = Some(prog);
        self.mod_install_report = None;
        self.cart.installing = from_cart;
    }

    pub(super) fn install_cart(&mut self) {
        let wants: Vec<mods::Want> = self
            .cart
            .items
            .iter()
            .filter_map(|i| {
                let v = self.cart.fit.get(&i.hit.project_id)?.clone()?;
                Some(mods::Want { kind: i.kind, project_id: i.hit.project_id.clone(), title: i.hit.title.clone(), version: Some(v) })
            })
            .collect();
        if wants.is_empty() {
            self.toast = Some("Не удалось: в корзине нет проектов, подходящих этой сборке".to_string());
            return;
        }
        self.install_wants(wants, true);
    }

    /// An install that came from the cart has ended: what was put in leaves the cart.
    pub(super) fn cart_install_done(&mut self, ok: bool) {
        if !std::mem::take(&mut self.cart.installing) || !ok {
            return;
        }
        let fit = std::mem::take(&mut self.cart.fit);
        self.cart.items.retain(|i| fit.get(&i.hit.project_id).is_none_or(|v| v.is_none()));
        self.cart.fit = fit;
    }

    /// Cart key of a catalog card or the card header. Returns true when clicked.
    pub(super) fn cart_button(&self, ui: &mut egui::Ui, project_id: &str) -> bool {
        let on = self.cart.has(project_id);
        let (icon, tip) = if on { (Icon::CartFill, "Убрать из корзины") } else { (Icon::Cart, "В корзину") };
        ui::icon_button(ui, icon, 34.0, on, tip).clicked()
    }

    pub(super) fn draw_cart(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if self.cart.items.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                if let Some(empty) = &self.assets.empty_state {
                    ui.add(egui::Image::new(egui::load::SizedTexture::from_handle(empty)).max_size(egui::vec2(160.0, 120.0)));
                }
                ui.label(RichText::new("Корзина пуста. Нажимайте на тележку у проектов в каталоге — потом поставите всё одной кнопкой.").color(theme::muted()));
            });
            return;
        }
        let target = self.state.instances.get(self.active_index()).cloned();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Установка в сборку").color(theme::muted()));
            match &target {
                None => {
                    ui.label(RichText::new("— сборок пока нет, создайте её в «Библиотеке»").color(theme::yellow()));
                }
                Some(t) => {
                    let names: Vec<String> = self.state.instances.iter().map(|i| i.name.clone()).collect();
                    let mut sel = self.active_index();
                    egui::ComboBox::from_id_salt("cartTarget").selected_text(t.name.clone()).show_ui(ui, |ui| {
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
                    ui.label(RichText::new(super::loader_label(t)).color(theme::muted()).size(13.0));
                }
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

        let mut remove: Option<usize> = None;
        let mut open: Option<usize> = None;
        for idx in 0..self.cart.items.len() {
            let (hit, kind) = (self.cart.items[idx].hit.clone(), self.cart.items[idx].kind);
            let fit = self.cart.fit.get(&hit.project_id).cloned();
            let opened = super::catalog_card(ui, &format!("cart-{}", hit.project_id), false, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    self.project_icon(ui, &hit.title, hit.icon_url.as_deref(), 44.0);
                    ui.vertical(|ui| {
                        ui.set_max_width((ui.available_width() - 80.0).max(200.0));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(truncate(&hit.title, 50)).size(15.5).family(ui::bold_family()));
                            ui::chip(ui, ProjectKind::Content(kind).label(), theme::muted());
                        });
                        match &fit {
                            None => {
                                ui.horizontal(|ui| {
                                    ui.add(egui::Spinner::new().size(14.0));
                                    ui.label(RichText::new("Проверяем совместимость…").color(theme::muted()).size(12.5));
                                });
                            }
                            Some(Some(v)) => {
                                ui.label(RichText::new(format!("Подходит: версия {}", truncate(&v.version_number, 40))).color(theme::green_dark()).size(12.5));
                            }
                            Some(None) => {
                                ui.label(RichText::new("Нет версии под эту сборку — этот проект пропустим").color(theme::danger()).size(12.5));
                            }
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = ui::icon_button(ui, Icon::Trash, 34.0, false, "Убрать из корзины");
                        if r.clicked() {
                            remove = Some(idx);
                        }
                    });
                });
            });
            if opened {
                open = Some(idx);
            }
            ui.add_space(4.0);
        }
        if let Some(i) = remove {
            self.cart.items.remove(i);
        }
        if let Some(i) = open {
            let (hit, kind) = (self.cart.items[i].hit.clone(), self.cart.items[i].kind);
            self.open_project(&hit, ProjectKind::Content(kind), project::TAB_ABOUT);
        }

        ui.add_space(6.0);
        let fitting = self.cart.items.iter().filter(|i| self.cart.fit.get(&i.hit.project_id).is_some_and(|v| v.is_some())).count();
        let busy = self.mod_install_job.is_some();
        ui.horizontal(|ui| {
            let label = format!("Установить всё ({fitting})");
            if ui::button(ui, ui::BtnKind::Primary, Some(Icon::Download), &label, fitting > 0 && !busy && target.is_some()).clicked() {
                self.install_cart();
            }
            if ui::secondary(ui, Some(Icon::Trash), "Очистить") {
                self.cart.items.clear();
            }
        });
    }

    /// A cart button for the header of the catalog: jumps to the cart.
    pub(super) fn cart_link(&mut self, ui: &mut egui::Ui) {
        if self.cart.items.is_empty() {
            return;
        }
        let label = format!("Корзина · {}", self.cart.items.len());
        if ui::button(ui, ui::BtnKind::Secondary, Some(Icon::CartFill), &label, true).clicked() {
            self.mods_view = ModsView::Cart;
            self.page = Page::Mods;
        }
    }
}
