//! Shareable recipe, not a relay server. No remote URLs or executable paths in invitations.
use std::{collections::BTreeMap,path::Path};
use serde::{Serialize,Deserialize};
use base64::{Engine,engine::general_purpose::URL_SAFE_NO_PAD};
use super::{instance::Instance,mods,modrinth};
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {pub format:u32,pub title:String,pub game_version:String,pub loader:String,pub loader_version:String,pub mods:Vec<RecipeMod>}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeMod {pub filename:String,pub sha1:String,pub enabled:bool}
fn valid_hash(h:&str)->bool{h.len()==40 && h.bytes().all(|b|b.is_ascii_hexdigit())}
impl Recipe {
    pub fn validate(&self)->Result<(),String>{
        if self.format!=1 || self.title.len()>200 || self.game_version.len()>100 || self.loader_version.len()>100 || self.mods.len()>2000 || !["","fabric","quilt","forge","neoforge"].contains(&self.loader.as_str()){return Err("Неподдерживаемый рецепт сборки".into());}
        let mut names=std::collections::HashSet::new();
        for m in &self.mods {if !valid_hash(&m.sha1) || m.filename.is_empty() || m.filename.len()>240 || m.filename.contains(['/', '\\', ':']) || !m.filename.to_lowercase().ends_with(".jar") || !names.insert(m.filename.to_lowercase()){return Err("Некорректная запись мода".into());}}
        Ok(())
    }
    pub fn code(&self)->Result<String,String>{self.validate()?;Ok(format!("limecraft-party-v1:{}",URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).map_err(|e|e.to_string())?)))}
    pub fn parse(code:&str)->Result<Self,String>{
        if code.len()>1_000_000{return Err("Код слишком большой".into());}
        let bytes=URL_SAFE_NO_PAD.decode(code.trim().strip_prefix("limecraft-party-v1:").ok_or("Это не код LimeCraft")?).map_err(|e|e.to_string())?;
        let r:Self=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;r.validate()?;Ok(r)
    }
}
pub fn capture(root:&Path,inst:&Instance)->Result<Recipe,String>{
    let items=mods::scan(root,&inst.name,mods::ContentKind::Mod)?;
    let r=Recipe{format:1,title:inst.name.clone(),game_version:inst.game_version.clone(),loader:inst.loader.clone(),loader_version:inst.loader_version.clone(),mods:items.into_iter().map(|m|RecipeMod{filename:m.file_name.trim_end_matches(".disabled").into(),sha1:m.sha1,enabled:m.enabled}).collect()};r.validate()?;Ok(r)
}
pub fn compare(root:&Path,inst:&Instance,r:&Recipe)->Result<Vec<String>,String>{
    r.validate()?;let local=capture(root,inst)?;let mut out=Vec::new();
    for (label,a,b) in [("Minecraft",&r.game_version,&local.game_version),("Загрузчик",&r.loader,&local.loader),("Версия загрузчика",&r.loader_version,&local.loader_version)]{if a!=b{out.push(format!("{label}: нужно {a}, у вас {b}"));}}
    let before:BTreeMap<_,_>=r.mods.iter().map(|m|(&m.sha1,m)).collect();let after:BTreeMap<_,_>=local.mods.iter().map(|m|(&m.sha1,m)).collect();
    for (hash,m) in &before {match after.get(hash){None=>out.push(format!("Нет точного файла: {}{}",m.filename,if m.enabled{""}else{" (отключён)"})),Some(local)if local.enabled!=m.enabled=>out.push(format!("Отличается активность: {}",m.filename)),_=>()}}
    for (hash,m) in &after{if !before.contains_key(hash){out.push(format!("Лишний или другая версия: {}",m.filename));}}
    Ok(out)
}
/// Download only enabled, missing exact files verified by the catalog. Never accepts URLs from a recipe.
pub fn missing_wants(client:&reqwest::blocking::Client,root:&Path,inst:&Instance,r:&Recipe)->Result<Vec<mods::Want>,String>{
    r.validate()?;
    if inst.game_version!=r.game_version || inst.loader!=r.loader || inst.loader_version!=r.loader_version{return Err("Сначала выберите сборку с такими же Minecraft и загрузчиком. Автозамена загрузчика запрещена.".into());}
    let local=capture(root,inst)?;
    let missing:Vec<_>=r.mods.iter().filter(|m|m.enabled && !local.mods.iter().any(|l|l.sha1.eq_ignore_ascii_case(&m.sha1) && l.enabled)).collect();
    let hashes:Vec<_>=missing.iter().map(|m|m.sha1.to_lowercase()).collect();let found=modrinth::versions_by_hashes(client,&hashes)?;
    let mut wants=Vec::new();
    for m in missing {
        if local.mods.iter().any(|l|l.sha1==m.sha1 && !l.enabled){return Err(format!("{} уже есть, но отключён. Включите вручную и повторите сравнение.",m.filename));}
        if local.mods.iter().any(|l|l.filename.eq_ignore_ascii_case(&m.filename) && l.sha1!=m.sha1){return Err(format!("{} занят другой версией; сначала создайте капсулу и разрешите конфликт вручную.",m.filename));}
        let v=found.get(&m.sha1.to_lowercase()).ok_or_else(||format!("{} не найден на Modrinth. Получите его из доверенного источника вручную.",m.filename))?;
        if !v.game_versions.contains(&inst.game_version) || !v.loaders.iter().any(|l|mods::mod_loaders(inst).contains(&l.as_str())){return Err(format!("Каталог не подтверждает совместимость {}",m.filename));}
        if !v.primary_file().is_some_and(|f|f.hashes.get("sha1").is_some_and(|h|h.eq_ignore_ascii_case(&m.sha1))){return Err(format!("Точный файл {} не является основным файлом версии; автоматическая установка отменена.",m.filename));}
        wants.push(mods::Want{kind:mods::ContentKind::Mod,project_id:v.project_id.clone(),title:m.filename.clone(),version:Some(v.clone())});
    }
    Ok(wants)
}
#[cfg(test)]mod tests {use super::*;#[test]fn code_roundtrip_and_rejects_unsafe_files(){let mut r=Recipe{format:1,title:"friends".into(),game_version:"1.21.1".into(),loader:"fabric".into(),loader_version:"0.16".into(),mods:vec![RecipeMod{filename:"a.jar".into(),sha1:"a".repeat(40),enabled:true}]};assert_eq!(Recipe::parse(&r.code().unwrap()).unwrap().mods.len(),1);r.mods[0].filename="../evil.jar".into();assert!(r.code().is_err());assert!(Recipe::parse("https://evil.test").is_err());}}
