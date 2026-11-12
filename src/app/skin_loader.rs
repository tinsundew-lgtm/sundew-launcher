use super::*;

#[derive(Default)]
pub(super) struct SkinLoaderUi { job: Option<Job<Vec<String>>>, progress: Option<Arc<install::InstallProgress>>, report: Vec<String> }
impl SkinLoaderUi { pub(super) fn busy(&self)->bool {self.job.is_some()} pub(super) fn progress(&self)->Option<Arc<install::InstallProgress>> {self.progress.clone()} }
impl LimeApp {
    pub(super) fn install_skin_loaders(&mut self, all:bool) {
        if self.is_busy() {self.toast=Some("Дождитесь завершения операций".into());return;}
        let targets:Vec<_>=if all {self.state.instances.clone()} else {self.state.instances.get(self.active_index()).cloned().into_iter().collect()};
        if targets.is_empty(){return;}
        if targets.iter().any(|i|launch::is_running(&i.name)){self.toast=Some("Остановите игры перед установкой CustomSkinLoader".into());return;}
        let root=self.root.clone(); let prog=Arc::new(install::InstallProgress::new());let p=prog.clone();
        self.skin_loader.progress=Some(prog);self.skin_loader.report.clear();
        self.skin_loader.job=Some(start_detached(&install::make_client(),move |client| {
            let mut report=Vec::new();
            for inst in targets {
                if p.cancelled.load(Ordering::Relaxed){report.push("Установка оставшихся сборок отменена".into());break;}
                let result=install_one_skin_loader(client,&root,&inst,&p);
                report.push(format!("{}: {}",inst.name,match result {Ok(s)=>s,Err(e)=>format!("не установлен — {e}")}));
            }
            Ok(report)
        }));
    }
    pub(super) fn poll_skin_loader(&mut self) {
        let Some(result)=self.skin_loader.job.as_ref().and_then(drain) else{return;};
        self.skin_loader.job=None;self.skin_loader.progress=None;self.installed_for=None;
        self.skin_loader.report=match result {Ok(r)=>r,Err(e)=>vec![e]};
        self.toast=Some("Проверка CustomSkinLoader завершена; отчёт на вкладке «Скины»".into());
    }
    pub(super) fn draw_skin_loader_install(&mut self,ui:&mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let idle=!self.is_busy();
            if ui.add_enabled(idle && !self.state.instances.is_empty(),egui::Button::new("Установить CSL в выбранную сборку")).clicked(){self.install_skin_loaders(false);}
            if ui.add_enabled(idle && !self.state.instances.is_empty(),egui::Button::new("Установить CSL во все совместимые сборки")).clicked(){self.install_skin_loaders(true);}
        });
        ui.label("Vanilla пропускается: нужен загрузчик модов. Для каждой сборки подбирается своя совместимая версия с Modrinth; загрузчик игры не меняется.");
        for line in &self.skin_loader.report {ui.label(line);}
    }
}
fn install_one_skin_loader(client:&reqwest::blocking::Client,root:&std::path::Path,inst:&Instance,prog:&Arc<install::InstallProgress>)->Result<String,String> {
    if mods::mod_loaders(inst).is_empty(){return Ok("пропущено: Vanilla, нет загрузчика модов".into());}
    if skins::has_skin_loader(&instance::instance_mines(root,&inst.name)){return Ok("уже установлен (версия не менялась)".into());}
    let version=mods::newest_fitting(client,inst,mods::ContentKind::Mod,"idMHQ4n2")?.ok_or("Нет опубликованной совместимой версии CustomSkinLoader")?;
    // Defense in depth: never trust server-side filtering alone.
    if !version.game_versions.contains(&inst.game_version) || !version.loaders.iter().any(|l|mods::mod_loaders(inst).contains(&l.as_str())) {return Err("Ответ каталога не соответствует версии игры или загрузчику".into());}
    let report=mods::install(client,root,inst,mods::ContentKind::Mod,&version,Some(prog))?;
    Ok(format!("установлено: {}{}",report.installed.join(", "),if report.warnings.is_empty(){String::new()}else{format!("; {}",report.warnings.join("; "))}))
}
