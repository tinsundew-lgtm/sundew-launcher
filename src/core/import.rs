//! Bringing existing builds into LimeCraft: from a game folder, from an archive
//! (Modrinth `.mrpack`, Prism / MultiMC `.zip`) and from other launchers that
//! are installed on this computer.
//!
//! Only the user's own data is copied (mods, configs, packs, options, worlds);
//! the game itself and the loader are installed fresh by LimeCraft.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::blocking::Client;

use super::install::{self, InstallProgress};
use super::instance::{self, Instance};

/// What gets copied from a game folder.
const DIRS: [&str; 8] = ["mods", "config", "resourcepacks", "shaderpacks", "scripts", "kubejs", "defaultconfigs", "datapacks"];
const FILES: [&str; 4] = ["options.txt", "optionsof.txt", "optionsshaders.txt", "servers.dat"];

/// Archive limits (a zip is untrusted input).
const MAX_ENTRY: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 8 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;

/// A build found in another launcher.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// Launcher it comes from ("Prism Launcher", ...).
    pub source: String,
    pub name: String,
    /// Folder of the build (its game files are inside, see [`game_dir`]).
    pub path: PathBuf,
    /// Empty when the launcher does not tell (the user picks it).
    pub game_version: String,
    pub loader: String,
    pub loader_version: String,
}

impl Found {
    /// Version and loader are known, so it can be imported in one click.
    pub fn is_complete(&self) -> bool {
        !self.game_version.is_empty()
    }
}

// ---------------------------------------------------------------- parsing

/// `(minecraft, loader, loader version)` from a Prism / MultiMC `mmc-pack.json`.
pub fn parse_mmc_pack(text: &str) -> Option<(String, String, String)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut mc = String::new();
    let mut loader = (String::new(), String::new());
    for c in v.get("components")?.as_array()? {
        let uid = c.get("uid").and_then(|x| x.as_str()).unwrap_or("");
        let ver = c.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string();
        match uid {
            "net.minecraft" => mc = ver,
            "net.fabricmc.fabric-loader" => loader = ("fabric".into(), ver),
            "org.quiltmc.quilt-loader" => loader = ("quilt".into(), ver),
            "net.minecraftforge" => loader = ("forge".into(), ver),
            "net.neoforged" => loader = ("neoforge".into(), ver),
            _ => {}
        }
    }
    if mc.is_empty() {
        return None;
    }
    // Forge is installed as "<minecraft>-<forge>"
    if loader.0 == "forge" && !loader.1.contains('-') {
        loader.1 = format!("{mc}-{}", loader.1);
    }
    Some((mc, loader.0, loader.1))
}

/// `name=` from an `instance.cfg`.
pub fn parse_instance_cfg_name(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("name="))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
}

/// `(name, minecraft, loader, loader version)` from a CurseForge `minecraftinstance.json`.
pub fn parse_curseforge(text: &str) -> Option<(String, String, String, String)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mc = v.get("gameVersion")?.as_str()?.to_string();
    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let (mut loader, mut lv) = (String::new(), String::new());
    if let Some(full) = v.get("baseModLoader").and_then(|b| b.get("name")).and_then(|x| x.as_str()) {
        // "forge-47.2.0", "fabric-0.15.7-1.20.1", "neoforge-21.1.50"
        let (kind, rest) = full.split_once('-').unwrap_or((full, ""));
        match kind {
            "forge" => {
                loader = "forge".into();
                lv = if rest.contains('-') { rest.to_string() } else { format!("{mc}-{rest}") };
            }
            "neoforge" => {
                loader = "neoforge".into();
                lv = rest.to_string();
            }
            "fabric" | "quilt" => {
                loader = kind.into();
                lv = rest.strip_suffix(&format!("-{mc}")).unwrap_or(rest).to_string();
            }
            _ => {}
        }
    }
    Some((name, mc, loader, lv))
}

/// Folder with the game files of a build: `.minecraft` / `minecraft` inside it
/// (Prism, MultiMC) or the folder itself.
pub fn game_dir(path: &Path) -> PathBuf {
    for sub in [".minecraft", "minecraft"] {
        let p = path.join(sub);
        if p.is_dir() {
            return p;
        }
    }
    path.to_path_buf()
}

// ---------------------------------------------------------------- other launchers

