//! Content management for an instance: mods, resource packs and shaders from
//! Modrinth. Installed files are identified by hash (like Prism does), can be
//! toggled (`.disabled` suffix, like Modrinth App / Prism), removed and
//! updated. Installing resolves required dependencies against the instance's
//! game version and loader first.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::blocking::Client;

use super::download;
use super::install::InstallProgress;
use super::instance::{self, Instance};
use super::modrinth::{self, Version};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ContentKind {
    Mod,
    ResourcePack,
    Shader,
}

impl ContentKind {
    pub const ALL: [ContentKind; 3] = [ContentKind::Mod, ContentKind::ResourcePack, ContentKind::Shader];

    /// Modrinth `project_type`.
    pub fn project_type(self) -> &'static str {
        match self {
            ContentKind::Mod => "mod",
            ContentKind::ResourcePack => "resourcepack",
            ContentKind::Shader => "shader",
        }
    }

    /// Folder inside the instance's `mines/`.
    pub fn dir_name(self) -> &'static str {
        match self {
            ContentKind::Mod => "mods",
            ContentKind::ResourcePack => "resourcepacks",
            ContentKind::Shader => "shaderpacks",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContentKind::Mod => "Моды",
            ContentKind::ResourcePack => "Ресурспаки",
            ContentKind::Shader => "Шейдеры",
        }
    }

    /// Only mods depend on the mod loader; packs and shaders just need the game version.
    pub fn uses_loader(self) -> bool {
        self == ContentKind::Mod
    }
}

/// Modrinth loader names usable for this instance (Quilt runs Fabric mods).
pub fn mod_loaders(inst: &Instance) -> Vec<&'static str> {
    match inst.loader.as_str() {
        "fabric" => vec!["fabric"],
        "quilt" => vec!["quilt", "fabric"],
        "forge" => vec!["forge"],
        "neoforge" => vec!["neoforge"],
        _ => Vec::new(),
    }
}

pub fn content_dir(root: &Path, instance_name: &str, kind: ContentKind) -> PathBuf {
    instance::instance_mines(root, instance_name).join(kind.dir_name())
}

/// One file in a content folder.
#[derive(Clone, Debug)]
pub struct InstalledItem {
    pub path: PathBuf,
    pub file_name: String,
    pub enabled: bool,
    pub size: u64,
    pub sha1: String,
    /// Filled in by `identify` for files Modrinth knows.
    pub project_id: Option<String>,
    pub title: Option<String>,
    pub version_number: Option<String>,
    /// Project icon on Modrinth.
    pub icon_url: Option<String>,
}

const DISABLED_SUFFIX: &str = ".disabled";

fn is_content_file(name: &str, kind: ContentKind) -> bool {
    let base = name.strip_suffix(DISABLED_SUFFIX).unwrap_or(name).to_lowercase();
    match kind {
        ContentKind::Mod => base.ends_with(".jar"),
        ContentKind::ResourcePack | ContentKind::Shader => base.ends_with(".zip"),
    }
}

/// List the files of a content folder (sorted by name), hashing each one.
pub fn scan(root: &Path, instance_name: &str, kind: ContentKind) -> Result<Vec<InstalledItem>, String> {
    let dir = content_dir(root, instance_name, kind);
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
        if !path.is_file() || !is_content_file(&name, kind) {
            continue;
        }
        let sha1 = download::file_sha1(&path)?;
        out.push(InstalledItem {
            enabled: !name.ends_with(DISABLED_SUFFIX),
            size: e.metadata().map(|m| m.len()).unwrap_or(0),
            file_name: name,
            path,
            sha1,
            project_id: None,
            title: None,
            version_number: None,
            icon_url: None,
        });
    }
    out.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));
    Ok(out)
}

/// Fill `project_id` / `title` / `version_number` for files Modrinth recognises.
pub fn identify(client: &Client, items: &mut [InstalledItem]) -> Result<(), String> {
    let hashes: Vec<String> = items.iter().map(|i| i.sha1.clone()).collect();
    let by_hash = modrinth::versions_by_hashes(client, &hashes)?;
    let mut ids: Vec<String> = by_hash.values().map(|v| v.project_id.clone()).collect();
    ids.sort();
    ids.dedup();
    let projects: HashMap<String, (String, Option<String>)> = modrinth::get_projects(client, &ids)
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.id, (p.title, p.icon_url)))
        .collect();
    for item in items {
        if let Some(v) = by_hash.get(&item.sha1) {
            item.project_id = Some(v.project_id.clone());
            item.title = projects.get(&v.project_id).map(|p| p.0.clone());
            item.icon_url = projects.get(&v.project_id).and_then(|p| p.1.clone());
            item.version_number = Some(v.version_number.clone());
        }
    }
    Ok(())
}

