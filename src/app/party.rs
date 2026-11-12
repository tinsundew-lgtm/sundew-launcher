use super::*;
use crate::core::party;
#[derive(Default)]pub(super) struct PartyUi {code:String,job:Option<Job<PartyOutcome>>,report:Vec<String>,recipe:Option<party::Recipe>,confirm:bool}
struct PartyOutcome{code:Option<String>,recipe:party::Recipe,lines:Vec<String>,wants:Option<Vec<mods::Want>>}
impl PartyUi{pub(super) fn busy(&self)->bool{self.job.is_some()}}
impl LimeApp {
    pub(super) fn poll_party(&mut self){
        let Some(result)=self.party.job.as_ref().and_then(drain)else{return;};self.party.job=None;
        match result{Err(e)=>self.party.report=vec![e],Ok(out)=>{
            if let Some(code)=out.code{self.party.code=code;}
            self.party.recipe=Some(out.recipe);self.party.report=out.lines;
            if let Some(wants)=out.wants{if wants.is_empty(){self.party.report.push("Нет недостающих включённых модов. Сверьте лишние файлы и активность отдельно.".into());}else{self.install_wants(wants,false);}}
        }}
    }
    fn party_action(&mut self,inst:&Instance,action:u8){
        if self.is_busy() || launch::is_running(&inst.name){self.toast=Some("Остановите игру и дождитесь завершения операций".into());return;}
        let root=self.root.clone();let inst=inst.clone();let code=self.party.code.clone();
        self.party.job=Some(start_detached(&install::make_client(),move |c|{
            if action==0{let recipe=party::capture(&root,&inst)?;return Ok(PartyOutcome{code:Some(recipe.code()?),recipe,lines:vec!["Код создан. Отправьте его друзьям любым мессенджером.".into()],wants:None});}
            let recipe=party::Recipe::parse(&code)?;let lines=party::compare(&root,&inst,&recipe)?;
            let wants=if action==2{
                let wants=party::missing_wants(c,&root,&inst,&recipe)?;
                if !wants.is_empty(){crate::core::capsules::create(&root,&inst,"Перед установкой по коду друзей",false)?;}
                Some(wants)
            }else{None};
            Ok(PartyOutcome{code:None,recipe,lines:if lines.is_empty(){vec!["Состав модов и версии совпадают.".into()]}else{lines},wants})
        }));
    }
    pub(super) fn draw_party(&mut self,ui:&mut egui::Ui,inst:&Instance){
        ui.heading("Собрать компанию");
        ui.label("Передайте друзьям код состава сборки: версии игры/загрузчика, имена, хэши и активность модов. Конфиги и миры не передаются. Это обмен рецептом, не онлайн-комната и не игровой сервер.");
        ui.add(egui::TextEdit::multiline(&mut self.party.code).desired_rows(4).char_limit(1_000_000).desired_width(f32::INFINITY).hint_text("Вставьте код друга или создайте свой"));
        let idle=!self.is_busy() && !launch::is_running(&inst.name);
        ui.horizontal_wrapped(|ui|{
            if ui.add_enabled(idle,egui::Button::new("Создать код сборки")).clicked(){self.party_action(inst,0);}
            if ui.button("Копировать код").clicked(){ui.ctx().copy_text(self.party.code.clone());}
            if ui.add_enabled(idle,egui::Button::new("Сравнить с кодом")).clicked(){self.party_action(inst,1);}
            if ui.add_enabled(idle,egui::Button::new("Установить недостающее…")).clicked(){self.party.confirm=true;}
        });
        if self.party.confirm {
            ui.label("Будут загружены только точные основные файлы, известные Modrinth и подходящие сборке. Сначала создаётся капсула. Лишние/конфликтующие файлы не удаляются автоматически; зависимости могут добавить файлы. После установки повторите сравнение.");
            ui.horizontal(|ui|{
                if ui.add_enabled(idle,egui::Button::new("Подтверждаю загрузку с Modrinth")).clicked(){self.party.confirm=false;self.party_action(inst,2);}
                if ui.button("Отмена").clicked(){self.party.confirm=false;}
            });
        }
        if self.party.busy(){ui.spinner();}
        for line in &self.party.report{ui.label(line);}
    }
}
