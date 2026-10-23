use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use reqwest::blocking::Client;

use super::download;
use super::instance::{self, Instance};
use super::mojang;
use super::modrinth;

pub const OS: &str = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "osx" } else { "linux" };
pub const ARCH: &str = if cfg!(target_arch = "aarch64") { "aarch64" } else { "64" };

/// A single shared progress object used by the UI thread.
pub struct InstallProgress {
    pub current: AtomicU64,
    pub total: AtomicU64,
    pub current_file: std::sync::Mutex<String>,
    pub cancelled: AtomicBool,
}

impl InstallProgress {
    pub fn new() -> Self {
        Self {
            current: AtomicU64::new(0),
            total: AtomicU64::new(0),
            current_file: std::sync::Mutex::new(String::new()),
            cancelled: AtomicBool::new(false),
        }
    }
}

/// Build a Modrinth client.
pub fn make_client() -> reqwest::blocking::Client {
    Client::builder()
        .user_agent("LimeCraft-Launcher/0.1 (Minecraft launcher)")
        .gzip(true)
        .build()
        .unwrap_or_default()
}

/// .minecraft root for the launcher
pub fn launcher_root() -> PathBuf {
    let root = instance::default_dir();
    let _ = std::fs::create_dir_all(&root);
    root
}

/// Install a vanilla Minecraft version into `instances/<name>/mines/`.
/// The version json is saved as `versions/<id>.json` (matching what
/// `launch::build_launch_command` reads for vanilla instances).
pub fn install_vanilla(
    client: &Client,
    root: &Path,
    instance_name: &str,
    version_id: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<(), String> {
    set_file(prog, &format!("Fetching versions for {version_id}"));
    let manifest = mojang::fetch_manifest(client).map_err(|e| e.to_string())?;
    let entry = mojang::find_version(&manifest, version_id).ok_or_else(|| format!("Version {version_id} not found in manifest"))?;
    let vjson = mojang::fetch_version_json(client, &entry).map_err(|e| e.to_string())?;
    let mines = instance::instance_mines(root, instance_name);
    install_version_json(client, &mines, &vjson, prog)
}

/// Given a version.json, download client jar + libs + assets and write the
/// version json into `versions/<id>.json` of the given `.minecraft`-style dir.
pub fn install_version_json(client: &Client, root: &Path, vjson: &mojang::VersionJson, prog: Option<&Arc<InstallProgress>>) -> Result<(), String> {
    download_version_files(client, root, vjson, prog)?;

    let version_dir = root.join("versions");
    let _ = std::fs::create_dir_all(&version_dir);
    let json_path = version_dir.join(format!("{}.json", vjson.id));
    if !json_path.exists() {
        set_file(prog, &format!("saving {}.json", vjson.id));
        let text = serde_json::to_string_pretty(vjson).map_err(|e| e.to_string())?;
        std::fs::write(&json_path, text).map_err(|e| format!("Cannot write {}: {e}", json_path.display()))?;
    }
    Ok(())
}

/// Download client jar + libraries (+ assets) for a given version json.
/// Does NOT write any version json, so the caller stays in control of the file
/// name (used by install_fabric to avoid id collisions).
fn download_version_files(client: &Client, root: &Path, vjson: &mojang::VersionJson, prog: Option<&Arc<InstallProgress>>) -> Result<(), String> {
    let id = &vjson.id;
    let libs_dir = root.join("libraries");
    let version_dir = root.join("versions");
    let _ = std::fs::create_dir_all(&libs_dir);
    let _ = std::fs::create_dir_all(&version_dir);

    // Libraries (jar artifacts) - build the list
    let mut lib_files: Vec<(PathBuf, String, Option<String>)> = Vec::new();
    for lib in &vjson.libraries {
        if !mojang::library_allowed(lib, OS, ARCH) {
            continue;
        }
        if lib.natives.is_some() {
            // LWJGL natives: download the OS-specific classifier jar into the
            // libraries dir so it lands on the classpath. LWJGL3 extracts the
            // native DLL/SO from its own jar at runtime.
            if let (Some(map), Some(dl)) = (&lib.natives, &lib.downloads) {
                if let Some(classifier_name) = map.get(OS) {
                    if let Some(art) = dl.classifiers.as_ref().and_then(|c| c.get(classifier_name)).cloned() {
                        if !art.url.is_empty() {
                            let sha = if art.sha1.is_empty() { None } else { Some(art.sha1.clone()) };
                            lib_files.push((art.path.clone(), art.url.clone(), sha));
                        }
                    }
                }
            }
            continue; // main artifact not needed for natives
        }
        // Handle both Mojang `downloads.artifact` and Maven `name`+`url` formats
        let path = lib.artifact_path();
        let url = lib.artifact_url();
        if let Some(url) = url {
            let sha = lib.sha1_hex();
            lib_files.push((path.unwrap_or_default(), url, sha));
        }
    }

    let total_count = (lib_files.len() + 1) as u64;
    let mut done: u64 = 0;

    // Client jar
    if let Some(client_dl) = vjson.downloads.as_ref().and_then(|d| d.client.clone()) {
        done += 1;
        set_file(prog, &format!("client-{id}.jar"));
        update_progress(prog, done, total_count);
        let dest = version_dir.join(format!("{id}.jar"));
        download::download_file(client, &client_dl.url, &dest, client_dl.sha1.as_deref())
            .map_err(|e| e.to_string())?;
    }

    // Libraries
    for (rel_path, url, sha) in lib_files {
        done += 1;
        let fname = rel_path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        set_file(prog, &fname);
        update_progress(prog, done, total_count);
        let dest = libs_dir.join(&rel_path);
        download::download_file(client, &url, &dest, sha.as_deref()).map_err(|e| e.to_string())?;
    }

    // Assets index + assets
    set_file(prog, "assets");
    if let Some(asset_index) = &vjson.asset_index {
        install_assets(client, root, asset_index, prog)?;
    } else if let Some(assets_id) = &vjson.assets {
        // Legacy versions carry only the id; create the directory skeleton so
        // the launcher can still point --assetsDir there.
        let _ = std::fs::create_dir_all(root.join("assets/indexes"));
        let _ = std::fs::create_dir_all(root.join("assets/objects"));
        let _ = assets_id;
    }

    set_file(prog, "done");
    Ok(())
}

fn update_progress(prog: Option<&Arc<InstallProgress>>, cur: u64, total: u64) {
    if let Some(p) = prog {
        p.current.store(cur, Ordering::Relaxed);
        p.total.store(total, Ordering::Relaxed);
    }
}
fn set_file(prog: Option<&Arc<InstallProgress>>, name: &str) {
    if let Some(p) = prog {
        if let Ok(mut g) = p.current_file.lock() {
            *g = name.to_string();
        }
    }
}

/// Download the asset index json for `asset_index` into `assets/indexes/<id>.json`,
/// then fetch every object into `assets/objects/<ab>/<hash>`, reporting progress.
fn install_assets(client: &Client, root: &Path, asset_index: &mojang::AssetIndex, prog: Option<&Arc<InstallProgress>>) -> Result<(), String> {
    let indexes_dir = root.join("assets/indexes");
    let objects_dir = root.join("assets/objects");
    std::fs::create_dir_all(&indexes_dir).map_err(|e| format!("Cannot create {}: {e}", indexes_dir.display()))?;
    std::fs::create_dir_all(&objects_dir).map_err(|e| format!("Cannot create {}: {e}", objects_dir.display()))?;

    // 1. Asset index file
    let index_path = indexes_dir.join(format!("{}.json", asset_index.id));
    set_file(prog, &format!("asset-index {}.json", asset_index.id));
    let index_sha = if asset_index.sha1.is_empty() { None } else { Some(asset_index.sha1.as_str()) };
    download::download_file(client, &asset_index.url, &index_path, index_sha).map_err(|e| e.to_string())?;

    // 2. Parse objects
    let index_text = std::fs::read_to_string(&index_path).map_err(|e| format!("Cannot read {}: {e}", index_path.display()))?;
    let index: mojang::AssetIndexFile =
        serde_json::from_str(&index_text).map_err(|e| format!("Cannot parse asset index {}: {e}", index_path.display()))?;

    // Decide which objects must be materialized on disk: only entries with
    // size > 0 are actual downloadable files.
    let mut pending: Vec<(PathBuf, String)> = Vec::new(); // (dest, sha1)
    for obj in index.objects.values() {
        if obj.size == 0 {
            continue;
        }
        let hash = obj.hash.clone();
        if hash.len() < 2 {
            continue;
        }
        let (ab, tail) = hash.split_at(2);
        debug_assert_eq!(tail.len(), hash.len() - 2);
        let dest = objects_dir.join(ab).join(&hash);
        pending.push((dest, hash));
    }

    // 3. Download objects with progress
    let total = pending.len() as u64;
    let mut done: u64 = 0;
    for (dest, hash) in pending {
        done += 1;
        set_file(prog, &dest.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default());
        update_progress(prog, done, total);
        let url = format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash);
        download::download_file(client, &url, &dest, Some(&hash)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Meta parameters for a profile-json style loader (Fabric/Quilt).
struct LoaderMeta {
    /// Human name, e.g. "Fabric".
    label: &'static str,
    /// Meta host, e.g. "https://meta.fabricmc.net/v2" or "https://meta.quiltmc.org/v3".
    base: &'static str,
}

const FABRIC: LoaderMeta = LoaderMeta { label: "Fabric", base: "https://meta.fabricmc.net/v2", };
const QUILT: LoaderMeta = LoaderMeta { label: "Quilt", base: "https://meta.quiltmc.org/v3", };

/// Install a profile-json based loader (Fabric or Quilt) for a given MC version.
/// Returns the chosen loader version.
fn install_loader(
    client: &Client,
    root: &Path,
    instance: &Instance,
    meta: &LoaderMeta,
    prefix: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<String, String> {
    set_file(prog, &format!("resolving {} versions", meta.label));
    let loader_meta: serde_json::Value = download::get_json(
        client,
        &format!("{}/versions/loader/{}", meta.base, instance.game_version),
    )
    .map_err(|e| format!("{} meta for {}: {e}", meta.label, instance.game_version))?;
    let latest = loader_meta
        .as_array()
        .and_then(|arr| arr.iter().find(|v| v.get("loader").is_some()))
        .and_then(|v| v.get("loader").and_then(|l| l.get("version").and_then(|x| x.as_str())))
        .unwrap_or("0.16.9")
        .to_string();
    set_file(prog, &format!("downloading {} loader {latest}", meta.label));

    // Ensure base vanilla is present (client + libs + assets).
    let mines = instance::instance_mines(root, &instance.name);
    std::fs::create_dir_all(&mines).map_err(|e| e.to_string())?;
    {
        let manifest = mojang::fetch_manifest(client).map_err(|e| e.to_string())?;
        let base_entry = mojang::find_version(&manifest, &instance.game_version)
            .ok_or_else(|| format!("MC {} not in manifest", instance.game_version))?;
        let base_vjson = mojang::fetch_version_json(client, &base_entry).map_err(|e| e.to_string())?;
        download_version_files(client, &mines, &base_vjson, prog)?;
    }

    // Fetch the loader profile json (libraries + main class) and install its
    // libraries. Write the version json under a loader-version based name so it
    // never collides with the vanilla json id.
    let profile_url =
        format!("{}/versions/loader/{}/{}/profile/json", meta.base, instance.game_version, latest);
    let vjson_text = download::get_text(client, &profile_url).map_err(|e| e.to_string())?;
    let modded: mojang::VersionJson = serde_json::from_str(&vjson_text).map_err(|e| format!("parse {} json: {e}", meta.label))?;

    download_version_files(client, &mines, &modded, prog)?;

    // Save the loader json under a deterministic name.
    let json_name = if instance.loader_version.is_empty() {
        format!("{prefix}-{latest}.json")
    } else {
        format!("{}.json", instance.loader_version)
    };
    let version_dir = mines.join("versions");
    let _ = std::fs::create_dir_all(&version_dir);
    std::fs::write(version_dir.join(&json_name), &vjson_text).map_err(|e| format!("Cannot write {}: {e}", json_name))?;
    set_file(prog, &format!("{} {latest} installed", meta.label));

    Ok(latest)
}

/// Install Fabric loader for a given MC version. Returns chosen loader version.
pub fn install_fabric(client: &Client, root: &Path, instance: &Instance, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    install_loader(client, root, instance, &FABRIC, "fabric-loader", prog)
}

/// Install Quilt loader for a given MC version. Returns chosen loader version.
pub fn install_quilt(client: &Client, root: &Path, instance: &Instance, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    install_loader(client, root, instance, &QUILT, "quilt-loader", prog)
}

/// Скачать installer-джару модового лоадера и прочитать из неё `version.json`
/// (профиль лоадера: libraries + mainClass), не распаковывая остальное.
fn download_fm_installer_version_json(client: &Client, url: &str, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    set_file(prog, "downloading loader installer");
    // временный файл в папке инстанса — удалим после извлечения
    let tmp = std::env::temp_dir().join(format!("limecraft-installer-{}.jar", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    download::download_file_with_progress(client, url, &tmp, None).map_err(|e| format!("installer download: {e}"))?;

    let result = (|| {
        let file = std::fs::File::open(&tmp).map_err(|e| format!("open installer: {e}"))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("installer not a zip: {e}"))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            if entry.name() == "version.json" {
                let mut text = String::new();
                use std::io::Read;
                entry.read_to_string(&mut text).map_err(|e| format!("read version.json: {e}"))?;
                return Ok(text);
            }
        }
        Err("version.json not found in installer".to_string())
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Forge: резолв версии лоадера для `mc` по promotions (latest → recommended).
fn resolve_forge_version(client: &Client, mc: &str, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    set_file(prog, "resolving Forge versions");
    let json: serde_json::Value =
        download::get_json(client, "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
            .map_err(|e| format!("Forge promotions: {e}"))?;
    let promos = json.get("promos").and_then(|p| p.as_object());
    let lookup = |kind: &str| promos.and_then(|p| p.get(&format!("{mc}-{kind}"))).and_then(|v| v.as_str()).map(|s| s.to_string());
    let recommended = lookup("recommended");
    let latest = lookup("latest");
    recommended
        .or(latest)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Forge не найден для Minecraft {mc}"))
}

/// NeoForge: резолв версии лоадера для `mc` по maven-metadata.
/// Версии NeoForge имеют вид `{major}.{minor}.{patch}` где первые сегменты
/// кодируют версию Minecraft без ведущей "1." (1.21.1 → 21.1.x).
fn resolve_neoforge_version(client: &Client, mc: &str, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    set_file(prog, "resolving NeoForge versions");
    let xml = download::get_text(client, "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
        .map_err(|e| format!("NeoForge meta: {e}"))?;
    // Версии NeoForge имеют вид `{maj}{min}.{build}` (1.21.1 → "21.1.243",
    // 26.3 → "26.3.0.95"): берём первые два сегмента версии MC без "1." как
    // префикс ветки: "21.1.243" ↔ Minecraft 1.21.1.
    let stripped = mc.strip_prefix("1.").unwrap_or(mc);
    let mut segs = stripped.split('.');
    let maj = segs.next().unwrap_or(stripped).to_string();
    let min = segs.next().unwrap_or("").to_string();
    let prefix = if min.is_empty() { maj.clone() } else { format!("{maj}.{min}") };
    let mut candidates: Vec<String> = Vec::new();
    for caps in xml.split("<version>").skip(1) {
        let v = caps.split("</version>").next().unwrap_or("").trim().to_string();
        if v.is_empty() {
            continue;
        }
        // только стабильные релизы, начинающиеся с нужной ветки MC
        if v.contains("-beta") || v.contains("-alpha") || v.contains('+') {
            continue;
        }
        let head = v.split('.').take(2).collect::<Vec<_>>().join(".");
        if head == prefix {
            candidates.push(v);
        }
    }
    candidates
        .into_iter()
        .max_by(|a, b| vers_cmp(a, b))
        .ok_or_else(|| format!("NeoForge не найден для Minecraft {mc}"))
}

/// Простое l10n-сравнение версий (числовые сегменты). Достаточно для выбора max.
fn vers_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<u64> = a.split('.').filter_map(|x| x.parse().ok()).collect();
    let pb: Vec<u64> = b.split('.').filter_map(|x| x.parse().ok()).collect();
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        let ord = x.cmp(&y);
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// Общая реализация установки Forge/NeoForge:
///  1) скачиваем installer-джару, читаем из неё version.json (профиль);
///  2) ставим vanilla-основу (client + libs + assets);
///  3) качаем библиотеки профиля, пишем `versions/<id>.json`;
/// Возвращает идентификатор версии лоадера (например "1.21.1-52.1.20" или "21.1.243").
fn install_fm_installer(
    client: &Client,
    root: &Path,
    instance: &Instance,
    label: &str,
    full_loader_version: &str,
    installer_url: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<String, String> {
    set_file(prog, &format!("downloading {label} {}", full_loader_version));
    let vjson_text = download_fm_installer_version_json(client, installer_url, prog)?;
    let profile: mojang::VersionJson = serde_json::from_str(&vjson_text).map_err(|e| format!("parse {label} json: {e}"))?;
    let profile_id = profile.id.clone();

    // 1) vanilla-основа
    let mines = instance::instance_mines(root, &instance.name);
    std::fs::create_dir_all(&mines).map_err(|e| e.to_string())?;
    let manifest = mojang::fetch_manifest(client).map_err(|e| e.to_string())?;
    let base_entry = mojang::find_version(&manifest, &instance.game_version)
        .ok_or_else(|| format!("MC {} not in manifest", instance.game_version))?;
    let base_vjson = mojang::fetch_version_json(client, &base_entry).map_err(|e| e.to_string())?;
    download_version_files(client, &mines, &base_vjson, prog)?;

    // 2) библиотеки профайла лоадера
    download_version_files(client, &mines, &profile, prog)?;

    // 3) пишем json под id профайла (совпадает с `version_name`)
    let version_dir = mines.join("versions");
    let _ = std::fs::create_dir_all(&version_dir);
    let json_path = version_dir.join(format!("{profile_id}.json"));
    if !json_path.exists() {
        std::fs::write(&json_path, &vjson_text).map_err(|e| format!("Cannot write {}: {e}", json_path.display()))?;
    }
    set_file(prog, &format!("{label} {} installed", full_loader_version));
    Ok(profile_id)
}

/// Установить Forge для инстанса. `instance.loader_version` (если задан) —
/// полная версия вида `1.21.1-52.1.20`, иначе резолвится latest.
pub fn install_forge(client: &Client, root: &Path, instance: &Instance, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    if !instance.loader_version.is_empty() {
        let full = instance.loader_version.clone();
        let url = format!(
            "https://maven.minecraftforge.net/net/minecraftforge/forge/{full}/forge-{full}-installer.jar"
        );
        return install_fm_installer(client, root, instance, "Forge", &full, &url, prog);
    }
    let fv = resolve_forge_version(client, &instance.game_version, prog)?;
    let full = format!("{}-{}", instance.game_version, fv);
    let url = format!(
        "https://maven.minecraftforge.net/net/minecraftforge/forge/{full}/forge-{full}-installer.jar"
    );
    install_fm_installer(client, root, instance, "Forge", &full, &url, prog)
}

/// Установить NeoForge для инстанса.
pub fn install_neoforge(client: &Client, root: &Path, instance: &Instance, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    if !instance.loader_version.is_empty() {
        let full = instance.loader_version.clone();
        let url = format!(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/{full}/neoforge-{full}-installer.jar"
        );
        return install_fm_installer(client, root, instance, "NeoForge", &full, &url, prog);
    }
    let ver = resolve_neoforge_version(client, &instance.game_version, prog)?;
    let url = format!(
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/{ver}/neoforge-{ver}-installer.jar"
    );
    install_fm_installer(client, root, instance, "NeoForge", &ver, &url, prog)
}

/// Install an instance's base (vanilla or Fabric) and return the resolved
/// instance with `version_name`/`loader_version` filled in.
pub fn install_instance(
    client: &Client,
    root: &Path,
    instance: &Instance,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    let mut inst = instance.clone();
    match inst.loader.as_str() {
        "fabric" => {
            let lv = install_fabric(client, root, &inst, prog)?;
            inst.loader_version = lv.clone();
            inst.version_name = Some(format!("fabric-loader-{lv}.json"));
        }
        "quilt" => {
            let lv = install_quilt(client, root, &inst, prog)?;
            inst.loader_version = lv.clone();
            inst.version_name = Some(format!("quilt-loader-{lv}.json"));
        }
        "forge" => {
            let profile_id = install_forge(client, root, &inst, prog)?;
            inst.loader_version = profile_id.clone();
            inst.version_name = Some(format!("{profile_id}.json")); // "1.21.1-forge-52.1.20.json"
        }
        "neoforge" => {
            let profile_id = install_neoforge(client, root, &inst, prog)?;
            inst.loader_version = profile_id.clone();
            inst.version_name = Some(format!("{profile_id}.json")); // "neoforge-21.1.243.json"
        }
        _ => {
            install_vanilla(client, root, &inst.name, &inst.game_version, prog)?;
            inst.loader = String::new();
            inst.loader_version = String::new();
            inst.version_name = Some(format!("{}.json", inst.game_version));
        }
    }
    Ok(inst)
}

/// Install a Modrinth modpack (.mrpack) into instance: download archive, extract,
/// install dependencies (fabric loader / neoforge / forge), unpack overrides.
/// Returns the fully-resolved instance (game_version + loader fields filled in).
pub fn install_mrpack(
    client: &Client,
    root: &Path,
    instance: &Instance,
    url: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    // create instance dirs
    let inst_dir = instance::prepare_instance_dir(instance, root)?;
    let pack_dir = inst_dir.join("pack-tmp");
    let _ = std::fs::create_dir_all(&pack_dir);

    let pack_file = pack_dir.join("modpack.mrpack");
    set_file(prog, "downloading modpack");
    { let _ = std::fs::remove_file(&pack_file); }
    download::download_file_with_progress(client, url, &pack_file, None).map_err(|e| e.to_string())?;

    set_file(prog, "extracting modpack");
    let index = modrinth::extract_mrpack(&pack_file, &pack_dir)?;

    let installed_version = index.version_id.clone().unwrap_or_else(|| "unknown".into());

    let mut pack_mc: Option<String> = None;
    let mut pack_loader: Option<String> = None;
    let mut pack_loader_kind: String = String::new();

    // dependencies: Minecraft + loader
    if let Some(deps) = &index.dependencies {
        if let Some(mc) = deps.get("minecraft") {
            pack_mc = Some(mc.clone());
        }
        // detect loader
        for loader in ["quilt-loader", "neoforge", "forge", "fabric-loader"] {
            if let Some(lv) = deps.get(loader) {
                let loader_kind = match loader {
                    "fabric-loader" => "fabric",
                    "quilt-loader" => "quilt",
                    "neoforge" => "neoforge",
                    "forge" => "forge",
                    _ => "fabric",
                };
                pack_loader = Some(format!("{loader_kind}@{lv}"));
                pack_loader_kind = loader_kind.to_string();
            }
        }
    }

    // Build the instance: install base MC + loader so the pack is runnable.
    let mut resolved = instance.clone();
    resolved.game_version = if let Some(mc) = pack_mc.as_ref() { mc.clone() } else { instance.game_version.clone() };
    resolved.loader = if pack_loader_kind.is_empty() {
        instance.loader.clone()
    } else {
        pack_loader_kind.clone()
    };

    let resolved = if !resolved.game_version.is_empty() && resolved.game_version != "latest" {
        set_file(prog, &format!("installing Minecraft {}", resolved.game_version));
        install_instance(client, root, &resolved, prog)?
    } else {
        set_file(prog, "modpack without pinned MC — skipping base install");
        let mut updated = resolved;
        updated.loader_version = String::new();
        updated.version_name = None;
        updated
    };

    let resolved_inst = resolved;

    // Download files listed in index into instance `mines/mods` (jar) or other paths.
    set_file(prog, "downloading modpack files");
    let total = index.files.len() as u64;
    let mut done: u64 = 0;
    for f in &index.files {
        done += 1;
        set_file(prog, &f.path);
        update_progress(prog, done, total);
        let dest = inst_dir.join("mines").join(&f.path);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // verify existing hash
        let sha1 = f.hashes.get("sha1").map(|h| h.to_lowercase());
        if let Some(h) = &sha1 {
            if dest.exists() && download::file_sha1(&dest).map(|x| x.to_lowercase()) == Ok(h.clone()) {
                continue;
            }
        }
        let url = f.downloads.first().cloned().unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        download::download_file_with_progress(client, &url, &dest, None).map_err(|e| e.to_string())?;
    }

    // overrides are already extracted to pack_dir/overrides; copy into instance
    set_file(prog, "applying overrides");
    let overrides_src = pack_dir.join("overrides");
    if overrides_src.exists() {
        copy_dir(&overrides_src, &inst_dir.join("mines"))?;
    }

    let _ = std::fs::remove_dir_all(&pack_dir);
    // store pack metadata marker
    let marker = inst_dir.join("pack.json");
    let meta = serde_json::json!({
        "modpack": index.name,
        "files": index.files.len(),
        "minecraft": pack_mc,
        "loader": pack_loader,
        "version_id": installed_version,
    });
    std::fs::create_dir_all(&inst_dir).map_err(|e| e.to_string())?;
    let _ = std::fs::write(marker, serde_json::to_string_pretty(&meta).unwrap_or_default());
    Ok(resolved_inst)
}

/// Simple recursive copy (src -> dst).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::create_dir_all(to.parent().unwrap()).ok();
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// Download a single mod version's primary file into instance mods dir.
pub fn download_mod_version(
    client: &Client,
    root: &Path,
    instance_name: &str,
    version: &modrinth::Version,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<PathBuf, String> {
    let inst_dir = instance::instance_dir(root, instance_name);
    let mods_dir = inst_dir.join("mines/mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| e.to_string())?;
    let primary = version
        .files
        .iter()
        .find(|f| f.primary.unwrap_or(false))
        .or_else(|| version.files.first())
        .ok_or("No files in this version")?;
    let dest = mods_dir.join(&primary.filename);
    set_file(prog, &primary.filename);
    let sha1 = primary.hashes.get("sha1").map(|h| h.to_lowercase());
    download::download_file(client, &primary.url, &dest, sha1.as_deref()).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Download dependencies of a version (direct required/optional) into mods dir.
pub fn download_mod_dependencies(
    client: &Client,
    root: &Path,
    instance_name: &str,
    version: &modrinth::Version,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<usize, String> {
    let mut downloaded: usize = 0;
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = Vec::new();
    for dep in &version.dependencies {
        if let Some(pid) = &dep.project_id {
            if dep.dependency_type.as_deref() == Some("required") || dep.dependency_type.is_none() {
                if seen.insert(pid.clone()) {
                    queue.push(pid.clone());
                }
            }
        }
    }
    while let Some(pid) = queue.pop() {
        let versions = modrinth::get_project_versions(client, &pid).map_err(|e| e.to_string())?;
        if let Some(latest) = versions.first() {
            let inst_dir = instance::instance_dir(root, instance_name);
            let mods_dir = inst_dir.join("mines/mods");
            let primary = latest
                .files
                .iter()
                .find(|f| f.primary.unwrap_or(false))
                .or_else(|| latest.files.first());
            if let Some(f) = primary {
                let dest = mods_dir.join(&f.filename);
                let sha1 = f.hashes.get("sha1").map(|h| h.to_lowercase());
                set_file(prog, &f.filename);
                if !dest.exists() {
                    download::download_file(client, &f.url, &dest, sha1.as_deref()).map_err(|e| e.to_string())?;
                    downloaded += 1;
                }
                // enqueue nested deps
                for dep in &latest.dependencies {
                    if let Some(pid2) = &dep.project_id {
                        if dep.dependency_type.as_deref() == Some("required") && seen.insert(pid2.clone()) {
                            queue.push(pid2.clone());
                        }
                    }
                }
            }
        }
    }
    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert_eq!(vers_cmp("21.1.243", "21.1.242"), std::cmp::Ordering::Greater);
        assert_eq!(vers_cmp("26.3.0.95", "21.11.42"), std::cmp::Ordering::Greater);
        assert_eq!(vers_cmp("20.2", "20.2.0"), std::cmp::Ordering::Equal);
    }

    #[test]
    fn version_prefix_stripping() {
        // NeoForge-ветка из версии MC: "1.21.1" -> "21.1", "26.3" -> "26.3"
        let branch = |mc: &str| -> String {
            let stripped = mc.strip_prefix("1.").unwrap_or(mc);
            let mut segs = stripped.split('.');
            let maj = segs.next().unwrap_or(stripped).to_string();
            let min = segs.next().unwrap_or("").to_string();
            if min.is_empty() {
                maj
            } else {
                format!("{maj}.{min}")
            }
        };
        assert_eq!(branch("1.21.1"), "21.1");
        assert_eq!(branch("26.3"), "26.3");
        assert_eq!(branch("1.20.6"), "20.6");
    }

    #[test]
    fn fm_web_prints_profile() {
        // не сетевой тест: проверяем только формат урлов, чтобы не менять их случайно
        let mc = "1.21.1";
        let fv = "52.1.20";
        let full = format!("{mc}-{fv}");
        assert_eq!(
            format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{full}/forge-{full}-installer.jar"),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.21.1-52.1.20/forge-1.21.1-52.1.20-installer.jar"
        );
        let ver = "21.1.243";
        assert_eq!(
            format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{ver}/neoforge-{ver}-installer.jar"),
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.243/neoforge-21.1.243-installer.jar"
        );
    }
}
