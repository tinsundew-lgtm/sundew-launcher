//! GitHub release updates. HTTPS + published SHA256 detects corruption, not publisher compromise.
use std::{fs,io::{Read,Write},path::{Path,PathBuf},sync::{Arc,atomic::Ordering}};
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use super::install::InstallProgress;
const API:&str="https://api.github.com/repos/tinsundew-lgtm/sundew-launcher/releases/latest";
const ASSET:&str="LimeCraft-launcher-windows-x64.exe";
#[derive(Clone,Debug)]pub struct Release {pub version:String,pub url:String,pub checksum_url:String,pub size:u64}
#[derive(Deserialize)]struct ApiRelease{tag_name:String,draft:bool,prerelease:bool,assets:Vec<Asset>}
#[derive(Deserialize)]struct Asset{name:String,browser_download_url:String,size:u64}
fn version(s:&str)->Option<(u32,u32,u32)>{let mut p=s.trim_start_matches('v').split('.');let v=(p.next()?.parse().ok()?,p.next()?.parse().ok()?,p.next()?.parse().ok()?);if p.next().is_some(){None}else{Some(v)}}
fn trusted(url:&str)->bool {url.starts_with("https://github.com/tinsundew-lgtm/sundew-launcher/releases/download/") && !url.contains(['?','#'])}
fn client()->Result<reqwest::blocking::Client,String>{reqwest::blocking::Client::builder().user_agent(concat!("LimeCraft/",env!("CARGO_PKG_VERSION"))).timeout(std::time::Duration::from_secs(90)).build().map_err(|e|e.to_string())}
pub fn check()->Result<Option<Release>,String>{
    let r:ApiRelease=client()?.get(API).send().map_err(|e|e.to_string())?.error_for_status().map_err(|e|e.to_string())?.json().map_err(|e|e.to_string())?;
    if r.draft || r.prerelease || version(&r.tag_name).ok_or("Необычный номер релиза")?<=version(env!("CARGO_PKG_VERSION")).unwrap(){return Ok(None);}
    let exe=r.assets.iter().find(|a|a.name==ASSET).ok_or("В новом релизе нет файла для автообновления Windows")?;
    let sum=r.assets.iter().find(|a|a.name==format!("{ASSET}.sha256")).ok_or("В релизе нет контрольной суммы")?;
    if !trusted(&exe.browser_download_url) || !trusted(&sum.browser_download_url) || exe.size==0 || exe.size>200*1024*1024{return Err("Неверные файлы обновления".into());}
    Ok(Some(Release{version:r.tag_name,url:exe.browser_download_url.clone(),checksum_url:sum.browser_download_url.clone(),size:exe.size}))
}
fn sha(path:&Path)->Result<String,String>{let mut f=fs::File::open(path).map_err(|e|e.to_string())?;let mut h=Sha256::new();let mut b=[0;65536];loop{let n=f.read(&mut b).map_err(|e|e.to_string())?;if n==0{break;}h.update(&b[..n]);}Ok(hex::encode(h.finalize()))}
fn no_links(path:&Path)->Result<(),String>{for p in path.ancestors(){if let Ok(m)=fs::symlink_metadata(p){
    #[cfg(windows)]{use std::os::windows::fs::MetadataExt;if m.file_attributes()&0x400!=0{return Err("Обновление через junction запрещено".into());}}
    if m.file_type().is_symlink(){return Err("Обновление через ссылку запрещено".into());}
}}Ok(())}
#[derive(Serialize,Deserialize)]struct Request {target:PathBuf,old_hash:String,new_hash:String,data:PathBuf}
pub fn prepare(r:&Release,data:&Path,prog:&Arc<InstallProgress>)->Result<PathBuf,String>{
    if !cfg!(windows){return Err("Автоустановка пока поддерживается только на Windows".into());}
    if !trusted(&r.url)||!trusted(&r.checksum_url){return Err("Недоверенный адрес обновления".into());}
    let exe=std::env::current_exe().map_err(|e|e.to_string())?;no_links(&exe)?;
    let stage=exe.parent().ok_or("Нет папки приложения")?.join(".limecraft-update");
    fs::create_dir(&stage).map_err(|e|format!("Не удалось создать staging {}: {e}. Остатки предыдущего обновления проверьте вручную.",stage.display()))?;
    let result=(||{
        let client=client()?;
        let mut text=String::new();client.get(&r.checksum_url).send().map_err(|e|e.to_string())?.error_for_status().map_err(|e|e.to_string())?.take(4096).read_to_string(&mut text).map_err(|e|e.to_string())?;
        let hash=text.split_whitespace().next().ok_or("Пустая контрольная сумма")?.to_lowercase();
        if hash.len()!=64 || !hash.bytes().all(|b|b.is_ascii_hexdigit()){return Err("Неверная контрольная сумма".into());}
        prog.total.store(r.size,Ordering::Relaxed);prog.current.store(0,Ordering::Relaxed);*prog.current_file.lock().map_err(|e|e.to_string())?=format!("Обновление {}",r.version);
        let mut response=client.get(&r.url).send().map_err(|e|e.to_string())?.error_for_status().map_err(|e|e.to_string())?;
        let payload=stage.join("payload.exe");let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&payload).map_err(|e|e.to_string())?;let mut b=[0;65536];let mut count=0u64;
        loop{if prog.cancelled.load(Ordering::Relaxed){return Err("Обновление отменено".into());}let n=response.read(&mut b).map_err(|e|e.to_string())?;if n==0{break;}count+=n as u64;if count>r.size{return Err("Размер обновления превышен".into());}file.write_all(&b[..n]).map_err(|e|e.to_string())?;prog.current.store(count,Ordering::Relaxed);}
        file.sync_all().map_err(|e|e.to_string())?;drop(file);
        if count!=r.size || sha(&payload)?!=hash{return Err("Обновление повреждено: SHA256/размер не совпадает".into());}
        let mut magic=[0u8;2];fs::File::open(&payload).map_err(|e|e.to_string())?.read_exact(&mut magic).map_err(|e|e.to_string())?;if &magic!=b"MZ"{return Err("Payload не является Windows exe".into());}
        fs::copy(&exe,stage.join("helper.exe")).map_err(|e|e.to_string())?;
        let req=Request{target:exe.clone(),old_hash:sha(&exe)?,new_hash:hash,data:data.to_path_buf()};
        fs::write(stage.join("request.json"),serde_json::to_vec(&req).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;Ok(stage.join("helper.exe"))
    })();
    if result.is_err(){let _=fs::remove_dir_all(&stage);}result
}
pub fn launch_helper(helper:&Path)->Result<(),String>{std::process::Command::new(helper).arg("--limecraft-apply-update").spawn().map(|_|()).map_err(|e|e.to_string())}
/// Executed by a copy of the old launcher after the UI exits. Only touches fixed staging files and the verified target.
pub fn apply_helper()->Result<(),String>{
    let helper=std::env::current_exe().map_err(|e|e.to_string())?;
    let stage=helper.parent().ok_or("Нет staging")?;let app=stage.parent().ok_or("Нет папки приложения")?;
    if helper.file_name().and_then(|s|s.to_str())!=Some("helper.exe") || stage.file_name().and_then(|s|s.to_str())!=Some(".limecraft-update"){return Err("Неверное расположение updater".into());}
    no_links(stage)?;let req:Request=serde_json::from_slice(&fs::read(stage.join("request.json")).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    if req.target.parent()!=Some(app) || req.target==helper || !req.data.is_absolute(){return Err("Неверная цель updater".into());}
    if !req.target.extension().is_some_and(|e|e.eq_ignore_ascii_case("exe")){return Err("Цель не exe".into());}
    no_links(&req.target)?;no_links(&stage.join("payload.exe"))?;no_links(&stage.join("request.json"))?;
    let receipt=app.join("limecraft-install.json");no_links(&receipt)?;
    let receipt_old=if receipt.exists(){
        let old=fs::read(&receipt).map_err(|e|e.to_string())?;
        let value:serde_json::Value=serde_json::from_slice(&old).map_err(|e|e.to_string())?;
        if value.get("product").and_then(|v|v.as_str())!=Some("LimeCraft") || value.get("sha256").and_then(|v|v.as_str())!=Some(&req.old_hash){return Err("Квитанция установки не соответствует текущему exe".into());}
        Some(old)
    }else{None};
    if sha(&req.target)?!=req.old_hash || sha(&stage.join("payload.exe"))?!=req.new_hash{return Err("Файлы изменились после проверки".into());}
    let backup=stage.join("previous.exe");if backup.exists(){return Err("Резервный exe уже существует".into());}
    let mut moved=false;
    for _ in 0..120 {if fs::rename(&req.target,&backup).is_ok(){moved=true;break;}std::thread::sleep(std::time::Duration::from_millis(500));}
    if !moved{return Err("Лаунчер не закрыт или нет доступа к exe".into());}
    if let Err(e)=fs::rename(stage.join("payload.exe"),&req.target){let rollback=fs::rename(&backup,&req.target);return Err(format!("Не удалось заменить exe: {e}; возврат: {rollback:?}"));}
    // Keep the installer receipt in sync; don't change its data pointer.
    if let Some(old)=receipt_old {
        let update_receipt=(||{
            fs::write(stage.join("previous-receipt.json"),&old).map_err(|e|e.to_string())?;
            let mut value:serde_json::Value=serde_json::from_slice(&old).map_err(|e|e.to_string())?;
            value["sha256"]=req.new_hash.into();let tmp=app.join(".limecraft-receipt-update.tmp");
            let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e|e.to_string())?;
            file.write_all(&serde_json::to_vec_pretty(&value).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;file.sync_all().map_err(|e|e.to_string())?;drop(file);
            fs::rename(tmp,&receipt).map_err(|e|e.to_string())?;Ok::<_,String>(())
        })();
        if let Err(e)=update_receipt {
            let remove=fs::rename(&req.target,stage.join("payload.exe"));
            let restore=if remove.is_ok(){fs::rename(&backup,&req.target)}else{Err(std::io::Error::other("Новый exe не перемещён"))};
            return Err(format!("Квитанция не обновлена: {e}; возврат exe: {restore:?}"));
        }
    }
    fs::write(stage.join("complete"),b"ok").map_err(|e|e.to_string())?;
    std::process::Command::new(&req.target).env("LIMECRAFT_HOME",&req.data).spawn().map_err(|e|e.to_string())?;Ok(())
}
pub fn cleanup_completed(){
    let Ok(exe)=std::env::current_exe()else{return;};let Some(parent)=exe.parent()else{return;};let stage=parent.join(".limecraft-update");
    if !stage.join("complete").is_file(){return;}
    // Helper may still be running; retry cleanup on the next launch rather than deleting unrelated files.
    if no_links(&stage).is_err(){return;}
    for name in ["helper.exe","previous.exe","payload.exe","request.json","previous-receipt.json"]{let _=fs::remove_file(stage.join(name));}
    if !stage.join("helper.exe").exists(){let _=fs::remove_file(stage.join("complete"));let _=fs::remove_dir(stage);}
}
#[cfg(test)]mod tests{use super::*;#[test]fn versions_and_urls(){assert!(version("v1.1.4")>version("v1.1.0"));assert!(version("v1.1.4-beta").is_none());assert!(!trusted("https://evil.test/releases/download/a"));assert!(trusted("https://github.com/tinsundew-lgtm/sundew-launcher/releases/download/v1.1.4/a.exe"));}}