#[derive(Clone, Copy)]
enum Kind {
    /// Prism / PolyMC / MultiMC layout (`mmc-pack.json`).
    Mmc,
    CurseForge,
    /// Plain game folders, nothing says which version.
    Plain,
}

/// Look into the folders where other launchers keep builds. `roots` is
/// `(launcher name, folder with the builds, layout)`.
fn scan(roots: &[(&str, PathBuf, Kind)]) -> Vec<Found> {
    let mut out = Vec::new();
    for (source, dir, kind) in roots {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for path in dirs {
            let folder = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if folder.starts_with('_') || folder.starts_with('.') {
                continue;
            }
            let found = match kind {
                Kind::Mmc => {
                    let Some((mc, loader, lv)) = std::fs::read_to_string(path.join("mmc-pack.json")).ok().and_then(|t| parse_mmc_pack(&t)) else {
                        continue;
                    };
                    let name = std::fs::read_to_string(path.join("instance.cfg")).ok().and_then(|t| parse_instance_cfg_name(&t)).unwrap_or(folder);
                    Found { source: source.to_string(), name, path, game_version: mc, loader, loader_version: lv }
                }
                Kind::CurseForge => {
                    let Some((name, mc, loader, lv)) =
                        std::fs::read_to_string(path.join("minecraftinstance.json")).ok().and_then(|t| parse_curseforge(&t))
                    else {
                        continue;
                    };
                    let name = if name.is_empty() { folder } else { name };
                    Found { source: source.to_string(), name, path, game_version: mc, loader, loader_version: lv }
                }
                Kind::Plain => {
                    let g = game_dir(&path);
                    if !DIRS.iter().any(|d| g.join(d).is_dir()) && !g.join("saves").is_dir() {
                        continue;
                    }
                    Found { source: source.to_string(), name: folder, path, game_version: String::new(), loader: String::new(), loader_version: String::new() }
                }
            };
            out.push(found);
        }
    }
    out
}

/// Builds of the launchers installed in their usual places.
pub fn scan_launchers() -> Vec<Found> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut roots: Vec<(&str, PathBuf, Kind)> = Vec::new();
    if let Some(app) = env("APPDATA") {
        roots.push(("Prism Launcher", app.join("PrismLauncher").join("instances"), Kind::Mmc));
        roots.push(("PolyMC", app.join("PolyMC").join("instances"), Kind::Mmc));
        roots.push(("Modrinth App", app.join("ModrinthApp").join("profiles"), Kind::Plain));
    }
    if let Some(home) = env("USERPROFILE").or_else(|| env("HOME")) {
        for base in [home.join("curseforge"), home.join("Documents").join("curseforge")] {
            roots.push(("CurseForge", base.join("minecraft").join("Instances"), Kind::CurseForge));
        }
        // Linux: ~/.local/share/<Launcher> and the Flatpak copies of Prism
        let share = env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
        roots.push(("Prism Launcher", share.join("PrismLauncher").join("instances"), Kind::Mmc));
        roots.push((
            "Prism Launcher",
            home.join(".var/app/org.prismlauncher.PrismLauncher/data/PrismLauncher/instances"),
            Kind::Mmc,
        ));
        roots.push(("PolyMC", share.join("PolyMC").join("instances"), Kind::Mmc));
        roots.push(("Modrinth App", share.join("ModrinthApp").join("profiles"), Kind::Plain));
    }
    scan(&roots)
}

/// The official launcher's `.minecraft`, if it exists (nothing says the version).
pub fn official_minecraft() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft")),
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".minecraft")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_dir())
}

// ---------------------------------------------------------------- copying

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // links could point out of the folder the user chose
        let meta = std::fs::symlink_metadata(&from).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// Copy the user's data (see [`DIRS`], [`FILES`], and `saves` when `worlds`)
