#![cfg_attr(not(debug_assertions),windows_subsystem="windows")]
mod install;
#[path="../../../src/data_location.rs"]
mod data_location;
use std::{path::PathBuf,sync::{Arc,atomic::{AtomicUsize,Ordering},mpsc}};
use eframe::egui;
static PAYLOAD:&[u8]=include_bytes!(concat!(env!("OUT_DIR"),"/launcher.exe"));
struct Setup {
    path:String, progress:Arc<AtomicUsize>, job:Option<mpsc::Receiver<Result<PathBuf,String>>>,
    message:String, installed:Option<PathBuf>, launch:bool,
    data_path:String, scanned_app:String, existing:Vec<PathBuf>,
}
impl Default for Setup {
    fn default()->Self {
        let path=dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Programs").join("LimeCraft");
        Self{path:path.display().to_string(),progress:Arc::new(AtomicUsize::new(0)),job:None,message:String::new(),installed:None,launch:true,data_path:String::new(),scanned_app:String::new(),existing:Vec::new()}
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
        if self.scanned_app!=self.path && !busy {
            self.scanned_app=self.path.clone();
            let setup_dir=std::env::current_exe().ok().and_then(|p|p.parent().map(|p|p.to_path_buf())).unwrap_or_default();
            self.existing=data_location::candidates(&PathBuf::from(&self.path),&setup_dir).into_iter().filter(|p|data_location::has_data(p)).collect();
            self.data_path=self.existing.first().cloned().unwrap_or_else(||dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("LimeCraft")).display().to_string();
        }
        if busy {ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);}
        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(32)).show(ui,|ui| {
            ui.heading(egui::RichText::new("LimeCraft").size(34.0).color(egui::Color32::from_rgb(148,220,105)));
            ui.label("Самописный установщик · Rust + egui");ui.add_space(20.0);
            ui.label("Установка для текущего пользователя, без прав администратора. Minecraft, Java и моды загружаются отдельно в лаунчере.");
            ui.label("Используйте уже существующие сборки: установщик сохранит путь, не изменяя их файлы.");ui.add_space(16.0);
            ui.add_enabled_ui(!busy && self.installed.is_none(),|ui| {
                ui.label("Папка приложения");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(390.0));
                    if ui.button("Обзор…").clicked() {if let Some(p)=rfd::FileDialog::new().pick_folder(){self.path=p.display().to_string();}}
                });
                ui.label("Папка данных (сборки и конфиги не перезаписываются)");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.data_path).desired_width(390.0));
                    if ui.button("Выбрать данные…").clicked(){if let Some(p)=rfd::FileDialog::new().pick_folder(){self.data_path=p.display().to_string();}}
                });
                if !self.existing.is_empty(){
                    egui::ComboBox::from_id_salt("existing_data").selected_text("Найдены существующие данные — использовать").show_ui(ui,|ui|{
                        for p in &self.existing {let label=p.display().to_string();ui.selectable_value(&mut self.data_path,label.clone(),label);}
                    });
                    ui.label("Обнаруженная папка выбрана автоматически. Если вариантов несколько, проверьте выбор.");
                }
                ui.label(format!("Встроенный файл: {:.1} МиБ",PAYLOAD.len() as f64/1048576.0));
                if PAYLOAD.is_empty(){ui.colored_label(egui::Color32::YELLOW,"Режим разработки: payload не встроен, установка недоступна.");}
                ui.add_space(12.0);
                if ui.add_enabled(!PAYLOAD.is_empty(),egui::Button::new("Установить LimeCraft")).clicked() {
                    let data_dir=PathBuf::from(self.data_path.trim());
                    let dir=PathBuf::from(self.path.trim()); let progress=self.progress.clone(); let (tx,rx)=mpsc::channel(); let ctx=ui.ctx().clone();
                    self.message="Проверка и установка… Не закрывайте окно.".into();self.job=Some(rx);
                    std::thread::spawn(move || {
                        let result=std::panic::catch_unwind(|| {install::validate_data_pointer(&dir,&data_dir)?;let exe=install::install(&dir,PAYLOAD,env!("LIMECRAFT_PAYLOAD_SHA256"),&progress)?;install::save_data_pointer(&dir,&data_dir).map_err(|e|format!("Лаунчер установлен, но выбор данных не сохранён: {e}. Не запускайте до исправления."))?;Ok(exe)}).unwrap_or_else(|_|Err("Внутренняя ошибка установки".into()));
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
    eframe::run_native("Установка LimeCraft",eframe::NativeOptions{viewport:egui::ViewportBuilder::default().with_inner_size([720.0,660.0]).with_min_inner_size([680.0,620.0]),..Default::default()},Box::new(|cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut fonts=egui::FontDefinitions::default();
        fonts.font_data.insert("manrope".into(),Arc::new(egui::FontData::from_static(include_bytes!("../../../assets/fonts/Manrope.ttf"))));
        fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0,"manrope".into());cc.egui_ctx.set_fonts(fonts);
        Ok(Box::new(Setup::default()))
    }))
}
