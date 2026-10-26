use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use reqwest::blocking::Client;

use super::download;
use super::instance::{self, Instance};
use super::launch;
use super::modrinth;
use super::mojang;

pub const OS: &str = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "osx" } else { "linux" };
pub const ARCH: &str = if cfg!(target_arch = "aarch64") { "aarch64" } else { "64" };

/// Parallel download workers for assets.
const ASSET_WORKERS: usize = 8;

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

/// Cancellation flag of an optional progress object.
fn cancel_flag(prog: Option<&Arc<InstallProgress>>) -> Option<&AtomicBool> {
    prog.map(|p| &p.cancelled)
}

fn check_cancel(prog: Option<&Arc<InstallProgress>>) -> Result<(), String> {
    if cancel_flag(prog).map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
        Err("Cancelled".to_string())
    } else {
        Ok(())
    }
}

/// Build the shared HTTP client (with timeouts: a hung server must not hang
/// the install forever).
pub fn make_client() -> reqwest::blocking::Client {
    Client::builder()
        .user_agent("LimeCraft-Launcher/0.1 (Minecraft launcher)")
        .gzip(true)
        .connect_timeout(std::time::Duration::from_secs(8))
        // long: whole-body limit for big downloads; API calls set their own 25 s limit
        .timeout(std::time::Duration::from_secs(1800))
        .build()
        .unwrap_or_default()
}

/// .minecraft root for the launcher.
///
/// Возвращает путь в пользовательской директории (`dirs::data_dir()`), создаёт
/// её и явно сообщает об ошибке, если прав на запись нет.
pub fn launcher_root() -> Result<PathBuf, String> {
    instance::resolve_root()
}

