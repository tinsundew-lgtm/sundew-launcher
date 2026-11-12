//! Shared by launcher and native installer. Only points to data; never migrates/deletes it.
use std::path::{Path,PathBuf};
pub const POINTER:&str="limecraft-data.json";
pub fn read_pointer(exe_dir:&Path)->Result<Option<PathBuf>,String>{
    let path=exe_dir.join(POINTER);
    let bytes=match std::fs::read(&path){Ok(b)=>b,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(e)=>return Err(format!("{}: {e}",path.display()))};
    let value:serde_json::Value=serde_json::from_slice(&bytes).map_err(|e|format!("{}: {e}",path.display()))?;
    let dir=PathBuf::from(value.get("data_dir").and_then(|v|v.as_str()).ok_or("Нет data_dir в limecraft-data.json")?);
    if !dir.is_absolute(){return Err("data_dir должен быть абсолютным путём".into());}Ok(Some(dir))
}
pub fn has_data(dir:&Path)->bool {dir.join("launcher_state.json").is_file() || dir.join("instances").is_dir()}
pub fn candidates(app_dir:&Path,setup_dir:&Path)->Vec<PathBuf>{
    let mut paths=Vec::new();
    if let Some(p)=std::env::var_os("LIMECRAFT_HOME").filter(|p|!p.is_empty()){paths.push(PathBuf::from(p));}
    for dir in [app_dir,setup_dir]{if let Ok(Some(p))=read_pointer(dir){paths.push(p);} paths.push(dir.join("LimeCraft-data"));paths.push(dir.to_path_buf());}
    if let Some(p)=dirs::data_dir(){paths.push(p.join("LimeCraft"));}
    paths.push(std::env::temp_dir().join("LimeCraft"));
    let mut unique=Vec::new();for p in paths{if p.is_absolute() && !unique.contains(&p){unique.push(p);}}unique
}
#[cfg(test)] mod tests {use super::*;#[test]fn reads_pointer_without_touching_data(){let p=std::env::temp_dir().join(format!("lc-pointer-{}",std::process::id()));std::fs::create_dir_all(&p).unwrap();assert!(read_pointer(&p).unwrap().is_none());std::fs::write(p.join(POINTER),br#"{"data_dir":"relative"}"#).unwrap();assert!(read_pointer(&p).is_err());std::fs::write(p.join(POINTER),serde_json::to_vec(&serde_json::json!({"data_dir":p.join("existing")})).unwrap()).unwrap();assert_eq!(read_pointer(&p).unwrap(),Some(p.join("existing")));assert!(!p.join("existing").exists());std::fs::remove_dir_all(p).unwrap();}}
