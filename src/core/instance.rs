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
    /// Своя иконка: имя файла картинки в папке сборки (обычно `icon.png`).
    #[serde(default)]
    pub icon: Option<String>,
    /// Unix seconds of the last launch.
    #[serde(default)]
    pub last_played: Option<u64>,
    /// Цвет плитки (RGB); `None` = цвет из имени.
    #[serde(default)]
    pub tile_color: Option<[u8; 3]>,
    /// Группа в библиотеке.
    #[serde(default)]
    pub group: Option<String>,
    /// Закреплена вверху библиотеки.
    #[serde(default)]
    pub pinned: bool,
    /// Свой ник для этой сборки (`None` = активный профиль).
    #[serde(default)]
    pub username: Option<String>,
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
    /// Do not fall back to community mirrors when a main server is unreachable.
    #[serde(default)]
    pub disable_mirrors: bool,
    /// Customization (look, library, game window, after launch).
    #[serde(default)]
    pub ui: super::prefs::UiPrefs,
    /// Offline profiles (nicknames); `account` mirrors the active one.
    #[serde(default)]
    pub profiles: Vec<Account>,
    #[serde(default)]
    pub active_profile: usize,
    /// Offline skin library (pictures in `<data>/skins`).
    #[serde(default)]
    pub skins: Vec<super::skins::Skin>,
}

impl LauncherState {
    /// Make `profiles` non-empty (older files only had `account`), keep the
    /// active index valid and `account` equal to the active profile.
    pub fn normalize(&mut self) {
        if self.profiles.is_empty() {
            self.profiles.push(self.account.clone().unwrap_or_else(Account::offline_default));
        }
        if self.active_profile >= self.profiles.len() {
            self.active_profile = 0;
        }
        // a profile never points at a skin that is gone
        let known: Vec<String> = self.skins.iter().map(|s| s.id.clone()).collect();
        for p in &mut self.profiles {
            if p.skin.as_ref().is_some_and(|s| !known.contains(s)) {
                p.skin = None;
            }
        }
        self.account = Some(self.profiles[self.active_profile].clone());
        self.ui = std::mem::take(&mut self.ui).sanitized();
    }

    pub fn active_account(&self) -> Account {
        self.profiles.get(self.active_profile).cloned().or_else(|| self.account.clone()).unwrap_or_else(Account::offline_default)
    }

    /// Account to launch `inst` with: its own nickname if set, else the active profile.
    pub fn account_for(&self, inst: &Instance) -> Account {
        let mut acc = self.active_account();
        if let Some(nick) = inst.username.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            if let Some(p) = self.profiles.iter().find(|p| p.username == nick) {
                return p.clone();
            }
            acc = Account { username: nick.to_string(), uuid: None, avatar: None, skin: None };
        }
        acc
    }
}

pub fn state_path(root: &Path) -> PathBuf {
    root.join("launcher_state.json")
}

static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Preferred (not yet verified) data directory: `LIMECRAFT_HOME`, otherwise the
/// per-user data dir (Windows: %APPDATA%\LimeCraft).
pub fn preferred_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("LIMECRAFT_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("LimeCraft")
}

/// The data directory in use: the one picked by `resolve_root`, or the
/// preferred one before that.
pub fn default_dir() -> PathBuf {
    ROOT.get().cloned().unwrap_or_else(preferred_dir)
}

/// True if we can really create nested directories and files under `dir`
/// (some environments allow creating the top folder but deny deeper writes).
fn dir_writable(dir: &Path) -> bool {
    let probe_dir = dir.join("instances").join(".probe");
    if std::fs::create_dir_all(&probe_dir).is_err() {
        return false;
    }
    let file = probe_dir.join("w.tmp");
    let ok = std::fs::write(&file, b"x").is_ok();
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(&probe_dir);
    ok
}

/// Pick a writable data directory. Order: `LIMECRAFT_HOME`, the per-user data
/// dir, `LimeCraft-data` next to the exe (portable mode), then TEMP. A
/// location that already holds a `launcher_state.json` wins over an empty one.
pub fn resolve_root() -> Result<PathBuf, String> {
    if let Some(r) = ROOT.get() {
        return Ok(r.clone());
    }
    let mut cands = vec![preferred_dir()];
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        cands.push(exe_dir.join("LimeCraft-data"));
    }
    cands.push(std::env::temp_dir().join("LimeCraft"));
    let writable: Vec<&PathBuf> = cands.iter().filter(|c| dir_writable(c)).collect();
    let chosen = writable
        .iter()
        .find(|c| state_path(c).exists())
        .or(writable.first())
        .map(|c| (*c).clone())
        .ok_or_else(|| {
            format!(
                "нет доступа на запись ни в одну папку данных ({}). Задайте LIMECRAFT_HOME",
                cands.iter().map(|c| c.display().to_string()).collect::<Vec<_>>().join(", ")
            )
        })?;
    let _ = ROOT.set(chosen.clone());
    Ok(chosen)
}

