use super::*;
use crate::core::self_update;
#[derive(Default)]pub(super) struct UpdaterUi{checked:bool,check:Option<Job<Option<self_update::Release>>>,download:Option<Job<PathBuf>>,release:Option<self_update::Release>,progress:Option<Arc<install::InstallProgress>>,helper:Option<PathBuf>,message:String}
impl UpdaterUi{pub(super) fn busy(&self)->bool{self.check.is_some()||self.download.is_some()}}
impl LimeApp{
    pub(super) fn poll_updater(&mut self){
        if !self.updater.checked {
            self.updater.checked=true;
            #[cfg(not(test))]
            if cfg!(windows){self.updater.check=Some(start_detached(&install::make_client(),move |_|self_update::check()));}
        }
        if let Some(result)=self.updater.check.as_ref().and_then(drain){self.updater.check=None;match result{Ok(r)=>{self.updater.release=r;self.updater.message=if self.updater.release.is_some(){"Доступна новая версия".into()}else{"Установлена актуальная версия".into()};},Err(e)=>self.updater.message=format!("Проверка обновления: {e}")}}
        if let Some(result)=self.updater.download.as_ref().and_then(drain){self.updater.download=None;self.updater.progress=None;match result{Ok(helper)=>{self.updater.helper=Some(helper);self.updater.message="Обновление проверено. Можно закрыть лаунчер и установить.".into();},Err(e)=>self.updater.message=e}}
    }
    fn check_launcher_update(&mut self){self.updater.checked=true;self.updater.check=Some(start_detached(&install::make_client(),move |_|self_update::check()));self.updater.message="Проверка GitHub Releases…".into();}
    pub(super) fn draw_update_panel(&mut self,ui:&mut egui::Ui){
        ui.heading(format!("LimeCraft {}",env!("CARGO_PKG_VERSION")));
        ui.label("Автоматическая проверка стабильных GitHub-релизов при запуске. Установка — только после подтверждения, без изменения сборок и конфигов.");
        if ui.add_enabled(!self.updater.busy(),egui::Button::new("Проверить обновление лаунчера")).clicked(){self.check_launcher_update();}
        ui.label(&self.updater.message);
        self.draw_update_controls(ui);
    }
    fn draw_update_controls(&mut self,ui:&mut egui::Ui){
        if let Some(release)=self.updater.release.clone(){
            ui.label(format!("Новый релиз: {}",release.version));
            if self.updater.helper.is_none() && ui.add_enabled(!self.updater.busy(),egui::Button::new("Скачать обновление лаунчера")).clicked(){
                let data=self.root.clone();let progress=Arc::new(install::InstallProgress::new());let p=progress.clone();self.updater.progress=Some(progress);
                self.updater.download=Some(start_detached(&install::make_client(),move |_|self_update::prepare(&release,&data,&p)));
            }
        }
        if let Some(p)=&self.updater.progress{self.install_progress_bar(ui,p,Some(&p.cancelled));}
        if let Some(helper)=self.updater.helper.clone(){
            let ready=!self.is_busy() && launch::running_count()==0;
            if ui.add_enabled(ready,egui::Button::new("Установить обновление и перезапустить")).clicked(){
                match self_update::launch_helper(&helper){Ok(())=>self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),Err(e)=>self.updater.message=e}
            }
            if !ready{ui.label("Сначала остановите игру и дождитесь завершения операций.");}
        }
    }
    pub(super) fn draw_update_banner(&mut self,ui:&mut egui::Ui){
        if self.updater.release.is_none(){return;}
        egui::Panel::bottom("launcher_update_notice").resizable(false).show(ui,|ui|{self.draw_update_controls(ui);});
    }
}