/// from a game folder into an instance's `mines` folder.
pub fn copy_game_files(game: &Path, mines: &Path, worlds: bool) -> Result<usize, String> {
    let mut copied = 0;
    let mut dirs: Vec<&str> = DIRS.to_vec();
    if worlds {
        dirs.push("saves");
    }
    for d in dirs {
        let from = game.join(d);
        if from.is_dir() {
            copy_tree(&from, &mines.join(d))?;
            copied += 1;
        }
    }
    for f in FILES {
        let from = game.join(f);
        if from.is_file() {
            std::fs::create_dir_all(mines).map_err(|e| e.to_string())?;
            std::fs::copy(&from, mines.join(f)).map_err(|e| format!("copy {}: {e}", from.display()))?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn same_or_nested(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a.starts_with(&b) || b.starts_with(&a),
        _ => false,
    }
}

/// Install `inst` (game version + loader) and bring the data of the folder `src`
/// into it.
pub fn import_folder(
    client: &Client,
    root: &Path,
    inst: &Instance,
    src: &Path,
    worlds: bool,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    let game = game_dir(src);
    if !game.is_dir() {
        return Err(format!("папка не найдена: {}", src.display()));
    }
    let mines = instance::instance_mines(root, &inst.name);
    if same_or_nested(&game, &instance::instance_dir(root, &inst.name)) {
        return Err("нельзя импортировать папку из самой себя".into());
    }
    let resolved = install::install_instance(client, root, inst, prog)?;
    copy_game_files(&game, &mines, worlds)?;
    Ok(resolved)
}

// ---------------------------------------------------------------- archives

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    Mrpack,
    /// Prism / MultiMC export.
    Mmc,
}

/// What an archive contains (read without unpacking it).
#[derive(Clone, Debug)]
pub struct ArchiveInfo {
    pub kind: ArchiveKind,
    pub name: String,
    pub game_version: String,
    pub loader: String,
    pub loader_version: String,
}

fn read_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut e = archive.by_name(name).ok()?;
    if e.size() > 5 * 1024 * 1024 {
        return None;
    }
    let mut s = String::new();
    e.read_to_string(&mut s).ok()?;
    Some(s)
}

/// Folder prefix (`""` or `"Name/"`) in which `file` lies in the archive, at
/// most one folder deep.
fn find_prefix(archive: &zip::ZipArchive<std::fs::File>, file: &str) -> Option<String> {
    archive
        .file_names()
        .filter_map(|n| n.strip_suffix(file).map(|p| p.to_string()))
        .find(|p| p.is_empty() || (p.ends_with('/') && p.matches('/').count() == 1))
}

/// Recognize an archive: `.mrpack`, or a Prism / MultiMC export.
pub fn inspect_archive(path: &Path) -> Result<ArchiveInfo, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "это не архив .zip / .mrpack".to_string())?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Импорт".into());

    if let Some(text) = read_entry(&mut archive, "modrinth.index.json") {
        let idx: super::modrinth::ModrinthIndex = serde_json::from_str(&text).map_err(|e| format!("modrinth.index.json: {e}"))?;
        let deps = idx.dependencies.clone().unwrap_or_default();
        let loader = [("quilt-loader", "quilt"), ("neoforge", "neoforge"), ("forge", "forge"), ("fabric-loader", "fabric")]
            .iter()
            .find(|(k, _)| deps.contains_key(*k))
            .map(|(_, l)| l.to_string())
            .unwrap_or_default();
        return Ok(ArchiveInfo {
            kind: ArchiveKind::Mrpack,
            name: idx.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or(stem),
            game_version: deps.get("minecraft").cloned().unwrap_or_default(),
            loader,
            loader_version: String::new(),
        });
    }
    if let Some(prefix) = find_prefix(&archive, "mmc-pack.json") {
        let pack = read_entry(&mut archive, &format!("{prefix}mmc-pack.json")).ok_or("mmc-pack.json не читается")?;
        let (mc, loader, lv) = parse_mmc_pack(&pack).ok_or("в mmc-pack.json нет версии Minecraft")?;
        let name = read_entry(&mut archive, &format!("{prefix}instance.cfg")).and_then(|t| parse_instance_cfg_name(&t)).unwrap_or(stem);
        return Ok(ArchiveInfo { kind: ArchiveKind::Mmc, name, game_version: mc, loader, loader_version: lv });
    }
    if archive.by_name("manifest.json").is_ok() {
        return Err("Сборки CurseForge пока не поддерживаются: для них нужен ключ CurseForge API. Откройте её в Prism Launcher и экспортируйте оттуда, либо найдите ту же сборку на Modrinth.".into());
    }
    Err("не похоже на сборку: нет ни modrinth.index.json, ни mmc-pack.json".into())
}

