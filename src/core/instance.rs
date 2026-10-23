use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::account::Account;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Instance {
    pub name: String,
    pub game_version: String,
    pub loader: String,          // "", "fabric", "quilt", "forge", "neoforge"
    pub loader_version: String,  // chosen loader version
    /// File name of the resolved version.json inside the instance's
    /// `mines/versions/` dir (e.g. "1.21.1.json", "fabric-0.16.9-1.21.1.json").
    #[serde(default)]
    pub version_name: Option<String>,
    pub modpack_slug: Option<String>,
    pub created: String,

    /// --- Каркас под индивидуальные настройки сборки (задел на будущее, Пункт 4) ---
    /// Выделенная память (МБ). `None` = используется глобальный `LauncherState.ram_mb`.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// Кастомные JVM-аргументы для этой сборки.
    #[serde(default)]
    pub jvm_args: Option<String>,
    /// Свой путь к Java для этой сборки (иначе глобальный из LauncherState).
    #[serde(default)]
    pub java_path: Option<String>,
    /// Путь/идентификатор иконки сборки (заготовка; рендер позже).
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LauncherState {
    pub instances: Vec<Instance>,
    pub java_path: Option<String>,
    pub ram_mb: u64,
    /// Учётная запись для запуска игры. Офлайн по умолчанию; при первом
    /// запуске (отсутствии поля в старом файле состояния) подставляется
    /// Account::offline_default() при загрузке в UI.
    #[serde(default)]
    pub account: Option<Account>,
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join("launcher_state.json")
}

pub fn default_dir() -> PathBuf {
    // Пользовательская директория данных (Windows: %APPDATA%,
    // macOS: ~/Library/Application Support, Linux: ~/.local/share).
    // Никогда не пишем рядом с .exe — это устраняет PermissionDenied
    // в защищённых папках в стиле "Program Files".
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("LimeCraft")
}

/// Read state file.
pub fn load_state(root: &Path) -> LauncherState {
    let p = state_path(root);
    match std::fs::read(&p) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => LauncherState::default(),
    }
}

pub fn save_state(root: &Path, state: &LauncherState) -> Result<(), String> {
    let p = state_path(root);
    let _ = std::fs::create_dir_all(root);
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(p, json).map_err(|e| e.to_string())
}

/// Path to a given instance's directory inside root.
pub fn instance_dir(root: &Path, name: &str) -> PathBuf {
    root.join("instances").join(sanitize(name))
}

/// Path to the `.minecraft`-style root of an instance (where "mines" holds
/// libraries/versions/assets — same layout `launch` reads from).
pub fn instance_mines(root: &Path, name: &str) -> PathBuf {
    instance_dir(root, name).join("mines")
}

pub fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect();
    while s.starts_with(' ') || s.starts_with('.') || s.starts_with('_') {
        s.remove(0);
    }
    if s.is_empty() {
        s = "instance".to_string();
    }
    s
}

/// Instance .minecraft layout:
///   instances/<name>/
///     mines/
///       libraries/
///       versions/<id>.json   (flat json + <id>.jar)
///       assets/
///       mods/
///       config/
///       saves/
#[allow(dead_code)]
pub fn prepare_instance_dir(instance: &Instance, root: &Path) -> Result<PathBuf, String> {
    let dir = instance_dir(root, &instance.name);
    std::fs::create_dir_all(dir.join("mines/libraries"))
        .map_err(|e| format!("Cannot create instance libraries: {e}"))?;
    std::fs::create_dir_all(dir.join("mines/versions"))
        .map_err(|e| format!("Cannot create versions dir: {e}"))?;
    std::fs::create_dir_all(dir.join("mines/assets"))
        .map_err(|e| format!("Cannot create assets dir: {e}"))?;
    std::fs::create_dir_all(dir.join("mines/mods"))
        .map_err(|e| format!("Cannot create mods dir: {e}"))?;
    Ok(dir)
}
