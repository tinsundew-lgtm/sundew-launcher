use super::*;
#[derive(Default)]
pub(super) struct SessionUi { for_name:String, draft:crate::core::session::SessionNotes }
impl LimeApp {
    pub(super) fn draw_session_notes(&mut self,ui:&mut egui::Ui,inst:&Instance) {
        if self.session.for_name!=inst.name {self.session.for_name=inst.name.clone();self.session.draft=inst.session.clone();}
        ui.heading("На чём я остановился");
        ui.label("Личный журнал этой сборки. Заполняется вручную: лаунчер не читает сюжет и не угадывает цели.");
        ui.label("Что было в прошлый раз");ui.add(egui::TextEdit::multiline(&mut self.session.draft.recap).desired_rows(4).char_limit(8000).desired_width(f32::INFINITY));
        ui.label("Следующая цель");ui.add(egui::TextEdit::multiline(&mut self.session.draft.next_goal).desired_rows(2).char_limit(2000).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("Прикрепить скриншот…").clicked() {
                if let Some(path)=rfd::FileDialog::new().add_filter("Изображения",&["png","jpg","jpeg","webp"]).pick_file(){
                    match images::load_rgba(&path) {Ok(_)=>self.session.draft.screenshot=Some(path.display().to_string()),Err(e)=>self.toast=Some(e)}
                }
            }
            if ui.button("Убрать скриншот").clicked(){self.session.draft.screenshot=None;}
        });
        if let Some(path)=&self.session.draft.screenshot {
            ui.label(path);
            // Use existing cached image pipeline; no image bytes are placed in global state.
            let key=format!("session:{}",inst.name);
            if !self.pics.contains_key(&key) {
                let tex=images::load_rgba(&PathBuf::from(path)).ok().map(|(rgba,w,h)|ui.ctx().load_texture(&key,egui::ColorImage::from_rgba_unmultiplied([w as usize,h as usize],&rgba),egui::TextureOptions::LINEAR));
                self.pics.insert(key.clone(),tex);
            }
            if let Some(Some(tex))=self.pics.get(&key){ui.add(egui::Image::new(tex).max_size(egui::vec2(600.0,300.0)));}
        }
        if ui.button("Сохранить заметки и параметры сессии").clicked(){
            if let Some(i)=self.state.instances.iter_mut().find(|i|i.name==inst.name){i.session=self.session.draft.clone();}
            self.pics.remove(&format!("session:{}",inst.name));self.save_state();self.toast=Some("Заметки сохранены".into());
        }
    }
}