/// Read state file. A corrupt file is preserved as `launcher_state.json.bak`
/// instead of being silently replaced by an empty state.
pub fn load_state(root: &Path) -> LauncherState {
    let p = state_path(root);
    match std::fs::read(&p) {
        Ok(bytes) => match serde_json::from_slice::<LauncherState>(&bytes) {
            Ok(mut s) => {
                s.normalize();
                s
            }
            Err(_) => {
                let _ = std::fs::copy(&p, p.with_extension("json.bak"));
                let mut s = LauncherState::default();
                s.normalize();
                s
            }
        },
        Err(_) => {
            let mut s = LauncherState::default();
            s.normalize();
            s
        }
    }
}

/// Atomic save: write a temp file next to the state and rename over it.
pub fn save_state(root: &Path, state: &LauncherState) -> Result<(), String> {
    let p = state_path(root);
    std::fs::create_dir_all(root).map_err(|e| format!("create {}: {e}", root.display()))?;
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
}

fn clear_readonly(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if let Ok(meta) = e.metadata() {
                let mut perms = meta.permissions();
                if perms.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perms.set_readonly(false);
                    let _ = std::fs::set_permissions(&p, perms);
                }
                if meta.is_dir() {
                    clear_readonly(&p);
                }
            }
        }
    }
}

/// First file or folder under `dir` that cannot be removed (for the error text).
fn first_blocker(dir: &Path) -> Option<(PathBuf, String)> {
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(b) = first_blocker(&p) {
                return Some(b);
            }
            if let Err(err) = std::fs::remove_dir(&p) {
                return Some((p, err.to_string()));
            }
        } else if let Err(err) = std::fs::remove_file(&p) {
            return Some((p, err.to_string()));
        }
    }
    None
}

/// Delete a folder tree, retrying (antivirus/indexers and a just-closed game
/// may still hold files) and clearing read-only flags. The error names the
/// file that could not be removed.
pub fn remove_dir_robust(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let mut last = String::new();
    for attempt in 0..5u64 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(400 * attempt));
            clear_readonly(dir);
        }
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    match first_blocker(dir) {
        Some((p, e)) => Err(format!("{} ({e}). Закройте игру и повторите", p.display())),
        None => match std::fs::remove_dir(dir) {
            Ok(()) => Ok(()),
            Err(_) => Err(format!("{}: {last}", dir.display())),
        },
    }
}

/// Join an untrusted relative path onto `base`, refusing absolute paths,
/// drive prefixes and `..` components (path traversal).
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let rel_path = Path::new(rel);
    let mut out = base.to_path_buf();
    for c in rel_path.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return Err(format!("небезопасный путь: {rel}")),
        }
    }
    // `\` is a separator only on Windows; reject it elsewhere too.
    if rel.contains('\\') && rel.split('\\').any(|p| p == "..") {
        return Err(format!("небезопасный путь: {rel}"));
    }
    if rel.is_empty() || !out.starts_with(base) || out == base {
        return Err(format!("небезопасный путь: {rel}"));
    }
    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal() {
        let base = Path::new("base");
        assert!(safe_join(base, "mods/a.jar").is_ok());
        assert!(safe_join(base, "../x").is_err());
        assert!(safe_join(base, "a/../../x").is_err());
        assert!(safe_join(base, "/etc/passwd").is_err());
        assert!(safe_join(base, "C:/Windows/x").is_err() || cfg!(not(windows)));
        assert!(safe_join(base, "..\\x").is_err());
        assert!(safe_join(base, "").is_err());
    }

    #[test]
    fn robust_remove_handles_readonly() {
        let dir = std::env::temp_dir().join(format!("lc-rm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        let f = dir.join("a/b/ro.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&f, perms).unwrap();
        remove_dir_robust(&dir).unwrap();
        assert!(!dir.exists());
        assert!(remove_dir_robust(&dir).is_ok()); // already gone
    }

    #[test]
    fn old_state_gets_profiles_and_ui_defaults() {
        let old = r#"{"instances":[],"java_path":null,"ram_mb":4096,"account":{"provider":"Offline","username":"Steve"}}"#;
        let mut s: LauncherState = serde_json::from_str(old).unwrap();
        s.normalize();
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.active_account().username, "Steve");
        assert_eq!(s.ui, crate::core::prefs::UiPrefs::default());
    }

    #[test]
    fn instance_nickname_overrides_the_profile() {
        let mut s = LauncherState::default();
        s.profiles = vec![
            Account { username: "Main".into(), uuid: None, avatar: None, skin: None },
            Account { username: "Alt".into(), uuid: None, avatar: Some("a.png".into()), skin: None },
        ];
        s.normalize();
        let mut inst = Instance { name: "x".into(), ..Default::default() };
        assert_eq!(s.account_for(&inst).username, "Main");
        inst.username = Some("Alt".into());
        assert_eq!(s.account_for(&inst).avatar.as_deref(), Some("a.png"), "a known profile is used as a whole");
        inst.username = Some("Builder".into());
        assert_eq!(s.account_for(&inst).username, "Builder");
    }

    #[test]
    fn state_roundtrip_and_backup() {
        let dir = std::env::temp_dir().join(format!("lc-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut st = LauncherState::default();
        st.ram_mb = 2048;
        save_state(&dir, &st).unwrap();
        assert_eq!(load_state(&dir).ram_mb, 2048);
        std::fs::write(state_path(&dir), b"{broken").unwrap();
        assert_eq!(load_state(&dir).ram_mb, 0);
        assert!(dir.join("launcher_state.json.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
