use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::instance;
use super::mojang::VersionJson;

/// Locate java executable.
pub fn find_java_path(custom: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = custom {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    // JAVA_HOME
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let mut cand = vec![PathBuf::from(&home).join("bin/java.exe")];
        if cfg!(windows) {
            cand.push(PathBuf::from(&home).join("bin").join("java.exe"));
        } else {
            cand.push(PathBuf::from(&home).join("bin/java"));
        }
        for c in cand {
            if c.exists() {
                return Some(c);
            }
        }
    }
    // which java
    if let Ok(fp) = which::which("java") {
        return Some(fp);
    }
    if let Ok(fp) = which::which("java.exe") {
        return Some(fp);
    }
    // common windows paths
    for base in [
        "C:\\Program Files\\Java",
        "C:\\Program Files\\Eclipse Adoptium",
        "C:\\Program Files\\Microsoft",
    ] {
        if let Ok(rd) = std::fs::read_dir(base) {
            for e in rd.flatten().filter(|e| e.path().is_dir()) {
                let j = e.path().join("bin/java.exe");
                if j.exists() {
                    return Some(j);
                }
            }
        }
    }
    None
}

pub struct LaunchResult {
    pub command: Vec<String>,
    pub game_dir: PathBuf,
}

/// Build the full java command line for an instance.
///
/// Layout (under `root`):
///   instances/<name>/mines/
///     libraries/   (all jar deps)
///     versions/    (version.jsons + client.jar)
///     assets/
///     saves/
///
/// `command` is a flat Vec = [jvm args..., main class, game args...].
pub fn build_launch_command(
    _java: &Path,
    root: &Path,
    instance: &instance::Instance,
    username: &str,
    uuid: &str,
    _access_token: &str,
    ram_mb: u64,
) -> Result<LaunchResult, String> {
    let inst_dir = instance::instance_dir(root, &instance.name);
    let mines = inst_dir.join("mines");
    let version_dir = mines.join("versions");
    let libs_dir = mines.join("libraries");
    let assets_root = mines.join("assets");
    let game_dir = mines.clone();

    // Which version json to read: loaders write a loader-named json and record
    // it in `version_name`; vanilla instances default to the game version.
    let json_name = instance.version_name.clone().unwrap_or_else(|| {
        if !instance.loader.is_empty() && !instance.loader_version.is_empty() {
            format!("{}.json", instance.loader_version)
        } else {
            format!("{}.json", instance.game_version)
        }
    });
    let json_path = version_dir.join(&json_name);
    let vjson_text = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("read version json {}: {e}", json_path.display()))?;
    let vjson: VersionJson = serde_json::from_str(&vjson_text).map_err(|e| format!("parse version json: {e}"))?;
    let main_class = vjson.main_class.clone().unwrap_or_else(|| "net.minecraft.client.Main".to_string());

    // classpath: all jars in mines/libraries + client jar + loader jars in versions dir
    let mut classpath = collect_jars(&libs_dir);
    let version_jar = version_dir.join(format!("{}.jar", instance.game_version));
    if version_jar.exists() && !classpath.iter().any(|p| p.eq(&version_jar)) {
        classpath.push(version_jar);
    }
    if let Ok(rd) = std::fs::read_dir(&version_dir) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "jar").unwrap_or(false) {
                let p = e.path();
                if !classpath.iter().any(|c| c == &p) {
                    classpath.push(p);
                }
            }
        }
    }

    let classpath_str = classpath
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" });

    let natives_dir = libs_dir.join("natives");
    let asset_id = vjson
        .assets
        .clone()
        .or_else(|| vjson.asset_index.as_ref().map(|a| a.id.clone()))
        .unwrap_or_else(|| "1.7.10".to_string());

    // jvm args
    let mut jvm_args = vec![
        format!("-Xmx{}M", ram_mb),
        "-XX:+UnlockExperimentalVMOptions".to_string(),
        format!("-Djava.library.path={}", natives_dir.display()),
        "-cp".to_string(),
        classpath_str.clone(),
    ];
    // optional jvm args from version json, with placeholders resolved
    if let Some(args) = &vjson.arguments {
        if let Some(jvm) = args.jvm.as_ref() {
            if let Some(arr) = jvm.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        if s.contains("${natives_directory}") || s.contains("${launcher_name}") || s.contains("${launcher_version}") {
                            continue; // handled via -Djava.library.path above
                        }
                        jvm_args.push(
                            s.replace("${library_directory}", &libs_dir.display().to_string())
                                .replace("${classpath}", &classpath_str)
                                .replace("${natives_directory}", &natives_dir.display().to_string()),
                        );
                    }
                }
            }
        }
    }
    jvm_args.push(main_class);

    // game args
    let mut game_args = vec![
        format!("--username={username}"),
        format!("--uuid={uuid}"),
        "--accessToken=0".to_string(),
        "--userType=mojang".to_string(),
        "--version".to_string(),
        instance.game_version.clone(),
        "--gameDir".to_string(),
        game_dir.display().to_string(),
        "--assetsDir".to_string(),
        assets_root.display().to_string(),
        "--assetIndex".to_string(),
        asset_id,
    ];
    // game args from json, but only plain values that add something
    if let Some(args) = &vjson.arguments {
        if let Some(game) = args.game.as_ref() {
            if let Some(arr) = game.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        if s.contains('$') || s.starts_with("--") || s == "0" {
                            continue;
                        }
                        game_args.push(s.to_string());
                    }
                }
            }
        }
    }

    let mut command = jvm_args;
    command.extend(game_args);

    Ok(LaunchResult { command, game_dir })
}

/// Recursive jar collection.
fn collect_jars(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(collect_jars(&p));
            } else if p.extension().map(|x| x == "jar").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

/// Actually run the game: spawn java with command args, redirect stdout to launch log.
pub fn spawn_game(java: &Path, command: &[String], game_dir: &Path) -> Result<std::process::Child, String> {
    let _ = std::fs::create_dir_all(game_dir.join("logs"));
    let log_path = game_dir.join("logs/latest.log");
    let log_file = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(java);
    cmd.args(command)
        .current_dir(game_dir)
        .stdout(Stdio::from(log_file.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log_file));
    cmd.spawn().map_err(|e| format!("cannot start java: {e}"))
}
