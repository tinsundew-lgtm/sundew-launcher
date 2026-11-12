#![cfg_attr(not(debug_assertions),windows_subsystem="windows")]
mod install;
use std::{path::PathBuf,sync::{Arc,atomic::{AtomicUsize,Ordering},mpsc}};
use eframe::egui;
static PAYLOAD:&[u8]=include_bytes!(concat!(env!("OUT_DIR"),"/launcher.exe"));
struct Setup {
    path:String, progress:Arc<AtomicUsize>, job:Option<mpsc::Receiver<Result<PathBuf,String>>>,
    message:String, installed:Option<PathBuf>, launch:bool,
}
impl Default for Setup {
    fn default()->Self {
        let path=dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Programs").join("LimeCraft");
        Self{path:path.display().to_string(),progress:Arc::new(AtomicUsize::new(0)),job:None,message:String::new(),installed:None,launch:true}
    }
}
impl eframe::App for Setup {
    fn ui(&mut self,ui:&mut egui::Ui,_frame:&mut eframe::Frame) {
        if let Some(rx)=&self.job {
            match rx.try_recv() {
                Ok(result)=>{self.job=None;match result {Ok(p)=>{self.installed=Some(p);self.message="Готово! LimeCraft установлен.".into();},Err(e)=>self.message=e}},
                Err(mpsc::TryRecvError::Disconnected)=>{self.job=None;self.message="Поток установки неожиданно завершился".into();},
                Err(mpsc::TryRecvError::Empty)=>(),
            }
        }
        let busy=self.job.is_some();
        if busy {ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);}
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(32)).show(ui,|ui| {
            ui.heading(egui::RichText::new("LimeCraft").size(34.0).color(egui::Color32::from_rgb(148,220,105)));
            ui.label("Самописный установщик · Rust + egui");ui.add_space(20.0);
            ui.label("Установка для текущего пользователя, без прав администратора. Minecraft, Java и моды загружаются отдельно в лаунчере.");
            ui.label("Игровые данные остаются в стандартной папке LimeCraft и не изменяются установщиком.");ui.add_space(16.0);
            ui.add_enabled_ui(!busy && self.installed.is_none(),|ui| {
                ui.label("Папка приложения");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(390.0));
                    if ui.button("Обзор…").clicked() {if let Some(p)=rfd::FileDialog::new().pick_folder(){self.path=p.display().to_string();}}
                });
                ui.label(format!("Встроенный файл: {:.1} МиБ",PAYLOAD.len() as f64/1048576.0));
                if PAYLOAD.is_empty(){ui.colored_label(egui::Color32::YELLOW,"Режим разработки: payload не встроен, установка недоступна.");}
                ui.add_space(12.0);
                if ui.add_enabled(!PAYLOAD.is_empty(),egui::Button::new("Установить LimeCraft")).clicked() {
                    let dir=PathBuf::from(self.path.trim()); let progress=self.progress.clone(); let (tx,rx)=mpsc::channel(); let ctx=ui.ctx().clone();
                    self.message="Проверка и установка… Не закрывайте окно.".into();self.job=Some(rx);
                    std::thread::spawn(move || {
                        let result=std::panic::catch_unwind(||install::install(&dir,PAYLOAD,env!("LIMECRAFT_PAYLOAD_SHA256"),&progress)).unwrap_or_else(|_|Err("Внутренняя ошибка установки".into()));
                        let _=tx.send(result);ctx.request_repaint();
                    });
                }
            });
            if busy {ui.add(egui::ProgressBar::new(self.progress.load(Ordering::Relaxed) as f32/100.0).show_percentage());}
            ui.add_space(12.0);ui.label(&self.message);
            if let Some(exe)=self.installed.clone() {
                ui.checkbox(&mut self.launch,"Запустить лаунчер");
                if ui.button("Завершить").clicked() {
                    if self.launch {
                        match std::process::Command::new(&exe).current_dir(exe.parent().unwrap()).spawn() {Ok(_)=>(),Err(e)=>{self.message=format!("Установлено, но не удалось запустить: {e}");return;}}
                    }
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        });
    }
}
fn main()->eframe::Result<()> {
    eframe::run_native("Установка LimeCraft",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([640.0,480.0]).with_min_inner_size([600.0,460.0]),..Default::default()},Box::new(|cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut fonts=egui::FontDefinitions::default();
        fonts.font_data.insert("manrope".into(),Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/Manrope.ttf"))));
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0,"manrope".into());cc.egui_ctx.set_fonts(fonts);
        Ok(Box::new(Setup::default()))
    }))
}
