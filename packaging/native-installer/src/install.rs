use std::{fs,io::Write,path::{Path,PathBuf},sync::atomic::{AtomicUsize,Ordering}};
use serde::{Serialize,Deserialize};
use sha2::{Digest,Sha256};
const EXE:&str="limecraft-launcher.exe";
const RECEIPT:&str="limecraft-install.json";
#[derive(Serialize,Deserialize)] struct Receipt { product:String, sha256:String }
fn hash(b:&[u8])->String {hex::encode(Sha256::digest(b))}
fn error(e:impl std::fmt::Display)->String {e.to_string()}
fn safe(p:&Path)->Result<(),String> {
    for p in p.ancestors() { match fs::symlink_metadata(p) {
        Ok(m)=> {
            #[cfg(windows)] { use std::os::windows::fs::MetadataExt; if m.file_attributes() & 0x400 !=0 { return Err("Путь содержит ссылку или junction".into()); } }
            if m.file_type().is_symlink() {return Err("Путь содержит ссылку".into());}
        },
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>(),Err(e)=>return Err(error(e)),
    }} Ok(())
}
fn new_file(path:&Path,bytes:&[u8])->Result<(),String> {
    let mut f=fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(error)?;
    f.write_all(bytes).map_err(error)?; f.sync_all().map_err(error)
}
pub fn install(dir:&Path,payload:&[u8],expected:&str,progress:&AtomicUsize)->Result<PathBuf,String> {
    if !dir.is_absolute() || dir.parent().is_none() {return Err("Выберите абсолютный путь к отдельной папке приложения".into());}
    if payload.is_empty() || !payload.starts_with(b"MZ") || hash(payload)!=expected {return Err("Встроенный launcher.exe отсутствует или повреждён".into());}
    safe(dir)?; fs::create_dir_all(dir).map_err(error)?;
    let target=dir.join(EXE); let receipt=dir.join(RECEIPT);
    safe(&target)?; safe(&receipt)?;
    let updating=target.exists();
    if updating {
        let old:Receipt=serde_json::from_slice(&fs::read(&receipt).map_err(|_|"В папке уже есть exe без квитанции LimeCraft. Выберите другую папку.")?).map_err(error)?;
        if old.product!="LimeCraft" || hash(&fs::read(&target).map_err(error)?)!=old.sha256 {return Err("Существующий exe изменён или принадлежит другой установке; замена отменена".into());}
    } else if receipt.exists() {return Err("Неполная старая установка: выберите новую папку".into());}
    progress.store(15,Ordering::Relaxed);
    // Refuse stale stages, never overwrite arbitrary files with reserved names.
    let stage=dir.join(".limecraft-setup-stage"); safe(&stage)?;
    fs::create_dir(&stage).map_err(|e|format!("Не удалось создать staging {}: {e}. Если осталась от прошлой установки, проверьте её вручную.",stage.display()))?;
    let staged=stage.join(EXE);
    let prepare=(|| {
        new_file(&staged,payload)?; progress.store(60,Ordering::Relaxed);
        if hash(&fs::read(&staged).map_err(error)?)!=expected {return Err("Проверка записанного exe не пройдена".into());}
        let r=Receipt{product:"LimeCraft".into(),sha256:expected.into()};
        new_file(&stage.join(RECEIPT),&serde_json::to_vec_pretty(&r).map_err(error)?)?; Ok::<_,String>(())
    })();
    if let Err(e)=prepare {let _=fs::remove_file(&staged);let _=fs::remove_file(stage.join(RECEIPT));let _=fs::remove_dir(&stage);return Err(e);}
    let mut old_exe=false; let mut old_receipt=false; let mut placed=false;
    let commit=(|| {
        if updating {fs::rename(&target,stage.join("previous.exe")).map_err(error)?; old_exe=true; fs::rename(&receipt,stage.join("previous.json")).map_err(error)?; old_receipt=true;}
        fs::rename(&staged,&target).map_err(error)?; placed=true;
        fs::rename(stage.join(RECEIPT),&receipt).map_err(error)?; Ok::<_,String>(())
    })();
    if let Err(e)=commit {
        let mut recovery=Vec::new();
        if placed {if let Err(e)=fs::rename(&target,&staged) {recovery.push(error(e));}}
        if old_exe {if let Err(e)=fs::rename(stage.join("previous.exe"),&target){recovery.push(error(e));}}
        if old_receipt {if let Err(e)=fs::rename(stage.join("previous.json"),&receipt){recovery.push(error(e));}}
        return Err(format!("Установка отменена: {e}. Закройте лаунчер перед обновлением. Staging сохранён: {}. Ошибки возврата: {}",stage.display(),recovery.join("; ")));
    }
    for name in ["previous.exe","previous.json"] {let _=fs::remove_file(stage.join(name));}
    let _=fs::remove_dir(&stage); progress.store(100,Ordering::Relaxed); Ok(target)
}
pub fn validate_data_pointer(app:&Path,data:&Path)->Result<(),String>{
    if !data.is_absolute() || data.parent().is_none(){return Err("Укажите абсолютный путь к папке данных".into());}
    safe(data)?;safe(&app.join(crate::data_location::POINTER))?;
    if data.exists() && !data.is_dir(){return Err("Путь данных не является папкой".into());}
    // Existing pointer must be valid; never silently replace arbitrary content.
    crate::data_location::read_pointer(app)?;Ok(())
}
pub fn save_data_pointer(app:&Path,data:&Path)->Result<(),String>{
    validate_data_pointer(app,data)?;
    let path=app.join(crate::data_location::POINTER);let tmp=app.join(".limecraft-data.tmp");
    let bytes=serde_json::to_vec_pretty(&serde_json::json!({"data_dir":data})).map_err(error)?;
    new_file(&tmp,&bytes)?;
    fs::rename(&tmp,&path).map_err(error)
}
#[cfg(test)] mod tests {
    use super::*;
    fn dir()->PathBuf {std::env::temp_dir().join(format!("lime-setup-test-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))}
    #[test] fn install_update_and_protect_unknown_files(){let d=dir();let p=b"MZtest";let n=AtomicUsize::new(0);install(&d,p,&hash(p),&n).unwrap();fs::write(d.join("keep.txt"),b"user").unwrap();let p2=b"MZnew";install(&d,p2,&hash(p2),&n).unwrap();assert_eq!(fs::read(d.join(EXE)).unwrap(),p2);assert!(d.join("keep.txt").exists());fs::write(d.join(EXE),b"changed").unwrap();assert!(install(&d,p,&hash(p),&n).is_err());assert_eq!(fs::read(d.join(EXE)).unwrap(),b"changed");fs::remove_dir_all(d).unwrap();}
    #[test] fn data_pointer_preserves_existing_files(){let app=dir();let data=dir();fs::create_dir_all(&app).unwrap();fs::create_dir_all(data.join("instances/test")).unwrap();fs::write(data.join("launcher_state.json"),b"existing").unwrap();save_data_pointer(&app,&data).unwrap();save_data_pointer(&app,&data).unwrap();assert_eq!(crate::data_location::read_pointer(&app).unwrap(),Some(data.clone()));assert_eq!(fs::read(data.join("launcher_state.json")).unwrap(),b"existing");assert!(data.join("instances/test").exists());fs::remove_dir_all(app).unwrap();fs::remove_dir_all(data).unwrap();}
    #[test] fn rejects_bad_payload(){let d=dir();assert!(install(&d,b"MZ", "bad",&AtomicUsize::new(0)).is_err());assert!(!d.exists());}
}
