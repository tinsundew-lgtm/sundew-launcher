use super::*;
#[derive(Default)]
pub(super) struct ScreenshotsUi {
    name:String, files:Vec<PathBuf>, list:Option<Job<Vec<PathBuf>>>,
    image:Option<Job<(PathBuf,Vec<u8>,u32,u32)>>,
    selected:Option<PathBuf>, texture:Option<egui::TextureHandle>, error:Option<String>,
}
impl ScreenshotsUi {pub(super) fn busy(&self)->bool {self.list.is_some() || self.image.is_some()}}
fn scan(dir:&std::path::Path)->Result<Vec<PathBuf>,String>{
    if !dir.exists(){return Ok(Vec::new());}
    let mut files=Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e|e.to_string())? {
        let entry=entry.map_err(|e|e.to_string())?;let p=entry.path();
        if entry.file_type().map_err(|e|e.to_string())?.is_file() && p.extension().and_then(|s|s.to_str()).is_some_and(|s|["png","jpg","jpeg","webp"].contains(&s.to_lowercase().as_str())){files.push(p);}
    }
    files.sort_by(|a,b|b.file_name().cmp(&a.file_name()));Ok(files)
}
impl LimeApp {
    fn refresh_screenshots(&mut self,inst:&Instance){
        let dir=instance::instance_mines(&self.root,&inst.name).join("screenshots");
        self.screenshots.list=Some(start_detached(&install::make_client(),move |_|scan(&dir)));
        self.screenshots.error=None;
    }
    pub(super) fn poll_screenshots(&mut self){
        if let Some(result)=self.screenshots.list.as_ref().and_then(drain){
            self.screenshots.list=None;
            match result {Ok(files)=>self.screenshots.files=files,Err(e)=>self.screenshots.error=Some(e)}
        }
        if let Some(result)=self.screenshots.image.as_ref().and_then(drain){
            self.screenshots.image=None;
            match result {
                Ok((path,bytes,w,h)) if self.screenshots.selected.as_ref()==Some(&path)=>{
                    self.screenshots.texture=Some(self.ctx.load_texture("instance-screenshot",egui::ColorImage::from_rgba_unmultiplied([w as usize,h as usize],&bytes),egui::TextureOptions::LINEAR));
                },
                Ok(_)=>(),Err(e)=>self.screenshots.error=Some(e),
            }
        }
    }
    pub(super) fn draw_screenshots(&mut self,ui:&mut egui::Ui,inst:&Instance){
        if self.screenshots.name!=inst.name {
            self.screenshots=ScreenshotsUi{name:inst.name.clone(),..Default::default()};self.refresh_screenshots(inst);
        }
        ui.heading("Скриншоты сборки");ui.label("Снимки Minecraft (F2) из папки screenshots этой сборки. Выберите снимок для просмотра.");
        let dir=instance::instance_mines(&self.root,&inst.name).join("screenshots");
        ui.horizontal(|ui|{
            if ui.add_enabled(!self.screenshots.busy(),egui::Button::new("Обновить скриншоты")).clicked(){self.refresh_screenshots(inst);}
            if ui.button("Открыть папку скриншотов").clicked(){match std::fs::create_dir_all(&dir){Ok(())=>open_in_explorer(&dir),Err(e)=>self.screenshots.error=Some(e.to_string())}}
        });
        if self.screenshots.busy(){ui.spinner();}
        if let Some(e)=&self.screenshots.error {ui.colored_label(theme::danger(),e);}
        if self.screenshots.files.is_empty() && self.screenshots.list.is_none(){ui.label("Скриншотов пока нет. Нажмите F2 во время игры, затем обновите список.");}
        egui::ScrollArea::horizontal().id_salt("screenshot_names").max_height(110.0).show(ui,|ui|{
            ui.horizontal(|ui|{for path in self.screenshots.files.clone(){
                let label=path.file_name().unwrap_or_default().to_string_lossy();
                if ui.selectable_label(self.screenshots.selected.as_ref()==Some(&path),label).clicked(){
                    self.screenshots.selected=Some(path.clone());self.screenshots.texture=None;self.screenshots.error=None;
                    self.screenshots.image=Some(start_detached(&install::make_client(),move |_|{
                        // Bound decoded preview to avoid uploading full 8K screenshots to the GPU.
                        let img=image::ImageReader::open(&path).map_err(|e|e.to_string())?.decode().map_err(|e|e.to_string())?.thumbnail(1920,1080).to_rgba8();
                        let (w,h)=img.dimensions();Ok((path,img.into_raw(),w,h))
                    }));
                }
            }});
        });
        if let Some(tex)=&self.screenshots.texture {ui.add(egui::Image::new(tex).max_size(egui::vec2(ui.available_width(),600.0)));}
        if let Some(path)=&self.screenshots.selected {if ui.button("Открыть оригинал").clicked(){open_in_explorer(path);}}
    }
}
#[cfg(test)]mod tests {use super::*;#[test]fn scan_only_images_sorted_and_no_missing_error(){let dir=std::env::temp_dir().join(format!("lc-shots-{}",std::process::id()));std::fs::create_dir_all(&dir).unwrap();std::fs::write(dir.join("a.png"),b"x").unwrap();std::fs::write(dir.join("b.PNG"),b"x").unwrap();std::fs::write(dir.join("no.txt"),b"x").unwrap();let files=scan(&dir).unwrap();assert_eq!(files.len(),2);assert_eq!(files[0].file_name().unwrap(),"b.PNG");std::fs::remove_dir_all(&dir).unwrap();assert!(scan(&dir).unwrap().is_empty());}}
