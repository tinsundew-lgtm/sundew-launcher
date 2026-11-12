//! Evidence-based hints, not an automatic mod-conflict solver.
use std::{fs::File, io::{Read, Seek, SeekFrom}, path::Path};
use super::instance::{self, Instance};
pub fn analyze_text(text: &str) -> Vec<String> {
    let t = text.to_lowercase(); let mut out = Vec::new();
    for (needles, hint) in [
        (&["outofmemoryerror", "java heap space"][..], "Нехватка памяти Java. Проверьте лимит RAM сборки и свободную память; не выделяйте всю память компьютера."),
        (&["unsupportedclassversionerror", "has been compiled by a more recent version"][..], "Несовместимая Java. Сверьте требуемую версию Java с версией Minecraft и загрузчика."),
        (&["incompatible mods found", "modresolutionexception", "requires version"][..], "Загрузчик сообщает о несовместимости или недостающей зависимости. Найдите строки requires / depends в журнале; сравните моды с последней рабочей капсулой."),
        (&["mixinapplyerror", "mixin transformation", "invalidmixinexception"][..], "Ошибка применения Mixin. Возможна несовместимость модов, но конкретный виновник не установлен. Проверьте указанные в журнале моды и версии загрузчика."),
        (&["glfw error 65542", "pixel format not accelerated"][..], "Не удалось инициализировать графику. Проверьте драйвер видеокарты и поддержку OpenGL."),
        (&["unable to access jarfile", "could not find or load main class"][..], "Не найден стартовый файл или класс. Проверьте полноту установки и выбранную версию; не удаляйте миры для переустановки."),
    ] { if needles.iter().any(|s| t.contains(s)) { out.push(hint.into()); } }
    if out.is_empty() { out.push("В прочитанной части журналов нет известных сигнатур. Это не означает, что сборка исправна. Посмотрите последние ошибки во вкладке «Журнал».".into()); }
    out
}
pub fn inspect(root: &Path, inst: &Instance) -> Result<Vec<String>,String> {
    let game = instance::instance_mines(root,&inst.name); let mut text = String::new(); let mut sources = Vec::new();
    for rel in ["launcher.log", "logs/latest.log"] {
        let path = game.join(rel);
        let mut f = match File::open(&path) { Ok(f) => f, Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, Err(e) => return Err(format!("{rel}: {e}")) };
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        f.seek(SeekFrom::Start(size.saturating_sub(512 * 1024))).map_err(|e|e.to_string())?;
        let mut bytes = Vec::new(); f.take(512*1024).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
        text.push_str(&String::from_utf8_lossy(&bytes)); text.push('\n'); sources.push(rel);
    }
    if sources.is_empty() { return Ok(vec!["Журналов пока нет. Запустите сборку и повторите диагностику после ошибки.".into()]); }
    let mut out=vec![format!("Источники: {} (последние 512 КиБ каждого; могут относиться к предыдущему запуску).",sources.join(", "))];
    out.extend(analyze_text(&text));
    let evidence:Vec<_>=text.lines().filter(|l| {
        let l=l.to_lowercase(); ["caused by:","requires ","depends on", "incompatible", "outofmemoryerror", "unsupportedclassversionerror", "mixinapplyerror"].iter().any(|n|l.contains(n))
    }).rev().take(12).map(|l|format!("Фрагмент журнала: {}",l.chars().take(350).collect::<String>())).collect();
    out.extend(evidence);
    match super::capsules::list(root,&inst.name) {
        Ok(caps) => if let Some(c)=caps.first() {
            out.push(format!("Сравнение с последней капсулой «{}» (это не обязательно рабочее состояние):",c.label));
            match super::capsules::compare(root,inst,&c.id) {
                Ok(changes)=> {out.push(format!("Изменений: {}",changes.len()));out.extend(changes.into_iter().take(40));},
                Err(e)=>out.push(format!("Не удалось сравнить: {e}")),
            }
        },
        Err(e)=>out.push(format!("Капсулы недоступны: {e}")),
    }
    // Inspect local archives only: do not load or execute mod code.
    let mut ids=std::collections::BTreeMap::<String,Vec<String>>::new();
    for item in super::mods::scan(root,&inst.name,super::mods::ContentKind::Mod)?.into_iter().filter(|i|i.enabled) {
        if let Ok(file)=File::open(&item.path) {
            match zip::ZipArchive::new(file) {
                Err(_)=>out.push(format!("Не читается JAR/ZIP: {}. Проверьте файл или восстановите его из капсулы.",item.file_name)),
                Ok(mut archive)=> {
                    if let Ok(mut meta)=archive.by_name("fabric.mod.json") {
                        let mut bytes=Vec::new(); if meta.size()<=128*1024 && meta.by_ref().take(128*1024).read_to_end(&mut bytes).is_ok() {
                            if let Ok(json)=serde_json::from_slice::<serde_json::Value>(&bytes) {
                                if let Some(id)=json.get("id").and_then(|v|v.as_str()){ids.entry(id.to_owned()).or_default().push(item.file_name.clone());}
                            }
                        }
                    }
                }
            }
        }
    }
    for (id,files) in ids {if files.len()>1{out.push(format!("Повторяющийся Fabric mod id «{id}»: {}. Это признак дублирования; перед отключением создайте капсулу.",files.join(", ")));}}
    Ok(out)
}
#[cfg(test)] mod tests { use super::*; #[test] fn hints_require_evidence() { assert!(analyze_text("fine")[0].contains("нет известных")); assert!(analyze_text("java.lang.OutOfMemoryError")[0].contains("памяти")); assert!(analyze_text("UnsupportedClassVersionError")[0].contains("Java")); } }
