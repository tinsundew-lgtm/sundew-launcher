use super::*;
use crate::core::{capsules, diagnostics};

#[derive(Default)]
pub(super) struct CapsuleUi {
    job: Option<Job<Outcome>>,
    for_name: String,
    loaded: bool,
    items: Vec<capsules::Capsule>,
    label: String,
    worlds: bool,
    automatic: bool,
    lines: Vec<String>,
    confirm: Option<String>,
}
struct Outcome { name: String, items: Vec<capsules::Capsule>, lines: Vec<String>, restored: Option<Instance> }
impl CapsuleUi {
    pub(super) fn busy(&self) -> bool { self.job.is_some() }
    pub(super) fn automatic(&self) -> bool { self.automatic }
}
impl LimeApp {
    pub(super) fn poll_capsules(&mut self) {
        let Some(result) = self.capsules.job.as_ref().and_then(drain) else { return; };
        self.capsules.job = None;
        match result {
            Err(e) => { self.capsules.lines = vec![e.clone()]; self.toast = Some(e); }
            Ok(out) => {
                if let Some(inst) = out.restored {
                    self.upsert_instance(inst); self.save_state(); self.inst_edit_for = None; self.installed_for = None;
                }
                if self.capsules.for_name == out.name {
                    self.capsules.items = out.items; self.capsules.lines = out.lines;
                }
            }
        }
    }
    fn capsule_action(&mut self, inst: &Instance, action: u8, id: String) {
        if self.capsules.job.is_some() { return; }
        let root = self.root.clone(); let inst = inst.clone();
        let label = self.capsules.label.clone(); let worlds = self.capsules.worlds;
        self.capsules.job = Some(start_detached(&install::make_client(), move |_| {
            let mut restored = None;
            let lines = match action {
                1 => { capsules::create(&root,&inst,&label,worlds)?; vec!["Капсула сохранена локально.".into()] }
                2 => { let lines=capsules::compare(&root,&inst,&id)?; if lines.is_empty() {vec!["Изменений нет.".into()]} else {lines} }
                3 => { restored=Some(capsules::restore(&root,&inst,&id)?); vec!["Восстановлено. Состояние до отката сохранено отдельной капсулой.".into()] }
                4 => diagnostics::inspect(&root,&inst)?,
                _ => Vec::new(),
            };
            let mut lines = lines;
            let items = match capsules::list(&root,&inst.name) {
                Ok(items) => items,
                Err(e) => { lines.push(format!("Не удалось обновить список: {e}")); Vec::new() }
            };
            Ok(Outcome {name:inst.name.clone(),items,lines,restored})
        }));
    }
    pub(super) fn draw_capsules(&mut self, ui: &mut egui::Ui, inst: &Instance) {
        if self.capsules.for_name != inst.name {
            self.capsules.for_name=inst.name.clone(); self.capsules.loaded=false;
            self.capsules.items.clear(); self.capsules.lines.clear(); self.capsules.confirm=None;
        }
        if !self.capsules.loaded && self.capsules.job.is_none() { self.capsules.loaded=true; self.capsule_action(inst,0,String::new()); }
        ui.heading("Капсулы и диагностика");
        ui.label("Локальная копия модов, конфигов, ресурспаков, шейдеров, versions и options. Библиотеки, Java и assets не копируются. Капсулы не отправляются в сеть.");
        ui.checkbox(&mut self.capsules.automatic,"Создавать капсулу перед запуском (до закрытия лаунчера)");
        ui.checkbox(&mut self.capsules.worlds,"Включить миры в ручную капсулу (может занять много места)");
        ui.add(egui::TextEdit::singleline(&mut self.capsules.label).hint_text("Название: например, рабочая сборка"));
        let idle = !self.is_busy() && !launch::is_running(&inst.name);
        ui.horizontal(|ui| {
            if ui.add_enabled(idle,egui::Button::new("Создать капсулу")).clicked() { self.capsule_action(inst,1,String::new()); }
            if ui.add_enabled(!self.is_busy(),egui::Button::new("Диагностика журналов")).clicked() { self.capsule_action(inst,4,String::new()); }
            if ui.add_enabled(!self.is_busy(),egui::Button::new("Обновить список")).clicked() { self.capsule_action(inst,0,String::new()); }
        });
        if self.capsules.job.is_some() { ui.spinner(); ui.label("Работа с файлами… Не закрывайте лаунчер."); }
        if !idle { ui.label("Создание и восстановление доступны после остановки игры и завершения фоновых операций."); }
        ui.separator();
        for c in self.capsules.items.clone() {
            ui.push_id(&c.id,|ui| {
                ui.horizontal_wrapped(|ui| {
                    let bytes:u64=c.files.iter().map(|f|f.size).sum();
                    ui.label(format!("{} · {} · {} файлов · {:.1} МиБ{}", if c.label.is_empty() {"Без названия"} else {&c.label},ago(c.created),c.files.len(),bytes as f64 / 1048576.0,if c.include_saves {" · с мирами"} else {""}));
                    if ui.add_enabled(idle,egui::Button::new("Сравнить")).clicked() { self.capsule_action(inst,2,c.id.clone()); }
                    if ui.add_enabled(idle,egui::Button::new("Восстановить…")).clicked() { self.capsules.confirm=Some(c.id.clone()); }
                });
                if self.capsules.confirm.as_deref()==Some(&c.id) {
                    ui.colored_label(theme::green(),"Будут заменены файлы и настройки в области капсулы, включая удаление добавленных позже модов. Перед этим создаётся страховочная капсула. Миры меняются только если они включены в выбранную капсулу.");
                    ui.horizontal(|ui| {
                        if ui.add_enabled(idle,egui::Button::new("Подтверждаю восстановление")).clicked() { self.capsules.confirm=None; self.capsule_action(inst,3,c.id.clone()); }
                        if ui.button("Отмена").clicked() { self.capsules.confirm=None; }
                    });
                }
            });
        }
        ui.separator();
        for line in &self.capsules.lines { ui.label(line); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capsule_page_renders_without_network_or_window() {
        let root=std::env::temp_dir().join(format!("lime-capsule-ui-{}",std::process::id()));
        let ctx=egui::Context::default();
        let mut app=LimeApp::with_root(&ctx,root.clone(),None);
        let inst=Instance{name:"capsule ui".into(),..Default::default()};
        app.capsules.for_name=inst.name.clone(); app.capsules.loaded=true;
        let mut out=ctx.run_ui(egui::RawInput::default(),|ui| {
            egui::CentralPanel::default().show(ui,|ui|app.draw_capsules(ui,&inst));
        });
        out.textures_delta.clear();
        assert!(!out.shapes.is_empty()); assert!(!app.capsules.busy());
        if root.exists() {std::fs::remove_dir_all(root).unwrap();}
    }
}