/// Enable/disable by renaming `x.jar` <-> `x.jar.disabled`. Returns the new path.
pub fn set_enabled(item: &InstalledItem, enabled: bool) -> Result<PathBuf, String> {
    if item.enabled == enabled {
        return Ok(item.path.clone());
    }
    let name = item.file_name.clone();
    let new_name = if enabled {
        name.strip_suffix(DISABLED_SUFFIX).unwrap_or(&name).to_string()
    } else {
        format!("{name}{DISABLED_SUFFIX}")
    };
    let dest = item.path.with_file_name(new_name);
    if dest.exists() {
        return Err(format!("{} уже существует", dest.display()));
    }
    std::fs::rename(&item.path, &dest).map_err(|e| format!("rename {}: {e}", item.path.display()))?;
    Ok(dest)
}

pub fn remove(item: &InstalledItem) -> Result<(), String> {
    std::fs::remove_file(&item.path).map_err(|e| format!("remove {}: {e}", item.path.display()))
}

/// An available newer version of an installed file.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub file_name: String,
    pub new_version: Version,
}

/// Ask Modrinth for newer versions of every installed file (by hash), limited
/// to the instance's game version and loader.
pub fn check_updates(client: &Client, items: &[InstalledItem], inst: &Instance, kind: ContentKind) -> Result<Vec<UpdateInfo>, String> {
    let hashes: Vec<String> = items.iter().map(|i| i.sha1.clone()).collect();
    let loaders = if kind.uses_loader() { mod_loaders(inst) } else { Vec::new() };
    let games: Vec<&str> = if inst.game_version.is_empty() || inst.game_version == "latest" {
        Vec::new()
    } else {
        vec![inst.game_version.as_str()]
    };
    let latest = modrinth::latest_versions_for_hashes(client, &hashes, &loaders, &games)?;
    Ok(items
        .iter()
        .filter_map(|it| {
            let v = latest.get(&it.sha1)?;
            let already = v.files.iter().any(|f| f.hashes.get("sha1").map(|h| h.eq_ignore_ascii_case(&it.sha1)).unwrap_or(false));
            (!already).then(|| UpdateInfo { file_name: it.file_name.clone(), new_version: v.clone() })
        })
        .collect())
}

fn primary_file(v: &Version) -> Result<&modrinth::ProjectFile, String> {
    v.files
        .iter()
        .find(|f| f.primary.unwrap_or(false))
        .or_else(|| v.files.first())
        .ok_or_else(|| "в этой версии нет файлов".to_string())
}

fn set_file(prog: Option<&Arc<InstallProgress>>, name: &str) {
    if let Some(p) = prog {
        if let Ok(mut g) = p.current_file.lock() {
            *g = name.to_string();
        }
    }
}

fn cancel_flag(prog: Option<&Arc<InstallProgress>>) -> Option<&std::sync::atomic::AtomicBool> {
    prog.map(|p| &p.cancelled)
}