/// Install a vanilla Minecraft version into `instances/<name>/mines/`.
pub fn install_vanilla(
    client: &Client,
    root: &Path,
    instance_name: &str,
    version_id: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<(), String> {
    let mines = instance::instance_mines(root, instance_name);
    install_base(client, &mines, version_id, prog).map(|_| ())
}

/// Install vanilla `version_id` (client jar, libraries, assets) and store the
/// ORIGINAL Mojang json as `versions/<id>.json` — loader profiles reference it
/// through `inheritsFrom`. Returns the parsed json.
fn install_base(
    client: &Client,
    mines: &Path,
    version_id: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<mojang::VersionJson, String> {
    set_file(prog, &format!("Fetching versions for {version_id}"));
    let manifest = mojang::fetch_manifest(client)?;
    let entry = mojang::find_version(&manifest, version_id)
        .ok_or_else(|| format!("Version {version_id} not found in manifest"))?;
    let (vjson, text) = mojang::fetch_version_json_text(client, &entry)?;
    download_version_files(client, mines, &vjson, prog)?;
    write_version_json(mines, &vjson.id, &text)?;
    Ok(vjson)
}

/// Write `versions/<id>.json` atomically.
fn write_version_json(mines: &Path, id: &str, text: &str) -> Result<(), String> {
    let version_dir = mines.join("versions");
    std::fs::create_dir_all(&version_dir).map_err(|e| format!("Cannot create {}: {e}", version_dir.display()))?;
    let path = instance::safe_join(&version_dir, &format!("{id}.json"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

/// Download client jar + libraries (+ assets) for a given version json.
/// Does NOT write any version json, so the caller stays in control of the file
/// name.
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
        if let Some(map) = &lib.natives {
            // Legacy natives: download the OS classifier jar; it is unpacked
            // into the natives dir at launch time.
            if let (Some(classifier), Some(dl)) = (map.get(OS), &lib.downloads) {
                let classifier = classifier.replace("${arch}", "64");
                if let Some(art) = dl.classifiers.as_ref().and_then(|c| c.get(&classifier)) {
                    if !art.url.is_empty() {
                        let sha = Some(art.sha1.clone()).filter(|s| !s.is_empty());
                        lib_files.push((art.path.clone(), art.url.clone(), sha));
                    }
                }
            }
            continue; // main artifact not needed for natives
        }
        // Handle both Mojang `downloads.artifact` and Maven `name`+`url` formats.
        // Libraries without a URL (e.g. produced by the Forge installer) are
        // not downloadable here and are skipped.
        if let (Some(path), Some(url)) = (lib.artifact_path(), lib.artifact_url()) {
            lib_files.push((path, url, lib.sha1_hex()));
        }
    }

    let total_count = (lib_files.len() + 1) as u64;
    let mut done: u64 = 0;

    // Client jar
    if let Some(client_dl) = vjson.downloads.as_ref().and_then(|d| d.client.clone()) {
        done += 1;
        set_file(prog, &format!("client-{id}.jar"));
        update_progress(prog, done, total_count);
        let dest = instance::safe_join(&version_dir, &format!("{id}.jar"))?;
        download::download_file_ex(client, &client_dl.url, &dest, client_dl.sha1.as_deref(), cancel_flag(prog), None)?;
    }

    // Libraries
    for (rel_path, url, sha) in lib_files {
        check_cancel(prog)?;
        done += 1;
        let fname = rel_path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        set_file(prog, &fname);
        update_progress(prog, done, total_count);
        let dest = instance::safe_join(&libs_dir, &rel_path.to_string_lossy())?;
        download::download_file_ex(client, &url, &dest, sha.as_deref(), cancel_flag(prog), None)?;
    }

    // Assets index + assets
    set_file(prog, "assets");
    if let Some(asset_index) = &vjson.asset_index {
        install_assets(client, root, asset_index, prog)?;
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
/// then fetch every object into `assets/objects/<ab>/<hash>` using a small
/// worker pool, reporting progress.
fn install_assets(client: &Client, root: &Path, asset_index: &mojang::AssetIndex, prog: Option<&Arc<InstallProgress>>) -> Result<(), String> {
    let indexes_dir = root.join("assets/indexes");
    let objects_dir = root.join("assets/objects");
    std::fs::create_dir_all(&indexes_dir).map_err(|e| format!("Cannot create {}: {e}", indexes_dir.display()))?;
    std::fs::create_dir_all(&objects_dir).map_err(|e| format!("Cannot create {}: {e}", objects_dir.display()))?;

    // 1. Asset index file
    let index_path = instance::safe_join(&indexes_dir, &format!("{}.json", asset_index.id))?;
    set_file(prog, &format!("asset-index {}.json", asset_index.id));
    let index_sha = if asset_index.sha1.is_empty() { None } else { Some(asset_index.sha1.as_str()) };
    download::download_file_ex(client, &asset_index.url, &index_path, index_sha, cancel_flag(prog), None)?;

    // 2. Parse objects
    let index_text = std::fs::read_to_string(&index_path).map_err(|e| format!("Cannot read {}: {e}", index_path.display()))?;
    let index: mojang::AssetIndexFile =
        serde_json::from_str(&index_text).map_err(|e| format!("Cannot parse asset index {}: {e}", index_path.display()))?;

    // Only entries with size > 0 are actual downloadable files.
    let mut pending: Vec<(PathBuf, String)> = Vec::new(); // (dest, sha1)
    for obj in index.objects.values() {
        let hash = obj.hash.to_lowercase();
        if obj.size == 0 || hash.len() < 2 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        pending.push((objects_dir.join(&hash[..2]).join(&hash), hash));
    }

    // 3. Download objects in parallel
    set_file(prog, "assets");
    let total = pending.len() as u64;
    update_progress(prog, 0, total);
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let failure: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..ASSET_WORKERS {
            s.spawn(|| loop {
                if failure.lock().map(|f| f.is_some()).unwrap_or(true) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((dest, hash)) = pending.get(i) else { break };
                let url = format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash);
                match download::download_file_ex(client, &url, dest, Some(hash), cancel_flag(prog), None) {
                    Ok(()) => update_progress(prog, done.fetch_add(1, Ordering::Relaxed) + 1, total),
                    Err(e) => {
                        if let Ok(mut f) = failure.lock() {
                            f.get_or_insert(e);
                        }
                        break;
                    }
                }
            });
        }
    });
    match failure.into_inner() {
        Ok(Some(e)) => Err(e),
        _ => Ok(()),
    }
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
/// A pinned `instance.loader_version` (e.g. from a modpack) is honoured,
/// otherwise the newest loader is used. Returns the loader version.
fn install_loader(
    client: &Client,
    root: &Path,
    instance: &Instance,
    meta: &LoaderMeta,
    prefix: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<String, String> {
    let latest = if !instance.loader_version.is_empty() {
        instance.loader_version.clone()
    } else {
        set_file(prog, &format!("resolving {} versions", meta.label));
        let loader_meta: serde_json::Value = download::get_json(
            client,
            &format!("{}/versions/loader/{}", meta.base, instance.game_version),
        )
        .map_err(|e| format!("{} meta for {}: {e}", meta.label, instance.game_version))?;
        loader_meta
            .as_array()
            .and_then(|arr| arr.iter().find(|v| v.get("loader").is_some()))
            .and_then(|v| v.get("loader").and_then(|l| l.get("version").and_then(|x| x.as_str())))
            .ok_or_else(|| format!("{} не поддерживает Minecraft {}", meta.label, instance.game_version))?
            .to_string()
    };
    set_file(prog, &format!("downloading {} loader {latest}", meta.label));

    // Vanilla base (client + libs + assets + versions/<mc>.json for inheritsFrom).
    let mines = instance::instance_mines(root, &instance.name);
    std::fs::create_dir_all(&mines).map_err(|e| e.to_string())?;
    install_base(client, &mines, &instance.game_version, prog)?;

    // Loader profile json (libraries + main class + inheritsFrom).
    let profile_url =
        format!("{}/versions/loader/{}/{}/profile/json", meta.base, instance.game_version, latest);
    let vjson_text = download::get_text(client, &profile_url)?;
    let modded: mojang::VersionJson = serde_json::from_str(&vjson_text).map_err(|e| format!("parse {} json: {e}", meta.label))?;
    download_version_files(client, &mines, &modded, prog)?;

    // Same name as `install_instance` records in `version_name`.
    let json_name = format!("{prefix}-{latest}");
    write_version_json(&mines, &json_name, &vjson_text)?;
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

/// Скачать installer-джару модового лоадера в `dest` и прочитать из неё
/// `version.json` (профиль лоадера: id + libraries + mainClass).
fn read_installer_profile(jar: &Path) -> Result<String, String> {
    let file = std::fs::File::open(jar).map_err(|e| format!("open installer: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("installer not a zip: {e}"))?;
    let mut entry = archive
        .by_name("version.json")
        .map_err(|_| "version.json not found in installer".to_string())?;
    let mut text = String::new();
    use std::io::Read;
    entry.read_to_string(&mut text).map_err(|e| format!("read version.json: {e}"))?;
    Ok(text)
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

/// Общая реализация установки Forge/NeoForge: скачиваем installer и запускаем
/// его (`--installClient`), чтобы он выполнил процессоры (патч клиента,
/// генерация mappings и т.д.). Возвращает id профиля лоадера.
fn install_fm_installer(
    client: &Client,
    root: &Path,
    instance: &Instance,
    label: &str,
    full_loader_version: &str,
    installer_url: &str,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<String, String> {
    set_file(prog, &format!("downloading {label} {full_loader_version}"));
    let mines = instance::instance_mines(root, &instance.name);
    std::fs::create_dir_all(&mines).map_err(|e| format!("Cannot create {}: {e}", mines.display()))?;

    // Installer лежит в папке инстанса (там точно есть права записи).
    let tmp_root = mines.join(".tmp");
    std::fs::create_dir_all(&tmp_root).map_err(|e| format!("Cannot create {}: {e}", tmp_root.display()))?;
    let installer = tmp_root.join(format!("installer-{}.jar", std::process::id()));
    let result = (|| -> Result<String, String> {
        // the installer runs as Java code on this computer: never run an unverified one
        let sha1 = download::maven_sha1(client, installer_url)
            .map_err(|e| format!("{label}: не удалось получить контрольную сумму установщика ({e}) — установка остановлена"))?;
        download::download_file_ex(client, installer_url, &installer, Some(&sha1), cancel_flag(prog), None)
            .map_err(|e| format!("installer download: {e}"))?;
        let profile_text = read_installer_profile(&installer)?;
        let profile: mojang::VersionJson =
            serde_json::from_str(&profile_text).map_err(|e| format!("parse {label} json: {e}"))?;
        let profile_id = profile.id.clone();

        // vanilla-основа (клиент, библиотеки, ассеты, versions/<mc>.json)
        let base = install_base(client, &mines, &instance.game_version, prog)?;

        // Installer требует launcher_profiles.json в целевой папке.
        let lp = mines.join("launcher_profiles.json");
        if !lp.exists() {
            std::fs::write(&lp, r#"{"profiles":{},"version":3}"#).map_err(|e| format!("write launcher_profiles.json: {e}"))?;
        }

        set_file(prog, &format!("running {label} installer (this takes a while)"));
        let required = base.java_version.as_ref().map(|j| j.major_version);
        let java = ensure_java(client, root, required.unwrap_or(17), prog)?;
        run_installer(&java, &installer, &mines, prog)?;

        // Installer пишет versions/<id>/<id>.json; подтягиваем недостающие библиотеки.
        let nested = mines.join("versions").join(&profile_id).join(format!("{profile_id}.json"));
        let installed: mojang::VersionJson = match std::fs::read_to_string(&nested) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("parse installed {label} json: {e}"))?,
            Err(_) => {
                write_version_json(&mines, &profile_id, &profile_text)?;
                profile
            }
        };
        download_version_files(client, &mines, &installed, prog)?;
        set_file(prog, &format!("{label} {full_loader_version} installed"));
        Ok(profile_id)
    })();
    let _ = std::fs::remove_file(&installer);
    let _ = std::fs::remove_dir(&tmp_root);
    result
}

/// `java -jar installer --installClient <dir>` with cancel support; output
/// goes to `installer.log` next to the game files.
fn run_installer(java: &Path, installer: &Path, mines: &Path, prog: Option<&Arc<InstallProgress>>) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let log_path = mines.join("installer.log");
    let log = std::fs::File::create(&log_path).map_err(|e| format!("create {}: {e}", log_path.display()))?;
    let mut cmd = Command::new(java);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no extra console window
    }
    // the system temp folder may be closed to programs: work inside the game folder
    let tmp = installer.parent().unwrap_or(mines).to_path_buf();
    let mut child = cmd
        .env("TMP", &tmp)
        .env("TEMP", &tmp)
        .arg(format!("-Djava.io.tmpdir={}", tmp.display()))
        .arg("-jar")
        .arg(installer)
        .arg("--installClient")
        .arg(mines)
        .current_dir(mines)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("cannot start installer: {e}"))?;
    loop {
        if check_cancel(prog).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(8).collect();
                let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
                return Err(format!("installer завершился с ошибкой ({status}); см. installer.log\n{tail}"));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(250)),
            Err(e) => return Err(format!("installer: {e}")),
        }
    }
}

/// Установить Forge для инстанса. `instance.loader_version` (если задан) —
/// полная версия вида `1.21.1-52.1.20`, иначе резолвится latest.
pub fn install_forge(client: &Client, root: &Path, instance: &Instance, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    let full = if !instance.loader_version.is_empty() {
        instance.loader_version.clone()
    } else {
        let fv = resolve_forge_version(client, &instance.game_version, prog)?;
        format!("{}-{}", instance.game_version, fv)
    };
    let url = format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{full}/forge-{full}-installer.jar");
    install_fm_installer(client, root, instance, "Forge", &full, &url, prog)
}

/// Установить NeoForge для инстанса.
pub fn install_neoforge(client: &Client, root: &Path, instance: &Instance, prog: Option<&Arc<InstallProgress>>) -> Result<String, String> {
    let ver = if !instance.loader_version.is_empty() {
        instance.loader_version.clone()
    } else {
        resolve_neoforge_version(client, &instance.game_version, prog)?
    };
    let url = format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{ver}/neoforge-{ver}-installer.jar");
    install_fm_installer(client, root, instance, "NeoForge", &ver, &url, prog)
}

/// File name (inside `versions/`) of an installed loader profile: the
/// Forge/NeoForge installers use `<id>/<id>.json`, ours are flat.
fn profile_file_name(mines: &Path, id: &str) -> String {
    if mines.join("versions").join(id).join(format!("{id}.json")).exists() {
        format!("{id}/{id}.json")
    } else {
        format!("{id}.json")
    }
}

/// Install an instance's base (vanilla or loader) and return the resolved
/// instance with `version_name`/`loader_version` filled in.
pub fn install_instance(
    client: &Client,
    root: &Path,
    instance: &Instance,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    let mut inst = instance.clone();
    let mines = instance::instance_mines(root, &inst.name);
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
        "forge" | "neoforge" => {
            let profile_id = if inst.loader == "forge" {
                install_forge(client, root, &inst, prog)?
            } else {
                install_neoforge(client, root, &inst, prog)?
            };
            inst.version_name = Some(profile_file_name(&mines, &profile_id));
            // keep the installer-style version (e.g. "1.21.1-52.1.20") for reinstalls
            if inst.loader_version.is_empty() {
                inst.loader_version = profile_id;
            }
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
/// install dependencies (loader), download files (hash-checked), apply overrides.
/// Returns the fully-resolved instance (game_version + loader fields filled in).
pub fn install_mrpack(
    client: &Client,
    root: &Path,
    instance: &Instance,
    pack: &modrinth::MrpackFile,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    install_mrpack_from(client, root, instance, PackSource::Remote(pack), prog)
}

/// Install a `.mrpack` the user already has on disk. The archive itself has no
/// checksum to compare with, but every file it lists is still hash-checked.
pub fn install_mrpack_file(
    client: &Client,
    root: &Path,
    instance: &Instance,
    file: &Path,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    install_mrpack_from(client, root, instance, PackSource::Local(file), prog)
}

enum PackSource<'a> {
    Remote(&'a modrinth::MrpackFile),
    Local(&'a Path),
}

fn install_mrpack_from(
    client: &Client,
    root: &Path,
    instance: &Instance,
    source: PackSource,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    let inst_dir = instance::prepare_instance_dir(instance, root)?;
    let pack_dir = inst_dir.join("pack-tmp");
    let _ = std::fs::remove_dir_all(&pack_dir);
    let _ = std::fs::create_dir_all(&pack_dir);
    let result = install_mrpack_inner(client, root, instance, source, &inst_dir, &pack_dir, prog);
    let _ = std::fs::remove_dir_all(&pack_dir);
    result
}

fn install_mrpack_inner(
    client: &Client,
    root: &Path,
    instance: &Instance,
    source: PackSource,
    inst_dir: &Path,
    pack_dir: &Path,
    prog: Option<&Arc<InstallProgress>>,
) -> Result<Instance, String> {
    let pack_file = pack_dir.join("modpack.mrpack");
    match source {
        PackSource::Remote(pack) => {
            set_file(prog, "downloading modpack");
            if pack.sha1.is_none() && pack.sha512.is_none() {
                return Err("у модпака нет контрольной суммы — установка остановлена".into());
            }
            download::download_file_ex(client, &pack.url, &pack_file, pack.sha1.as_deref(), cancel_flag(prog), None)?;
            if let Some(expected) = &pack.sha512 {
                if &download::file_sha512(&pack_file)? != expected {
                    return Err("модпак: SHA-512 не совпал — файл повреждён или подменён".into());
                }
            }
        }
        PackSource::Local(path) => {
            set_file(prog, "reading modpack");
            std::fs::copy(path, &pack_file).map_err(|e| format!("не удалось прочитать {}: {e}", path.display()))?;
        }
    }

    set_file(prog, "extracting modpack");
    let extract_dir = pack_dir.join("x");
    let index = modrinth::extract_mrpack(&pack_file, &extract_dir)?;
    let installed_version = index.version_id.clone().unwrap_or_else(|| "unknown".into());

    // dependencies: Minecraft + loader (with the pack's pinned loader version)
    let mut pack_mc: Option<String> = None;
    let mut pack_loader: Option<String> = None;
    let mut pack_loader_kind = String::new();
    let mut pack_loader_version = String::new();
    if let Some(deps) = &index.dependencies {
        pack_mc = deps.get("minecraft").cloned();
        for (key, kind) in [("quilt-loader", "quilt"), ("neoforge", "neoforge"), ("forge", "forge"), ("fabric-loader", "fabric")] {
            if let Some(lv) = deps.get(key) {
                pack_loader = Some(format!("{kind}@{lv}"));
                pack_loader_kind = kind.to_string();
                pack_loader_version = lv.clone();
                break;
            }
        }
    }

    let mut resolved = instance.clone();
    if let Some(mc) = &pack_mc {
        resolved.game_version = mc.clone();
    }
    if !pack_loader_kind.is_empty() {
        resolved.loader = pack_loader_kind.clone();
        resolved.loader_version = match pack_loader_kind.as_str() {
            // Forge versions are installed as "<mc>-<forge>"
            "forge" if !pack_loader_version.contains('-') => format!("{}-{}", resolved.game_version, pack_loader_version),
            _ => pack_loader_version.clone(),
        };
    }

    let resolved = if !resolved.game_version.is_empty() && resolved.game_version != "latest" {
        set_file(prog, &format!("installing Minecraft {}", resolved.game_version));
        install_instance(client, root, &resolved, prog)?
    } else {
        let mut updated = resolved;
        updated.loader_version = String::new();
        updated.version_name = None;
        updated
    };

    // Download files listed in the index (client-side only, hash-checked).
    set_file(prog, "downloading modpack files");
    let mines = inst_dir.join("mines");
    let files: Vec<&modrinth::IndexFile> = index.files.iter().filter(|f| f.client_supported()).collect();
    let total = files.len() as u64;
    for (i, f) in files.iter().enumerate() {
        check_cancel(prog)?;
        set_file(prog, &f.path);
        update_progress(prog, i as u64 + 1, total);
        let dest = instance::safe_join(&mines, &f.path)?;
        let sha1 = f.hashes.get("sha1").map(|h| h.to_lowercase());
        let sha512 = f.hashes.get("sha512").map(|h| h.to_lowercase());
        if sha1.is_none() && sha512.is_none() {
            return Err(format!("у файла {} нет хэша — отказ от установки", f.path));
        }
        let url = f
            .downloads
            .iter()
            .find(|u| modrinth::is_allowed_download_url(u))
            .ok_or_else(|| format!("у файла {} нет допустимого источника загрузки", f.path))?;
        download::download_file_ex(client, url, &dest, sha1.as_deref(), cancel_flag(prog), None)?;
        if let Some(expected) = &sha512 {
            let actual = download::file_sha512(&dest)?;
            if &actual != expected {
                let _ = std::fs::remove_file(&dest);
                return Err(format!("sha512 mismatch for {}", f.path));
            }
        }
    }

    // overrides, then client-only overrides on top
    set_file(prog, "applying overrides");
    for dir in ["overrides", "client-overrides"] {
        copy_dir(&extract_dir.join(dir), &mines)?;
    }

    let meta = serde_json::json!({
        "modpack": index.name,
        "files": index.files.len(),
        "minecraft": pack_mc,
        "loader": pack_loader,
        "version_id": installed_version,
    });
    let _ = std::fs::write(inst_dir.join("pack.json"), serde_json::to_string_pretty(&meta).unwrap_or_default());
    Ok(resolved)
}

/// Simple recursive copy (src -> dst).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst).map_err(|e| format!("create dir {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read dir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::{Mock, route};

    #[test]
    fn maven_checksum_is_required_and_parsed() {
        let m = Mock::start();
        let c = Client::new();
        m.set(vec![route("/a.jar.sha1", 200, "FA4F90047C23E6DF4D2B4E649AEC7FD5D1E20ACD  a.jar\n")]);
        assert_eq!(download::maven_sha1(&c, &format!("{}/a.jar", m.url())).unwrap(), "fa4f90047c23e6df4d2b4e649aec7fd5d1e20acd");
        m.set(vec![route("/a.jar.sha1", 200, "<html>not found</html>")]);
        assert!(download::maven_sha1(&c, &format!("{}/a.jar", m.url())).is_err());
        m.set(vec![]);
        assert!(download::maven_sha1(&c, &format!("{}/a.jar", m.url())).is_err());
    }

    /// A tiny "JRE" archive: <top>/bin/java.exe
    fn fake_jre_zip() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::SimpleFileOptions = Default::default();
            z.start_file("jdk-99-jre/bin/java.exe", opts).unwrap();
            std::io::Write::write_all(&mut z, b"MZ fake").unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    #[cfg(all(windows, target_arch = "x86_64"))]
    fn java_download_is_verified_by_sha256() {
        use sha2::{Digest, Sha256};
        let m = Mock::start();
        let zip = fake_jre_zip();
        let good = hex::encode(Sha256::digest(&zip));
        let root = std::env::temp_dir().join(format!("lc-jre-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: only this test reads the variable
        unsafe { std::env::set_var("LIMECRAFT_ADOPTIUM_API", m.url()) };
        let c = Client::new();
        let asset = |sha: &str| format!(r#"[{{"binary":{{"package":{{"link":"{}/jre.zip","checksum":"{sha}","size":1}}}}}}]"#, m.url());

        // a tampered archive is refused and not unpacked
        m.set(vec![route("/assets/latest/99/", 200, asset(&"0".repeat(64))), route("/jre.zip", 200, zip.clone())]);
        let err = ensure_java(&c, &root, 99, None).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(!root.join("runtime/java-99").exists());

        // the genuine one is installed
        m.set(vec![route("/assets/latest/99/", 200, asset(&good)), route("/jre.zip", 200, zip)]);
        let exe = ensure_java(&c, &root, 99, None).unwrap();
        assert!(exe.ends_with("java-99/bin/java.exe") && exe.exists(), "{}", exe.display());
        let _ = std::fs::remove_dir_all(&root);
        unsafe { std::env::remove_var("LIMECRAFT_ADOPTIUM_API") };
    }

    fn jre_tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-jre-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A .tar.gz with the given (path, bytes) files.
    fn make_targz(path: &Path, files: &[(&str, &[u8])]) {
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            // (a raw header so that even "../" names can be written)
            let mut raw = h.clone();
            raw.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            raw.set_cksum();
            tar.append(&raw, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn a_linux_jre_archive_is_unpacked_without_its_top_folder() {
        let d = jre_tmp("ok");
        let ar = d.join("jre.tar.gz");
        make_targz(&ar, &[("jdk-25+9-jre/bin/java", b"ELF-java"), ("jdk-25+9-jre/lib/modules", b"mods")]);
        let out = d.join("out");
        unpack_jre(&ar, &out).unwrap();
        assert_eq!(std::fs::read(out.join("bin/java")).unwrap(), b"ELF-java");
        assert_eq!(std::fs::read(out.join("lib/modules")).unwrap(), b"mods");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(std::fs::metadata(out.join("bin/java")).unwrap().permissions().mode() & 0o100 != 0, "java must be executable");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_jre_archive_cannot_write_outside_its_folder() {
        let d = jre_tmp("evil");
        let ar = d.join("evil.tar.gz");
        make_targz(&ar, &[("top/../../escape.txt", b"x")]);
        let out = d.join("out");
        assert!(unpack_jre(&ar, &out).is_err());
        assert!(!d.join("escape.txt").exists() && !d.parent().unwrap().join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_right_jre_is_asked_for_on_this_system() {
        let m = Mock::start();
        let sha = "ab".repeat(32);
        let reply = |link: &str| format!(r#"[{{"binary":{{"package":{{"link":"{link}","checksum":"{sha}","size":1}}}}}}]"#);
        let c = Client::new();
        m.set(vec![route("/assets/latest/21/", 200, reply("https://x/jre.tar.gz"))]);
        assert!(adoptium_jre_at(&m.url(), &c, 21, ("linux", "x64", ".tar.gz")).is_ok());
        assert!(adoptium_jre_at(&m.url(), &c, 21, ("windows", "x64", ".zip")).unwrap_err().contains("формат"), "a tar.gz is not what Windows wants");
        let asked = m.requests().join(" ");
        assert!(asked.contains("os=linux") && asked.contains("architecture=x64"), "{asked}");
    }

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

fn adoptium_base() -> String {
    std::env::var("LIMECRAFT_ADOPTIUM_API")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://api.adoptium.net/v3".to_string())
}

/// What Adoptium offers for this computer: (os, architecture, archive ending).
fn adoptium_platform() -> Option<(&'static str, &'static str, &'static str)> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some(("windows", "x64", ".zip"))
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(("linux", "x64", ".tar.gz"))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(("linux", "aarch64", ".tar.gz"))
    } else {
        None
    }
}

fn java_file_name() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

/// Download link + SHA-256 of the newest Temurin JRE for this computer.
fn adoptium_jre(client: &Client, major: u32, platform: (&str, &str, &str)) -> Result<(String, String), String> {
    adoptium_jre_at(&adoptium_base(), client, major, platform)
}

fn adoptium_jre_at(api: &str, client: &Client, major: u32, platform: (&str, &str, &str)) -> Result<(String, String), String> {
    let (os, arch, ending) = platform;
    let url = format!("{api}/assets/latest/{major}/hotspot?architecture={arch}&image_type=jre&os={os}&vendor=eclipse");
    let v: serde_json::Value = download::get_json(client, &url)?;
    let pkg = v
        .get(0)
        .and_then(|a| a.get("binary"))
        .and_then(|b| b.get("package"))
        .ok_or_else(|| format!("Adoptium: нет сборки Java {major} для {os} {arch}"))?;
    let link = pkg.get("link").and_then(|l| l.as_str()).ok_or("Adoptium: нет ссылки на архив")?;
    let sha = pkg.get("checksum").and_then(|c| c.as_str()).ok_or("Adoptium: нет контрольной суммы")?;
    if !link.ends_with(ending) {
        return Err(format!("Adoptium: неожиданный формат архива: {link}"));
    }
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Adoptium: неверная контрольная сумма".into());
    }
    Ok((link.to_string(), sha.to_lowercase()))
}

/// Unpack a JRE archive (`.zip` or `.tar.gz`) into `dest`, dropping its single
/// top-level folder. Entries that try to leave `dest` are refused; links are skipped.
fn unpack_jre(archive: &Path, dest: &Path) -> Result<(), String> {
    // "jdk-21.0.5+11-jre/bin/java" -> "bin/java"; None for the folder itself or an unsafe path
    fn inside(rel: &Path) -> Result<Option<PathBuf>, String> {
        let mut out = PathBuf::new();
        for (i, c) in rel.components().enumerate() {
            match c {
                std::path::Component::Normal(p) => {
                    if i > 0 {
                        out.push(p);
                    }
                }
                std::path::Component::CurDir => {}
                _ => return Err(format!("в архиве Java небезопасный путь: {}", rel.display())),
            }
        }
        Ok(Some(out).filter(|p| !p.as_os_str().is_empty()))
    }
    let name = archive.to_string_lossy().to_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let file = std::fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries().map_err(|e| format!("bad JRE archive: {e}"))? {
            let mut entry = entry.map_err(|e| format!("bad JRE archive: {e}"))?;
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                continue; // links, devices
            }
            let rel = entry.path().map_err(|e| e.to_string())?.into_owned();
            let Some(rel) = inside(&rel)? else { continue };
            let out = dest.join(rel);
            if kind.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mode = entry.header().mode().unwrap_or(0o644);
            let mut f = std::fs::File::create(&out).map_err(|e| format!("write {}: {e}", out.display()))?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
            drop(f);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode | 0o600));
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        return Ok(());
    }
    let file = std::fs::File::open(archive).map_err(|e| format!("open {}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("bad JRE archive: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let Some(rel) = inside(&rel)? else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| format!("write {}: {e}", out.display()))?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Make sure a Java runtime for `major` exists: reuse a matching installed one,
/// otherwise download a JRE from Adoptium into `<root>/runtime/java-<major>`
/// (Windows x64 and Linux; on other systems the user installs Java).
pub fn ensure_java(client: &Client, root: &Path, major: u32, prog: Option<&Arc<InstallProgress>>) -> Result<PathBuf, String> {
    if let Ok(p) = launch::find_java_path(None, Some(major)) {
        return Ok(p);
    }
    let Some(platform) = adoptium_platform() else {
        return Err(format!("Установите Java {major} или укажите путь в «Настройки»"));
    };
    // Adoptium ships 8, 11, 17, 21, 25; Mojang's 16 maps to 17.
    let major = if major == 16 { 17 } else { major };
    let target = root.join("runtime").join(format!("java-{major}"));
    let exe = target.join("bin").join(java_file_name());
    if exe.exists() {
        return Ok(exe);
    }
    set_file(prog, &format!("downloading Java {major}"));
    let (url, sha256) = adoptium_jre(client, major, platform)?;
    let zip_path = root.join("runtime").join(format!("java-{major}{}", platform.2));
    let dl = Arc::new(download::DownloadProgress::new("java"));
    // mirror cancel flag + progress of the install progress object
    let watcher_done = Arc::new(AtomicBool::new(false));
    if let Some(p) = prog {
        let (p, dl, done) = (p.clone(), dl.clone(), watcher_done.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                dl.cancelled.store(p.cancelled.load(Ordering::Relaxed), Ordering::Relaxed);
                p.current.store(dl.current.load(Ordering::Relaxed), Ordering::Relaxed);
                p.total.store(dl.total.load(Ordering::Relaxed), Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
    }
    let res = download::download_file_ex(client, &url, &zip_path, None, None, Some(&dl));
    watcher_done.store(true, Ordering::Relaxed);
    res?;
    // Adoptium publishes a SHA-256 for every build: refuse anything else
    let actual = download::file_sha256(&zip_path)?;
    if !actual.eq_ignore_ascii_case(&sha256) {
        let _ = std::fs::remove_file(&zip_path);
        return Err(format!("Java {major}: SHA-256 архива не совпал — файл повреждён или подменён"));
    }

    set_file(prog, &format!("unpacking Java {major}"));
    let tmp = root.join("runtime").join(format!("java-{major}.partial"));
    let _ = std::fs::remove_dir_all(&tmp);
    let unpack = unpack_jre(&zip_path, &tmp);
    let _ = std::fs::remove_file(&zip_path);
    if let Err(e) = unpack {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if !tmp.join("bin").join(java_file_name()).exists() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("в скачанном архиве нет {}", java_file_name()));
    }
    std::fs::rename(&tmp, &target).map_err(|e| format!("finalize Java: {e}"))?;
    Ok(exe)
}
