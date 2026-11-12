//! Local, content-verified configuration snapshots. Never follows filesystem links.
use std::{collections::BTreeMap, fs, io::Read, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::instance::{self, Instance};

const SCOPES: &[&str] = &["mods", "config", "resourcepacks", "shaderpacks", "options.txt", "optionsof.txt", "versions"];
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord { pub path: String, pub sha256: String, pub size: u64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capsule {
    pub id: String, pub label: String, pub created: u64,
    pub files: Vec<FileRecord>, pub instance: Instance, pub include_saves: bool,
}
fn err(e: impl std::fmt::Display) -> String { e.to_string() }
fn plain(path: &Path) -> Result<(), String> {
    // Check ancestors too: on Windows junctions carry FILE_ATTRIBUTE_REPARSE_POINT.
    for p in path.ancestors() {
        match fs::symlink_metadata(p) {
            Ok(m) => {
                #[cfg(windows)]
                { use std::os::windows::fs::MetadataExt; if m.file_attributes() & 0x400 != 0 { return Err(format!("Ссылки запрещены: {}", p.display())); } }
                if m.file_type().is_symlink() { return Err(format!("Ссылки запрещены: {}", p.display())); }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(err(e)),
        }
    }
    Ok(())
}
fn store(root: &Path, name: &str) -> PathBuf { instance::instance_dir(root, name).join("capsules") }
fn scopes(worlds: bool) -> Vec<&'static str> { let mut s = SCOPES.to_vec(); if worlds { s.push("saves"); } s }
fn digest(path: &Path) -> Result<(String,u64), String> {
    plain(path)?;
    let mut f = fs::File::open(path).map_err(err)?;
    let mut h = Sha256::new(); let mut size = 0; let mut buf = [0u8; 65536];
    loop { let n = f.read(&mut buf).map_err(err)?; if n == 0 { break; } h.update(&buf[..n]); size += n as u64; }
    Ok((hex::encode(h.finalize()), size))
}
fn walk(base: &Path, rel: &Path, out: &mut Vec<FileRecord>) -> Result<(), String> {
    let p = base.join(rel); plain(&p)?;
    let meta = match fs::metadata(&p) { Ok(m) => m, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()), Err(e) => return Err(err(e)) };
    if meta.is_dir() {
        for e in fs::read_dir(&p).map_err(err)? { let e = e.map_err(err)?; walk(base, &rel.join(e.file_name()), out)?; }
    } else if meta.is_file() {
        let path = rel.to_str().ok_or("Имя файла не является UTF-8")?.replace('\\', "/");
        let (sha256, size) = digest(&p)?; out.push(FileRecord {path, sha256, size});
    } else { return Err(format!("Необычный файл: {}", p.display())); }
    Ok(())
}
fn inventory(base: &Path, worlds: bool) -> Result<Vec<FileRecord>, String> {
    let mut files = Vec::new(); for s in scopes(worlds) { walk(base, Path::new(s), &mut files)?; }
    files.sort_by(|a,b| a.path.cmp(&b.path)); Ok(files)
}
fn valid_id(id: &str) -> Result<(), String> { if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-') { Err("Некорректный ID капсулы".into()) } else { Ok(()) } }
fn load(root: &Path, name: &str, id: &str) -> Result<Capsule, String> {
    valid_id(id)?; let dir = store(root,name).join(id); plain(&dir)?;
    plain(&dir.join("manifest.json"))?;
    let c: Capsule = serde_json::from_slice(&fs::read(dir.join("manifest.json")).map_err(err)?).map_err(err)?;
    if c.id != id { return Err("ID манифеста не совпадает".into()); }
    let allowed = scopes(c.include_saves);
    let mut seen = std::collections::HashSet::new();
    for f in &c.files {
        let parts: Vec<_> = f.path.split('/').collect();
        if f.path.contains(['\\', ':']) || parts.iter().any(|s| s.is_empty() || *s == "." || *s == "..") || !allowed.contains(&parts[0]) || !seen.insert(f.path.to_lowercase()) {
            return Err("Небезопасный путь в капсуле".into());
        }
        instance::safe_join(&dir.join("files"), &f.path)?;
    }
    Ok(c)
}
pub fn list(root: &Path, name: &str) -> Result<Vec<Capsule>, String> {
    let base = store(root,name); plain(&base)?;
    if !base.exists() { return Ok(Vec::new()); }
    let mut out = Vec::new();
    for e in fs::read_dir(base).map_err(err)? {
        let e = e.map_err(err)?; let id = e.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') { continue; }
        out.push(load(root,name,&id).map_err(|e| format!("Капсула {id}: {e}"))?);
    }
    out.sort_by(|a,b| b.id.cmp(&a.id)); Ok(out)
}
pub fn create(root: &Path, inst: &Instance, label: &str, include_saves: bool) -> Result<Capsule, String> {
    let base = store(root,&inst.name); plain(&base)?; fs::create_dir_all(&base).map_err(err)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(err)?;
    let id = format!("{}-{}", now.as_secs(), now.subsec_nanos());
    let temp = base.join(format!(".{id}")); let dest = base.join(&id);
    fs::create_dir(&temp).map_err(err)?;
    let result = (|| {
        let source = instance::instance_mines(root,&inst.name);
        let files = inventory(&source,include_saves)?;
        for f in &files {
            let to = temp.join("files").join(&f.path); fs::create_dir_all(to.parent().unwrap()).map_err(err)?;
            plain(&source.join(&f.path))?; fs::copy(source.join(&f.path), &to).map_err(err)?;
            if digest(&to)? != (f.sha256.clone(),f.size) { return Err("Файлы изменились во время создания капсулы; повторите после остановки игры".into()); }
        }
        let c = Capsule { id: id.clone(), label: label.trim().chars().take(120).collect(), created: now.as_secs(), files, instance: inst.clone(), include_saves };
        fs::write(temp.join("manifest.json"), serde_json::to_vec_pretty(&c).map_err(err)?).map_err(err)?;
        fs::rename(&temp,&dest).map_err(err)?; Ok(c)
    })();
    if result.is_err() { let _ = fs::remove_dir_all(&temp); }
    result
}
pub fn compare(root: &Path, inst: &Instance, id: &str) -> Result<Vec<String>, String> {
    let c = load(root,&inst.name,id)?;
    let before: BTreeMap<_,_> = c.files.iter().map(|f| (f.path.as_str(),f.sha256.as_str())).collect();
    let current = inventory(&instance::instance_mines(root,&inst.name), c.include_saves)?;
    let after: BTreeMap<_,_> = current.iter().map(|f| (f.path.as_str(),f.sha256.as_str())).collect();
    let mut lines = Vec::new();
    for (p,h) in &before { match after.get(p) { None => lines.push(format!("Удалён: {p}")), Some(v) if v != h => lines.push(format!("Изменён: {p}")), _=>() } }
    for p in after.keys() { if !before.contains_key(p) { lines.push(format!("Добавлен: {p}")); } }
    for (label,a,b) in [("Minecraft", &c.instance.game_version,&inst.game_version),("Загрузчик",&c.instance.loader,&inst.loader),("Версия загрузчика",&c.instance.loader_version,&inst.loader_version)] {
        if a != b { lines.push(format!("{label}: {a} → {b}")); }
    }
    if c.instance.memory_mb != inst.memory_mb || c.instance.java_path != inst.java_path || c.instance.jvm_args != inst.jvm_args || c.instance.version_name != inst.version_name { lines.push("Изменены настройки запуска".into()); }
    Ok(lines)
}
pub fn restore(root: &Path, inst: &Instance, id: &str) -> Result<Instance,String> {
    let c = load(root,&inst.name,id)?;
    let source = store(root,&inst.name).join(id).join("files");
    for f in &c.files { if digest(&source.join(&f.path))? != (f.sha256.clone(),f.size) { return Err(format!("Повреждена капсула: {}",f.path)); } }
    // Mandatory durable safety copy before touching the live instance.
    let backup = create(root,inst,"Перед откатом",c.include_saves)?;
    let game = instance::instance_mines(root,&inst.name); plain(&game)?;
    let stage = store(root,&inst.name).join(format!(".restore-{}",backup.id));
    fs::create_dir(&stage).map_err(err)?;
    let prepared = (|| {
        for f in &c.files { let to = stage.join("new").join(&f.path); fs::create_dir_all(to.parent().unwrap()).map_err(err)?; fs::copy(source.join(&f.path),&to).map_err(err)?; if digest(&to)? != (f.sha256.clone(),f.size) { return Err("Ошибка проверки восстановленных файлов".into()); } }
        Ok::<_,String>(())
    })();
    if let Err(e) = prepared { let _ = fs::remove_dir_all(&stage); return Err(e); }
    fs::create_dir_all(stage.join("old")).map_err(err)?;
    let mut moved: Vec<(&str,bool,bool)> = Vec::new();
    let result = (|| {
        for scope in scopes(c.include_saves) {
            let live = game.join(scope); plain(&live)?;
            // Check all live descendants before moving an existing subtree.
            let mut check = Vec::new(); walk(&game,Path::new(scope),&mut check)?;
            let existed = live.exists();
            if existed { fs::rename(&live,stage.join("old").join(scope)).map_err(err)?; }
            moved.push((scope,existed,false));
            let new = stage.join("new").join(scope);
            if new.exists() { fs::rename(new,&live).map_err(err)?; moved.last_mut().unwrap().2 = true; }
        }
        Ok::<_,String>(())
    })();
    if let Err(e) = result {
        let mut failures = Vec::new();
        for (scope,existed,installed) in moved.into_iter().rev() {
            let live = game.join(scope);
            if installed { if let Err(e) = fs::rename(&live,stage.join("new").join(scope)) { failures.push(err(e)); continue; } }
            if existed { if let Err(e) = fs::rename(stage.join("old").join(scope),&live) { failures.push(err(e)); } }
        }
        return Err(format!("Откат не выполнен: {e}. Страховочная капсула {}. Ошибки возврата: {}", backup.id, failures.join("; ")));
    }
    // Keep pre-restore capsule; staging only contains redundant files.
    let _ = fs::remove_dir_all(&stage);
    let mut restored = inst.clone();
    restored.game_version = c.instance.game_version; restored.loader = c.instance.loader;
    restored.loader_version = c.instance.loader_version; restored.version_name = c.instance.version_name;
    restored.memory_mb = c.instance.memory_mb; restored.jvm_args = c.instance.jvm_args; restored.java_path = c.instance.java_path;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf,Instance) { let root = std::env::temp_dir().join(format!("lime-capsule-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())); let i = Instance { name:"test".into(), ..Default::default() }; fs::create_dir_all(instance::instance_mines(&root,&i.name).join("mods")).unwrap(); (root,i) }
    #[test] fn roundtrip_and_safety_copy() {
        let (r,i)=fixture(); let g=instance::instance_mines(&r,&i.name);
        fs::write(g.join("mods/a.jar"),b"old").unwrap(); fs::create_dir_all(g.join("saves")).unwrap(); fs::write(g.join("saves/world"),b"world").unwrap();
        let c=create(&r,&i,"good",false).unwrap(); fs::write(g.join("mods/a.jar"),b"new").unwrap(); fs::write(g.join("mods/b.jar"),b"extra").unwrap();
        assert_eq!(compare(&r,&i,&c.id).unwrap().len(),2); restore(&r,&i,&c.id).unwrap(); assert_eq!(fs::read(g.join("mods/a.jar")).unwrap(),b"old"); assert!(!g.join("mods/b.jar").exists()); assert!(g.join("saves/world").exists()); assert_eq!(list(&r,&i.name).unwrap().len(),2); fs::remove_dir_all(r).unwrap();
    }
    #[test] fn corrupted_payload_never_touches_live() { let (r,i)=fixture(); let g=instance::instance_mines(&r,&i.name); fs::write(g.join("mods/a"),b"ok").unwrap(); let c=create(&r,&i,"",false).unwrap(); fs::write(store(&r,&i.name).join(&c.id).join("files/mods/a"),b"bad").unwrap(); assert!(restore(&r,&i,&c.id).is_err()); assert_eq!(fs::read(g.join("mods/a")).unwrap(),b"ok"); assert!(restore(&r,&i,"../x").is_err()); fs::remove_dir_all(r).unwrap(); }
    #[test] fn rejects_manifest_traversal_and_duplicate_paths() {
        let (r,i)=fixture(); let g=instance::instance_mines(&r,&i.name); fs::write(g.join("mods/a"),b"ok").unwrap();
        let mut c=create(&r,&i,"",false).unwrap(); let manifest=store(&r,&i.name).join(&c.id).join("manifest.json");
        c.files[0].path="mods/../../outside".into(); fs::write(&manifest,serde_json::to_vec(&c).unwrap()).unwrap(); assert!(restore(&r,&i,&c.id).is_err());
        c.files[0].path="mods/a".into(); let mut duplicate=c.files[0].clone(); duplicate.path="MODS/A".into(); c.files.push(duplicate);
        fs::write(&manifest,serde_json::to_vec(&c).unwrap()).unwrap(); assert!(restore(&r,&i,&c.id).is_err()); assert_eq!(fs::read(g.join("mods/a")).unwrap(),b"ok"); fs::remove_dir_all(r).unwrap();
    }
    #[test] fn worlds_opt_in() { let (r,i)=fixture(); let g=instance::instance_mines(&r,&i.name); fs::create_dir_all(g.join("saves")).unwrap(); fs::write(g.join("saves/world"),b"before").unwrap(); let c=create(&r,&i,"worlds",true).unwrap(); fs::write(g.join("saves/world"),b"after").unwrap(); restore(&r,&i,&c.id).unwrap(); assert_eq!(fs::read(g.join("saves/world")).unwrap(),b"before"); fs::remove_dir_all(r).unwrap(); }
}
