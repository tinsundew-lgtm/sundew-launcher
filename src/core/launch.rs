use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::install::{ARCH, OS};
use super::instance;
use super::mojang::{self, VersionJson};

const CP_SEP: &str = if cfg!(windows) { ";" } else { ":" };

// ------------------------------------------------------------------ Java

/// Major version of a java executable (`1.8.0_392` -> 8, `21.0.2` -> 21).
pub fn java_major(java: &Path) -> Option<u32> {
    let mut cmd = Command::new(java);
    cmd.arg("-version").stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    parse_java_major(&text)
}

fn parse_java_major(text: &str) -> Option<u32> {
    let start = text.find("version \"")? + "version \"".len();
    let ver = text[start..].split('"').next()?;
    let mut parts = ver.split(['.', '_', '-', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn java_exe() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

fn java_candidates() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("JAVA_HOME") {
        out.push(PathBuf::from(home).join("bin").join(java_exe()));
    }
    if let Ok(fp) = which::which("java") {
        out.push(fp);
    }
    let mut bases: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(pf) = std::env::var(pf) {
                for sub in ["Java", "Eclipse Adoptium", "Microsoft", "Zulu", "BellSoft", "Amazon Corporation", "Semeru"] {
                    bases.push(PathBuf::from(&pf).join(sub));
                }
            }
        }
    } else {
        bases.push(PathBuf::from("/usr/lib/jvm"));
        bases.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
    }
    for base in bases {
        if let Ok(rd) = std::fs::read_dir(&base) {
            let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            for d in dirs {
                out.push(d.join("bin").join(java_exe()));
                out.push(d.join("Contents/Home/bin").join(java_exe()));
            }
        }
    }
    // runtimes downloaded by the launcher itself
    if let Ok(rd) = std::fs::read_dir(instance::default_dir().join("runtime")) {
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        out.extend(dirs.into_iter().map(|d| d.join("bin").join(java_exe())));
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| p.exists() && seen.insert(p.clone()));
    out
}

/// Locate a java executable. An explicit `custom` path always wins. Otherwise
/// the installed Java matching `required` (exact, else the smallest newer one)
/// is chosen; with `required = None` the first one found is returned.
pub fn find_java_path(custom: Option<&str>, required: Option<u32>) -> Result<PathBuf, String> {
    if let Some(p) = custom.map(str::trim).filter(|p| !p.is_empty()) {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
        return Err(format!("Java не найдена по заданному пути: {p}"));
    }
    let cands = java_candidates();
    let Some(req) = required else {
        return cands.into_iter().next().ok_or_else(|| "Java не найдена — задайте путь в «Настройки»".to_string());
    };
    let mut found: Vec<(u32, PathBuf)> = cands
        .into_iter()
        .filter_map(|p| java_major(&p).map(|m| (m, p)))
        .collect();
    found.sort_by_key(|(m, _)| *m);
    if let Some((_, p)) = found.iter().find(|(m, _)| *m == req) {
        return Ok(p.clone());
    }
    if let Some((_, p)) = found.iter().find(|(m, _)| *m > req) {
        return Ok(p.clone());
    }
    Err(format!("Нужна Java {req}+, но подходящая не найдена — установите её или задайте путь в «Настройки»"))
}

// ------------------------------------------------------------------ version json