/// Unpack an untrusted archive into `target` (no paths outside it, size limits).
fn safe_extract(path: &Path, target: &Path) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("не архив: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err("в архиве слишком много файлов".into());
    }
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("запись #{i}: {e}"))?;
        let name = entry.name().to_string();
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("в архиве небезопасный путь: {name}"));
        };
        let out = target.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        total = total.saturating_add(entry.size());
        if entry.size() > MAX_ENTRY || total > MAX_TOTAL {
            return Err("архив слишком большой".into());
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut dst = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::io::copy(&mut (&mut entry).take(MAX_ENTRY), &mut dst).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(())
}

/// Install a build from an archive recognized by [`inspect_archive`]. `name` is
/// the (unique) name of the new instance.
pub fn import_archive(
    client: &Client,
    root: &Path,
    archive: &Path,
    info: &ArchiveInfo,
    name: &str,
    worlds: bool,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    let base = Instance { name: name.to_string(), created: String::new(), ..Default::default() };
    match info.kind {
        ArchiveKind::Mrpack => install::install_mrpack_file(client, root, &base, archive, prog),
        ArchiveKind::Mmc => {
            let tmp = root.join("cache").join(format!("import-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&tmp);
            let result = (|| {
                safe_extract(archive, &tmp)?;
                let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
                let zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
                let prefix = find_prefix(&zip, "mmc-pack.json").unwrap_or_default();
                let inst = Instance {
                    name: name.to_string(),
                    game_version: info.game_version.clone(),
                    loader: info.loader.clone(),
                    loader_version: info.loader_version.clone(),
                    ..Default::default()
                };
                import_folder(client, root, &inst, &tmp.join(prefix.trim_end_matches('/')), worlds, prog)
            })();
            let _ = std::fs::remove_dir_all(&tmp);
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-import-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const PACK: &str = r#"{"components":[{"uid":"net.minecraft","version":"1.20.1"},{"uid":"net.minecraftforge","version":"47.2.0"}],"formatVersion":1}"#;

    #[test]
    fn mmc_pack_gives_version_and_loader() {
        assert_eq!(parse_mmc_pack(PACK), Some(("1.20.1".into(), "forge".into(), "1.20.1-47.2.0".into())));
        let fabric = r#"{"components":[{"uid":"net.minecraft","version":"1.21.1"},{"uid":"net.fabricmc.fabric-loader","version":"0.16.9"}]}"#;
        assert_eq!(parse_mmc_pack(fabric), Some(("1.21.1".into(), "fabric".into(), "0.16.9".into())));
        assert_eq!(parse_mmc_pack(r#"{"components":[{"uid":"net.minecraft","version":"1.8.9"}]}"#), Some(("1.8.9".into(), String::new(), String::new())));
        assert_eq!(parse_mmc_pack("{}"), None);
        assert_eq!(parse_mmc_pack("not json"), None);
        assert_eq!(parse_instance_cfg_name("InstanceType=OneSix\nname= My Pack \n"), Some("My Pack".into()));
    }

    #[test]
    fn curseforge_instance_json() {
        let t = r#"{"name":"ATM","gameVersion":"1.20.1","baseModLoader":{"name":"forge-47.2.0"}}"#;
        assert_eq!(parse_curseforge(t), Some(("ATM".into(), "1.20.1".into(), "forge".into(), "1.20.1-47.2.0".into())));
        let t = r#"{"name":"F","gameVersion":"1.20.1","baseModLoader":{"name":"fabric-0.15.7-1.20.1"}}"#;
        assert_eq!(parse_curseforge(t).unwrap().3, "0.15.7");
        let t = r#"{"name":"V","gameVersion":"1.19"}"#;
        assert_eq!(parse_curseforge(t).unwrap().2, "");
    }

    #[test]
    fn other_launchers_are_found() {
        let d = tmp("scan");
        let mmc = d.join("prism");
        std::fs::create_dir_all(mmc.join("Good/.minecraft/mods")).unwrap();
        std::fs::write(mmc.join("Good/mmc-pack.json"), PACK).unwrap();
        std::fs::write(mmc.join("Good/instance.cfg"), "name=Good Pack\n").unwrap();
        std::fs::create_dir_all(mmc.join("NoPack")).unwrap();
        std::fs::create_dir_all(mmc.join("_LAUNCHER_TEMP")).unwrap();
        std::fs::write(mmc.join("_LAUNCHER_TEMP/mmc-pack.json"), PACK).unwrap();
        let plain = d.join("modrinth");
        std::fs::create_dir_all(plain.join("Prof/mods")).unwrap();
        std::fs::create_dir_all(plain.join("Empty")).unwrap();

        let found = scan(&[("Prism Launcher", mmc, Kind::Mmc), ("Modrinth App", plain, Kind::Plain)]);
        let names: Vec<_> = found.iter().map(|f| (f.source.as_str(), f.name.as_str(), f.is_complete())).collect();
        assert_eq!(names, vec![("Prism Launcher", "Good Pack", true), ("Modrinth App", "Prof", false)]);
        assert_eq!(found[0].loader, "forge");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_user_data_is_copied() {
        let d = tmp("copy");
        let game = d.join("src/.minecraft");
        for p in ["mods", "config", "saves/World", "logs", "versions/1.20.1"] {
            std::fs::create_dir_all(game.join(p)).unwrap();
        }
        std::fs::write(game.join("mods/a.jar"), b"jar").unwrap();
        std::fs::write(game.join("saves/World/level.dat"), b"lvl").unwrap();
        std::fs::write(game.join("logs/latest.log"), b"log").unwrap();
        std::fs::write(game.join("options.txt"), b"fov:90").unwrap();
        std::fs::write(game.join("launcher_accounts.json"), b"secret").unwrap();
        assert_eq!(game_dir(&d.join("src")), game);

        let without = d.join("a");
        copy_game_files(&game, &without, false).unwrap();
        assert!(without.join("mods/a.jar").is_file() && without.join("options.txt").is_file());
        assert!(!without.join("saves").exists(), "worlds are optional");
        assert!(!without.join("logs").exists() && !without.join("versions").exists());
        assert!(!without.join("launcher_accounts.json").exists(), "accounts are never copied");

        let with = d.join("b");
        copy_game_files(&game, &with, true).unwrap();
        assert!(with.join("saves/World/level.dat").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn zip_with(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts: zip::write::SimpleFileOptions = Default::default();
        for (n, c) in files {
            z.start_file(*n, opts).unwrap();
            std::io::Write::write_all(&mut z, c.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn archives_are_recognized() {
        let d = tmp("zip");
        let mrpack = d.join("My Pack.mrpack");
        zip_with(&mrpack, &[("modrinth.index.json", r#"{"formatVersion":1,"game":"minecraft","versionId":"1","name":"Cool","files":[],"dependencies":{"minecraft":"1.21.1","fabric-loader":"0.16.9"}}"#)]);
        let info = inspect_archive(&mrpack).unwrap();
        assert_eq!((info.kind, info.name.as_str(), info.game_version.as_str(), info.loader.as_str()), (ArchiveKind::Mrpack, "Cool", "1.21.1", "fabric"));

        // Prism exports with files at the root, and inside one folder
        for prefix in ["", "Pack/"] {
            let z = d.join("prism.zip");
            zip_with(&z, &[(&format!("{prefix}mmc-pack.json"), PACK), (&format!("{prefix}instance.cfg"), "name=From Prism\n")]);
            let info = inspect_archive(&z).unwrap();
            assert_eq!((info.kind, info.name.as_str(), info.loader_version.as_str()), (ArchiveKind::Mmc, "From Prism", "1.20.1-47.2.0"));
        }

        let cf = d.join("cf.zip");
        zip_with(&cf, &[("manifest.json", "{}"), ("overrides/options.txt", "")]);
        assert!(inspect_archive(&cf).unwrap_err().contains("CurseForge"));
        let junk = d.join("junk.zip");
        zip_with(&junk, &[("readme.txt", "hi")]);
        assert!(inspect_archive(&junk).is_err());
        std::fs::write(d.join("no.zip"), b"nope").unwrap();
        assert!(inspect_archive(&d.join("no.zip")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unsafe_archives_are_refused() {
        let d = tmp("evil");
        let z = d.join("evil.zip");
        zip_with(&z, &[("../escape.txt", "x")]);
        assert!(safe_extract(&z, &d.join("out")).is_err());
        assert!(!d.join("escape.txt").exists());
        zip_with(&z, &[("ok/a.txt", "x")]);
        safe_extract(&z, &d.join("out")).unwrap();
        assert!(d.join("out/ok/a.txt").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }
}