/// Download one version's primary file into the content folder (verified by SHA-1).
fn download_version(
    client: &Client,
    dir: &Path,
    v: &Version,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<PathBuf, String> {
    let f = primary_file(v)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let dest = instance::safe_join(dir, &f.filename)?;
    set_file(prog, &f.filename);
    let sha1 = f.hashes.get("sha1").map(|h| h.to_lowercase());
    download::download_file_ex(client, &f.url, &dest, sha1.as_deref(), cancel_flag(prog), None)?;
    Ok(dest)
}

/// Replace an installed file with a newer version; the old file is removed
/// only after the new one is verified. A disabled file stays disabled.
pub fn apply_update(
    client: &Client,
    root: &Path,
    inst: &Instance,
    kind: ContentKind,
    item: &InstalledItem,
    new_version: &Version,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<PathBuf, String> {
    let dir = content_dir(root, &inst.name, kind);
    let dest = download_version(client, &dir, new_version, prog)?;
    if dest != item.path {
        let _ = std::fs::remove_file(&item.path);
    }
    if !item.enabled {
        let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let disabled = dest.with_file_name(format!("{name}{DISABLED_SUFFIX}"));
        std::fs::rename(&dest, &disabled).map_err(|e| format!("rename {}: {e}", dest.display()))?;
        return Ok(disabled);
    }
    Ok(dest)
}

/// What an install would pull in (shown to the user / used by `install`).
#[derive(Debug, Default)]
pub struct InstallPlan {
    /// Required dependencies not yet installed, newest compatible version.
    pub dependencies: Vec<Version>,
    /// Human-readable problems (no compatible version, incompatible mods...).
    pub warnings: Vec<String>,
}

/// Resolve required dependencies of `version` for this instance.
/// `installed_projects` are project ids already present (skipped).
pub fn plan_install(
    client: &Client,
    inst: &Instance,
    kind: ContentKind,
    version: &Version,
    installed_projects: &HashSet<String>,
) -> Result<InstallPlan, String> {
    let mut plan = InstallPlan::default();
    let mut seen: HashSet<String> = installed_projects.clone();
    seen.insert(version.project_id.clone());
    let mut queue: Vec<Version> = vec![version.clone()];
    let loaders = if kind.uses_loader() { mod_loaders(inst) } else { Vec::new() };
    let game = Some(inst.game_version.as_str()).filter(|g| !g.is_empty() && *g != "latest");

    while let Some(cur) = queue.pop() {
        for dep in &cur.dependencies {
            let kind_str = dep.dependency_type.as_deref().unwrap_or("required");
            if kind_str == "incompatible" {
                if let Some(pid) = &dep.project_id {
                    if installed_projects.contains(pid) {
                        plan.warnings.push(format!("несовместим с уже установленным проектом {pid}"));
                    }
                }
                continue;
            }
            if kind_str != "required" {
                continue;
            }
            // a pinned version wins, otherwise the newest compatible one
            let resolved: Result<Option<Version>, String> = match (&dep.version_id, &dep.project_id) {
                (Some(vid), _) => modrinth::get_version(client, vid).map(Some),
                (None, Some(pid)) if !seen.contains(pid) => {
                    let list = match game {
                        Some(g) => modrinth::get_versions_for(client, pid, g, &loaders)?,
                        None => modrinth::get_project_versions(client, pid)?,
                    };
                    Ok(list.into_iter().next())
                }
                _ => Ok(None),
            };
            let pid = dep.project_id.clone().unwrap_or_default();
            match resolved? {
                Some(v) => {
                    if seen.insert(v.project_id.clone()) {
                        queue.push(v.clone());
                        plan.dependencies.push(v);
                    }
                }
                None if !pid.is_empty() && !seen.contains(&pid) => {
                    plan.warnings.push(format!("для зависимости {pid} нет версии под Minecraft {}", inst.game_version));
                }
                None => {}
            }
        }
    }
    Ok(plan)
}

/// Result of an install.
#[derive(Debug, Default)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub warnings: Vec<String>,
}

/// Install `version` plus its required dependencies into the instance.
pub fn install(
    client: &Client,
    root: &Path,
    inst: &Instance,
    kind: ContentKind,
    version: &Version,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<InstallReport, String> {
    if kind == ContentKind::Mod && mod_loaders(inst).is_empty() {
        return Err("У этой сборки нет загрузчика (vanilla) — моды работать не будут. Создайте сборку с Fabric, Quilt, Forge или NeoForge.".into());
    }
    let dir = content_dir(root, &inst.name, kind);
    let mut present = scan(root, &inst.name, kind)?;
    let _ = identify(client, &mut present); // best effort: skip dependencies already installed
    let installed_projects: HashSet<String> = present.iter().filter_map(|i| i.project_id.clone()).collect();

    let plan = plan_install(client, inst, kind, version, &installed_projects)?;
    let mut report = InstallReport { installed: Vec::new(), warnings: plan.warnings };

    for v in std::iter::once(version).chain(plan.dependencies.iter()) {
        if let Some(p) = prog {
            if p.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
        }
        let dest = download_version(client, &dir, v, prog)?;
        report.installed.push(dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-mods-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scan_toggle_remove() {
        let root = tmp("scan");
        let dir = content_dir(&root, "i", ContentKind::Mod);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jar"), b"aaa").unwrap();
        std::fs::write(dir.join("b.jar.disabled"), b"bbb").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let items = scan(&root, "i", ContentKind::Mod).unwrap();
        assert_eq!(items.len(), 2);
        assert!(items[0].enabled && items[0].file_name == "a.jar");
        assert!(!items[1].enabled && items[1].file_name == "b.jar.disabled");

        let new_path = set_enabled(&items[0], false).unwrap();
        assert!(new_path.ends_with("a.jar.disabled") && new_path.exists() && !dir.join("a.jar").exists());

        let items = scan(&root, "i", ContentKind::Mod).unwrap();
        let back = set_enabled(&items[0], true).unwrap();
        assert!(back.ends_with("a.jar") && back.exists());

        let items = scan(&root, "i", ContentKind::Mod).unwrap();
        remove(&items[0]).unwrap();
        assert_eq!(scan(&root, "i", ContentKind::Mod).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn packs_use_zip_and_own_folder() {
        let root = tmp("packs");
        let dir = content_dir(&root, "i", ContentKind::ResourcePack);
        assert!(dir.ends_with("resourcepacks"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.zip"), b"z").unwrap();
        std::fs::write(dir.join("m.jar"), b"j").unwrap();
        assert_eq!(scan(&root, "i", ContentKind::ResourcePack).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn loaders_per_instance() {
        let mut i = Instance::default();
        assert!(mod_loaders(&i).is_empty());
        i.loader = "quilt".into();
        assert_eq!(mod_loaders(&i), vec!["quilt", "fabric"]);
    }
}
