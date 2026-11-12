use super::*;
const MOODS:&[&str]=&["Любое","Расслабиться","Приключение","С друзьями","Строительство"];
#[derive(Default)]
pub(super) struct SessionUi { for_name:String, draft:crate::core::session::SessionNotes, minutes:u32, mood:usize }
impl LimeApp {
    pub(super) fn draw_session_notes(&mut self,ui:&mut egui::Ui,inst:&Instance) {
        if self.session.for_name!=inst.name {self.session.for_name=inst.name.clone();self.session.draft=inst.session.clone();}
        ui.heading("На чём я остановился");
        ui.label("Личный журнал этой сборки. Заполняется вручную: лаунчер не читает сюжет и не угадывает цели.");
        ui.label("Что было в прошлый раз");ui.add(egui::TextEdit::multiline(&mut self.session.draft.recap).desired_rows(4).char_limit(8000).desired_width(f32::INFINITY));
        ui.label("Следующая цель");ui.add(egui::TextEdit::multiline(&mut self.session.draft.next_goal).desired_rows(2).char_limit(2000).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            ui.label("Минимум на игру, мин:");ui.add(egui::DragValue::new(&mut self.session.draft.min_minutes).range(1..=1440));
            ui.label("Запуск/подготовка, мин:");ui.add(egui::DragValue::new(&mut self.session.draft.startup_minutes).range(0..=120));
        });
        egui::ComboBox::from_id_salt("session_mood").selected_text(&self.session.draft.mood).show_ui(ui,|ui| {for mood in MOODS {ui.selectable_value(&mut self.session.draft.mood,(*mood).into(),*mood);}});
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
    pub(super) fn draw_time_picker(&mut self,ui:&mut egui::Ui) {
        egui::CollapsingHeader::new("У меня есть немного времени — подобрать сборку").show(ui,|ui| {
            if self.session.minutes==0 {self.session.minutes=40;}
            ui.horizontal(|ui| {
                ui.label("Есть минут:");ui.add(egui::DragValue::new(&mut self.session.minutes).range(5..=1440));
                egui::ComboBox::from_id_salt("time_mood").selected_text(MOODS[self.session.mood]).show_ui(ui,|ui|{for (idx,mood) in MOODS.iter().enumerate(){ui.selectable_value(&mut self.session.mood,idx,*mood);}});
            });
            ui.label("Оценка по вашим параметрам сессии; время загрузки файлов и реальная скорость запуска не измеряются.");
            let mut matching:Vec<_>=self.state.instances.iter().enumerate().filter(|(_,i)|crate::core::session::fits(&i.session,self.session.minutes,MOODS[self.session.mood])).map(|(idx,i)|(idx,i.clone())).collect();
            matching.sort_by_key(|(_,i)|std::cmp::Reverse(i.last_played));
            if matching.is_empty(){ui.label("Нет сборок под этот бюджет. Измените время или заполните параметры на вкладке «Сессия».");}
            for (idx,inst) in matching.into_iter().take(5) {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{} · от {} мин + {} мин на запуск",inst.name,inst.session.min_minutes,inst.session.startup_minutes));
                    if ui.button(format!("Открыть {}",inst.name)).clicked(){self.active_instance=idx;self.page=Page::Instance;self.inst_tab=4;}
                });
                if !inst.session.next_goal.is_empty(){ui.label(format!("Дальше: {}",inst.session.next_goal));}
            }
        });
    }
}