/// Read `versions/<name>` (flat `<id>.json`, or `<id>/<id>.json` as written by
/// the Forge/NeoForge installers).
fn read_version_file(version_dir: &Path, name: &str) -> Result<VersionJson, String> {
    let direct = instance::safe_join(version_dir, name)?;
    let path = if direct.exists() {
        direct
    } else {
        let stem = name.trim_end_matches(".json");
        let nested = version_dir.join(stem).join(format!("{stem}.json"));
        if nested.exists() { nested } else { direct }
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read version json {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse version json {}: {e}", path.display()))
}

/// Java major version the instance needs (from `javaVersion`), if known.
pub fn required_java_major(root: &Path, inst: &instance::Instance) -> Option<u32> {
    let version_dir = instance::instance_mines(root, &inst.name).join("versions");
    resolve_version(&version_dir, &version_json_name(inst))
        .ok()
        .and_then(|(v, _)| v.java_version.map(|j| j.major_version))
}

/// Load the instance's version json with `inheritsFrom` merged in.
/// Returns the merged json and the id whose `<id>.jar` is the vanilla client.
pub fn resolve_version(version_dir: &Path, json_name: &str) -> Result<(VersionJson, String), String> {
    let own = read_version_file(version_dir, json_name)?;
    match own.inherits_from.clone() {
        Some(parent_id) => {
            let parent = read_version_file(version_dir, &format!("{parent_id}.json"))?;
            let jar_id = parent.inherits_from.clone().unwrap_or(parent_id);
            Ok((own.merged_with(&parent), jar_id))
        }
        None => {
            let id = own.id.clone();
            Ok((own, id))
        }
    }
}

pub fn version_json_name(inst: &instance::Instance) -> String {
    inst.version_name.clone().unwrap_or_else(|| format!("{}.json", inst.game_version))
}

// ------------------------------------------------------------------ args

fn substitute(s: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
    }
    out
}

/// Expand a Mojang `arguments.game|jvm` array: plain strings, and objects
/// `{ rules, value }` whose rules are evaluated for this OS.
fn expand_args(val: Option<&serde_json::Value>, vars: &HashMap<&str, String>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(arr) = val.and_then(|v| v.as_array()) else { return out };
    for item in arr {
        match item {
            serde_json::Value::String(s) => out.push(substitute(s, vars)),
            serde_json::Value::Object(o) => {
                let rules: Vec<mojang::Rule> = o
                    .get("rules")
                    .and_then(|r| serde_json::from_value(r.clone()).ok())
                    .unwrap_or_default();
                if !mojang::rules_allow(&rules, OS, ARCH) {
                    continue;
                }
                match o.get("value") {
                    Some(serde_json::Value::String(s)) => out.push(substitute(s, vars)),
                    Some(serde_json::Value::Array(vs)) => {
                        out.extend(vs.iter().filter_map(|v| v.as_str()).map(|s| substitute(s, vars)))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

// ------------------------------------------------------------------ natives

/// File endings of native libraries (a native jar also holds checksums and metadata).
const NATIVE_EXTS: [&str; 4] = [".dll", ".so", ".dylib", ".jnilib"];

/// Since Minecraft 1.19 the LWJGL natives are ordinary libraries named like
/// `org.lwjgl:lwjgl:3.3.3:natives-windows` (a jar on the classpath), not entries
/// of the old `natives` table.
fn is_native_jar(lib: &mojang::Library) -> bool {
    lib.natives.is_none() && lib.name.split(':').nth(3).is_some_and(|classifier| classifier.starts_with("natives-"))
}

/// Does a native jar fit the CPU we run on? Mojang lists the natives of every CPU
/// under the same OS rule (`natives-windows` and `natives-windows-arm64`), so the
/// classifier decides. Putting the wrong one on the classpath is harmful: LWJGL
/// then reports a "platform/architecture mismatch" and the game dies. Libraries
/// that are not native jars always fit.
fn native_jar_fits(lib: &mojang::Library, arch: &str) -> bool {
    if !is_native_jar(lib) {
        return true;
    }
    let classifier = lib.name.split(':').nth(3).unwrap_or("");
    let other_cpu = ["-arm64", "-aarch64", "-arm32", "-x86", "-x32"].iter().any(|s| classifier.ends_with(s));
    if arch == "aarch64" {
        classifier.ends_with("-arm64") || classifier.ends_with("-aarch64")
    } else {
        !other_cpu
    }
}

/// Unpack the files of a native jar flat into `dest` (files that are already
/// there with the right size are left alone: a running game may hold them).
fn unpack_flat(jar: &Path, dest: &Path, only_native: bool) -> Result<(), String> {
    let file = std::fs::File::open(jar).map_err(|e| format!("{}: {e}", jar.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", jar.display()))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| format!("{}: {e}", jar.display()))?;
        if entry.is_dir() || entry.name().starts_with("META-INF") {
            continue;
        }
        let Some(name) = entry.enclosed_name().and_then(|p| p.file_name().map(|f| f.to_owned())) else { continue };
        let lower = name.to_string_lossy().to_lowercase();
        if only_native && !NATIVE_EXTS.iter().any(|e| lower.ends_with(e)) {
            continue;
        }
        let target = dest.join(&name);
        if target.metadata().map(|m| m.len() == entry.size()).unwrap_or(false) {
            continue;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| format!("Не удалось записать {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("Не удалось записать {}: {e}", target.display()))?;
    }
    Ok(())
}

/// Put every native library of the version into `natives_dir`, where the game
/// is told to look for them (`java.library.path` and `org.lwjgl.librarypath`).
fn extract_natives(vjson: &VersionJson, libs_dir: &Path, natives_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(natives_dir).map_err(|e| format!("Не удалось создать {}: {e}", natives_dir.display()))?;
    for lib in &vjson.libraries {
        if !mojang::library_allowed(lib, OS, ARCH) {
            continue;
        }
        if let (Some(map), Some(dl)) = (&lib.natives, &lib.downloads) {
            // old style: a classifier jar listed in the `natives` table
            let Some(classifier) = map.get(OS).map(|c| c.replace("${arch}", "64")) else { continue };
            let Some(art) = dl.classifiers.as_ref().and_then(|c| c.get(&classifier)) else { continue };
            let jar = libs_dir.join(&art.path);
            if jar.is_file() {
                unpack_flat(&jar, natives_dir, false)?;
            }
        } else if is_native_jar(lib) && native_jar_fits(lib, ARCH) {
            let Some(rel) = lib.artifact_path() else { continue };
            let jar = libs_dir.join(rel);
            if !jar.is_file() {
                // without it the game dies with "Failed to locate library: lwjgl.dll"
                return Err(format!("Не найдена библиотека {} — переустановите сборку", lib.name));
            }
            unpack_flat(&jar, natives_dir, true)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ launch

pub struct LaunchResult {
    pub java: PathBuf,
    pub command: Vec<String>,
    pub game_dir: PathBuf,
}

/// Build the full java command line for an instance.
///
/// Layout (under `root`): `instances/<name>/mines/{libraries,versions,assets}`.
/// `command` is a flat Vec = [jvm args..., main class, game args...].
pub fn build_launch_command(
    custom_java: Option<&str>,
    root: &Path,
    instance: &instance::Instance,
    username: &str,
    uuid: &str,
    access_token: &str,
    ram_mb: u64,
    extra_game_args: &[String],
) -> Result<LaunchResult, String> {
    let mines = instance::instance_mines(root, &instance.name);
    let version_dir = mines.join("versions");
    let libs_dir = mines.join("libraries");
    let assets_root = mines.join("assets");
    let natives_dir = mines.join("natives");

    let json_name = version_json_name(instance);
    let (vjson, jar_id) = resolve_version(&version_dir, &json_name)?;

    // Java: instance override > global setting > auto-detect by javaVersion.
    let required = vjson.java_version.as_ref().map(|j| j.major_version);
    let custom = instance.java_path.as_deref().filter(|s| !s.trim().is_empty()).or(custom_java);
    let java = find_java_path(custom, required)?;

    // classpath from the library list (not "every jar in the folder")
    let mut classpath: Vec<PathBuf> = Vec::new();
    for lib in &vjson.libraries {
        if !mojang::library_allowed(lib, OS, ARCH) || lib.natives.is_some() || !native_jar_fits(lib, ARCH) {
            continue;
        }
        if let Some(rel) = lib.artifact_path() {
            let p = libs_dir.join(rel);
            if p.exists() && !classpath.contains(&p) {
                classpath.push(p);
            }
        }
    }
    let client_jar = version_dir.join(format!("{jar_id}.jar"));
    if client_jar.exists() {
        classpath.push(client_jar);
    } else {
        return Err(format!("Не найден клиент {jar_id}.jar — переустановите сборку"));
    }
    extract_natives(&vjson, &libs_dir, &natives_dir)?;

    let classpath_str = classpath.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(CP_SEP);
    let asset_id = vjson
        .assets
        .clone()
        .or_else(|| vjson.asset_index.as_ref().map(|a| a.id.clone()))
        .ok_or("В version json нет информации об ассетах")?;

    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", username.to_string());
    vars.insert("version_name", vjson.id.clone());
    vars.insert("game_directory", mines.display().to_string());
    vars.insert("assets_root", assets_root.display().to_string());
    vars.insert("game_assets", assets_root.display().to_string());
    vars.insert("assets_index_name", asset_id);
    vars.insert("auth_uuid", uuid.to_string());
    vars.insert("auth_access_token", access_token.to_string());
    vars.insert("auth_session", access_token.to_string());
    vars.insert("clientid", String::new());
    vars.insert("auth_xuid", String::new());
    vars.insert("user_type", if access_token == "0" { "legacy" } else { "msa" }.to_string());
    vars.insert("version_type", "release".to_string());
    vars.insert("user_properties", "{}".to_string());
    vars.insert("natives_directory", natives_dir.display().to_string());
    vars.insert("launcher_name", "LimeCraft".to_string());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").to_string());
    vars.insert("classpath", classpath_str.clone());
    vars.insert("library_directory", libs_dir.display().to_string());
    vars.insert("classpath_separator", CP_SEP.to_string());

    let ram = instance.memory_mb.unwrap_or(ram_mb);
    let mut command = vec![format!("-Xmx{ram}M")];
    if let Some(extra) = instance.jvm_args.as_deref() {
        command.extend(extra.split_whitespace().map(str::to_string));
    }
    let jvm = expand_args(vjson.arguments.as_ref().and_then(|a| a.jvm.as_ref()), &vars);
    let has_cp = jvm.iter().any(|a| a == "-cp" || a == "-classpath");
    let has_natives = jvm.iter().any(|a| a.starts_with("-Djava.library.path"));
    command.extend(jvm);
    if !has_natives {
        command.push(format!("-Djava.library.path={}", natives_dir.display()));
    }
    // LWJGL finds its libraries here first, so it never has to unpack them into
    // the system temp folder (which some computers do not let programs write to);
    // everything else that needs a temp folder gets one inside the game folder too.
    let tmp_dir = mines.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    for (key, value) in [("-Dorg.lwjgl.librarypath=", natives_dir.display().to_string()), ("-Djava.io.tmpdir=", tmp_dir.display().to_string())] {
        if !command.iter().any(|a| a.starts_with(key)) {
            command.push(format!("{key}{value}"));
        }
    }
    if !has_cp {
        command.push("-cp".to_string());
        command.push(classpath_str);
    }
    command.push(vjson.main_class.clone().unwrap_or_else(|| "net.minecraft.client.main.Main".to_string()));

    let game = match (&vjson.arguments, &vjson.minecraft_arguments) {
        (Some(a), _) if a.game.is_some() => expand_args(a.game.as_ref(), &vars),
        (_, Some(legacy)) => legacy.split_whitespace().map(|s| substitute(s, &vars)).collect(),
        _ => Vec::new(),
    };
    command.extend(game);
    // window size / fullscreen from the settings
    command.extend(extra_game_args.iter().cloned());

    Ok(LaunchResult { java, command, game_dir: mines })
}

/// New lines appended to a log since `offset` (live console). When the file got
/// shorter (a new game run truncated it), reading restarts from the beginning
/// and `restarted` is true. At most `max_bytes` are read per call.
pub fn read_log_tail(path: &Path, offset: &mut u64, max_bytes: u64) -> (Vec<String>, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return (Vec::new(), false) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut restarted = false;
    if len < *offset {
        *offset = 0;
        restarted = true;
    }
    if len == *offset {
        return (Vec::new(), restarted);
    }
    // a huge log: show only its end
    if len - *offset > max_bytes {
        *offset = len - max_bytes;
    }
    if f.seek(SeekFrom::Start(*offset)).is_err() {
        return (Vec::new(), restarted);
    }
    let mut buf = Vec::new();
    let _ = f.take(max_bytes).read_to_end(&mut buf);
    // keep an incomplete last line for the next call
    let complete = buf.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
    *offset += complete as u64;
    let text = String::from_utf8_lossy(&buf[..complete]);
    (text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect(), restarted)
}

/// Games started by this launcher: instance name -> process id.
static RUNNING: std::sync::Mutex<Vec<(String, u32)>> = std::sync::Mutex::new(Vec::new());

pub fn is_running(instance_name: &str) -> bool {
    RUNNING.lock().map(|r| r.iter().any(|(n, _)| n == instance_name)).unwrap_or(false)
}

pub fn running_count() -> usize {
    RUNNING.lock().map(|r| r.len()).unwrap_or(0)
}

/// Remember the child and clean up when it exits.
pub fn track(instance_name: &str, mut child: std::process::Child) {
    let name = instance_name.to_string();
    if let Ok(mut r) = RUNNING.lock() {
        r.push((name.clone(), child.id()));
    }
    std::thread::spawn(move || {
        let _ = child.wait();
        if let Ok(mut r) = RUNNING.lock() {
            r.retain(|(n, _)| *n != name);
        }
    });
}

/// Ask the OS to terminate the running game of `instance_name`.
pub fn stop(instance_name: &str) -> Result<(), String> {
    let pid = RUNNING
        .lock()
        .ok()
        .and_then(|r| r.iter().find(|(n, _)| n == instance_name).map(|(_, p)| *p))
        .ok_or("игра не запущена")?;
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"]);
        c
    } else {
        let mut c = Command::new("kill");
        c.arg(pid.to_string());
        c
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    cmd.status().map_err(|e| format!("не удалось остановить игру: {e}")).map(|_| ())
}

/// Run the game with output going to `launcher.log` (NOT `logs/latest.log`,
/// which Minecraft's own log4j writes to).
pub fn spawn_game(java: &Path, command: &[String], game_dir: &Path) -> Result<std::process::Child, String> {
    let log_path = game_dir.join("launcher.log");
    let log_file = std::fs::File::create(&log_path).map_err(|e| format!("create {}: {e}", log_path.display()))?;
    let mut cmd = Command::new(java);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no extra console window
    }
    // a temp folder the game can always write to (the system one may be locked down)
    let tmp = game_dir.join("tmp");
    let _ = std::fs::create_dir_all(&tmp);
    cmd.env("TMP", &tmp).env("TEMP", &tmp);
    cmd.args(command)
        .current_dir(game_dir)
        .stdout(Stdio::from(log_file.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log_file));
    cmd.spawn().map_err(|e| format!("cannot start java: {e}"))
}

/// Wait briefly to catch an instant crash; on exit return the log tail.
pub fn check_early_exit(child: &mut std::process::Child, game_dir: &Path, secs: u64) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            let log = std::fs::read_to_string(game_dir.join("launcher.log")).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(8).collect();
            let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
            return Err(format!("Игра завершилась сразу (код {:?}). Лог: launcher.log\n{tail}", status.code()));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_tail_reads_only_new_complete_lines() {
        let dir = std::env::temp_dir().join(format!("lc-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("launcher.log");
        std::fs::write(&p, "one\r\ntwo\nthr").unwrap();
        let mut off = 0;
        let (lines, restarted) = read_log_tail(&p, &mut off, 1 << 20);
        assert_eq!(lines, vec!["one", "two"]);
        assert!(!restarted);
        std::fs::write(&p, "one\r\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(read_log_tail(&p, &mut off, 1 << 20).0, vec!["three", "four"]);
        assert!(read_log_tail(&p, &mut off, 1 << 20).0.is_empty());
        // a new game run truncates the file
        std::fs::write(&p, "fresh\n").unwrap();
        let (lines, restarted) = read_log_tail(&p, &mut off, 1 << 20);
        assert!(restarted);
        assert_eq!(lines, vec!["fresh"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_settings_reach_the_command_line() {
        // a minimal installed vanilla version
        let root = std::env::temp_dir().join(format!("lc-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let inst = instance::Instance { name: "w".into(), game_version: "1.0".into(), ..Default::default() };
        let versions = instance::instance_mines(&root, "w").join("versions");
        std::fs::create_dir_all(&versions).unwrap();
        std::fs::write(versions.join("1.0.json"), r#"{"id":"1.0","mainClass":"a.Main","assets":"1","libraries":[],"arguments":{"game":["--version","${version_name}"],"jvm":[]}}"#).unwrap();
        std::fs::write(versions.join("1.0.jar"), b"jar").unwrap();
        let java = std::env::current_exe().unwrap(); // any existing file stands in for java
        let extra = vec!["--width".to_string(), "1600".to_string(), "--fullscreen".to_string()];
        let r = build_launch_command(Some(java.to_str().unwrap()), &root, &inst, "Steve", "uuid", "0", 2048, &extra).unwrap();
        let tail: Vec<&str> = r.command.iter().rev().take(3).rev().map(|s| s.as_str()).collect();
        assert_eq!(tail, vec!["--width", "1600", "--fullscreen"]);
        assert!(r.command.contains(&"-Xmx2048M".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A zip in `path` with the given (name, bytes) entries.
    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let o: zip::write::SimpleFileOptions = Default::default();
        for (n, c) in files {
            z.start_file(*n, o).unwrap();
            std::io::Write::write_all(&mut z, c).unwrap();
        }
        z.finish().unwrap();
    }

    const NATIVE_LIB: &str = r#"{"id":"1.0","mainClass":"a.Main","assets":"1","arguments":{"game":[],"jvm":[]},
        "libraries":[{"name":"org.lwjgl:lwjgl:3.3.3:natives-windows",
        "downloads":{"artifact":{"path":"org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar","url":"x"}}}]}"#;

    fn native_fixture(tag: &str) -> (PathBuf, instance::Instance) {
        let root = std::env::temp_dir().join(format!("lc-natives-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let inst = instance::Instance { name: "n".into(), game_version: "1.0".into(), ..Default::default() };
        let mines = instance::instance_mines(&root, "n");
        std::fs::create_dir_all(mines.join("versions")).unwrap();
        std::fs::write(mines.join("versions/1.0.json"), NATIVE_LIB).unwrap();
        std::fs::write(mines.join("versions/1.0.jar"), b"jar").unwrap();
        (root, inst)
    }

    #[test]
    fn lwjgl_natives_are_unpacked_and_found_without_the_system_temp_folder() {
        let (root, inst) = native_fixture("ok");
        let mines = instance::instance_mines(&root, "n");
        write_zip(
            &mines.join("libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"),
            &[("windows/x64/org/lwjgl/lwjgl.dll", b"DLL1"), ("windows/x64/org/lwjgl/lwjgl.dll.sha1", b"abc"), ("META-INF/MANIFEST.MF", b"m")],
        );
        let java = std::env::current_exe().unwrap();
        let r = build_launch_command(Some(java.to_str().unwrap()), &root, &inst, "Steve", "u", "0", 1024, &[]).unwrap();
        let natives = mines.join("natives");
        assert_eq!(std::fs::read(natives.join("lwjgl.dll")).unwrap(), b"DLL1");
        assert!(!natives.join("lwjgl.dll.sha1").exists() && !natives.join("MANIFEST.MF").exists(), "only the libraries are unpacked");
        assert!(r.command.contains(&format!("-Dorg.lwjgl.librarypath={}", natives.display())), "{:?}", r.command);
        assert!(r.command.contains(&format!("-Djava.io.tmpdir={}", mines.join("tmp").display())), "{:?}", r.command);
        assert!(mines.join("tmp").is_dir());
        // the native jar stays on the classpath too
        let cp = r.command.iter().position(|a| a == "-cp").map(|i| r.command[i + 1].clone()).unwrap_or_default();
        assert!(cp.contains("natives-windows.jar"), "{cp}");
        // starting again keeps a library that is already there (a game may hold it open)
        build_launch_command(Some(java.to_str().unwrap()), &root, &inst, "Steve", "u", "0", 1024, &[]).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn natives_of_another_cpu_never_reach_the_classpath() {
        let lib = |name: &str| mojang::Library { name: name.into(), ..Default::default() };
        let (x64, arm, x86, plain) = (
            lib("org.lwjgl:lwjgl:3.4.3:natives-windows"),
            lib("org.lwjgl:lwjgl:3.4.3:natives-windows-arm64"),
            lib("org.lwjgl:lwjgl:3.3.3:natives-windows-x86"),
            lib("org.lwjgl:lwjgl:3.4.3"),
        );
        assert!(native_jar_fits(&x64, "64") && !native_jar_fits(&arm, "64") && !native_jar_fits(&x86, "64"));
        assert!(!native_jar_fits(&x64, "aarch64") && native_jar_fits(&arm, "aarch64"));
        assert!(native_jar_fits(&plain, "64") && native_jar_fits(&plain, "aarch64"), "ordinary libraries are not touched");
        assert!(!native_jar_fits(&lib("org.lwjgl:lwjgl:3.4.3:natives-macos-arm64"), "64"));
        assert!(native_jar_fits(&lib("org.lwjgl:lwjgl:3.4.3:natives-linux"), "64"));
    }

    #[test]
    fn both_cpus_listed_by_mojang_give_only_the_right_libraries() {
        // the real layout of Minecraft 26.x: x64 and arm64 natives under the same rule
        let root = std::env::temp_dir().join(format!("lc-natives-both-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let inst = instance::Instance { name: "n".into(), game_version: "1.0".into(), ..Default::default() };
        let mines = instance::instance_mines(&root, "n");
        std::fs::create_dir_all(mines.join("versions")).unwrap();
        let lib = |classifier: &str| {
            format!(r#"{{"name":"org.lwjgl:lwjgl:3.4.3:natives-{classifier}","rules":[{{"action":"allow","os":{{"name":"{OS}"}}}}],
              "downloads":{{"artifact":{{"path":"org/lwjgl/lwjgl/3.4.3/lwjgl-3.4.3-natives-{classifier}.jar","url":"x"}}}}}}"#)
        };
        let json = format!(
            r#"{{"id":"1.0","mainClass":"a.Main","assets":"1","arguments":{{"game":[],"jvm":[]}},"libraries":[{},{}]}}"#,
            lib(&format!("{OS}")),
            lib(&format!("{OS}-arm64"))
        );
        std::fs::write(mines.join("versions/1.0.json"), json).unwrap();
        std::fs::write(mines.join("versions/1.0.jar"), b"jar").unwrap();
        let libs = mines.join("libraries/org/lwjgl/lwjgl/3.4.3");
        write_zip(&libs.join(format!("lwjgl-3.4.3-natives-{OS}.jar")), &[("x64/org/lwjgl/lwjgl.dll", b"X64-DLL")]);
        write_zip(&libs.join(format!("lwjgl-3.4.3-natives-{OS}-arm64.jar")), &[("arm64/org/lwjgl/lwjgl.dll", b"ARM-DLL-LONGER")]);
        let java = std::env::current_exe().unwrap();
        let r = build_launch_command(Some(java.to_str().unwrap()), &root, &inst, "S", "u", "0", 1024, &[]).unwrap();
        let cp = r.command.iter().position(|a| a == "-cp").map(|i| r.command[i + 1].clone()).unwrap_or_default();
        if ARCH == "aarch64" {
            assert!(cp.contains("arm64.jar") && !cp.contains(&format!("natives-{OS}.jar")), "{cp}");
            assert_eq!(std::fs::read(mines.join("natives/lwjgl.dll")).unwrap(), b"ARM-DLL-LONGER");
        } else {
            assert!(cp.contains(&format!("natives-{OS}.jar")) && !cp.contains("arm64"), "{cp}");
            assert_eq!(std::fs::read(mines.join("natives/lwjgl.dll")).unwrap(), b"X64-DLL", "the other CPU's file must not win");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn user_jvm_properties_are_not_overridden() {
        let (root, mut inst) = native_fixture("user");
        let mines = instance::instance_mines(&root, "n");
        write_zip(&mines.join("libraries/org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"), &[("a/lwjgl.dll", b"x")]);
        inst.jvm_args = Some("-Djava.io.tmpdir=D:/mytmp".into());
        let java = std::env::current_exe().unwrap();
        let r = build_launch_command(Some(java.to_str().unwrap()), &root, &inst, "Steve", "u", "0", 1024, &[]).unwrap();
        assert_eq!(r.command.iter().filter(|a| a.starts_with("-Djava.io.tmpdir=")).count(), 1);
        assert!(r.command.contains(&"-Djava.io.tmpdir=D:/mytmp".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_native_jar_is_reported_not_silently_skipped() {
        let (root, inst) = native_fixture("missing");
        let java = std::env::current_exe().unwrap();
        let err = build_launch_command(Some(java.to_str().unwrap()), &root, &inst, "Steve", "u", "0", 1024, &[]).err().expect("must fail");
        assert!(err.contains("lwjgl") && err.contains("переустановите"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn java_version_parsing() {
        assert_eq!(parse_java_major("openjdk version \"1.8.0_392\" 2023"), Some(8));
        assert_eq!(parse_java_major("openjdk version \"21.0.2\" 2024-01-16"), Some(21));
        assert_eq!(parse_java_major("java version \"17\" 2021"), Some(17));
        assert_eq!(parse_java_major("garbage"), None);
    }

    #[test]
    fn args_rules_and_placeholders() {
        let json: serde_json::Value = serde_json::from_str(
            r#"["--username","${auth_player_name}",
            {"rules":[{"action":"allow","features":{"is_demo_user":true}}],"value":"--demo"},
            {"rules":[{"action":"allow","os":{"name":"windows"}}],"value":["-Dwin=1"]},
            {"rules":[{"action":"allow","os":{"name":"nonexistent-os"}}],"value":"-Dnope"}]"#,
        )
        .unwrap();
        let mut vars = HashMap::new();
        vars.insert("auth_player_name", "Steve".to_string());
        let out = expand_args(Some(&json), &vars);
        assert_eq!(&out[..2], &["--username", "Steve"]);
        assert!(!out.contains(&"--demo".to_string()));
        assert!(!out.contains(&"-Dnope".to_string()));
        assert_eq!(out.contains(&"-Dwin=1".to_string()), OS == "windows");
    }
}
