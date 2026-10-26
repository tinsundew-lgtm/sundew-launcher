//! Offline player profile: a nickname and the UUID Minecraft derives from it.
//! There is no Microsoft sign-in: the game runs in offline mode (singleplayer,
//! LAN and servers with `online-mode=false`).

use serde::{Deserialize, Serialize};

/// Учётная запись лаунчера (офлайн). Хранится в launcher_state.json.
/// Старые поля (provider, токены) из прежних версий просто игнорируются.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Account {
    pub username: String,
    /// Фиксированный UUID (необязательно); иначе выводится из ника.
    #[serde(default)]
    pub uuid: Option<String>,
    /// Аватар: имя файла в `<data>/profiles/`.
    #[serde(default)]
    pub avatar: Option<String>,
    /// Skin of the library (`skins::Skin::id`) shown in the game by CustomSkinLoader.
    #[serde(default)]
    pub skin: Option<String>,
}

impl Account {
    /// Аккаунт по умолчанию: ник «Player».
    pub fn offline_default() -> Self {
        Self { username: "Player".to_string(), uuid: None, avatar: None, skin: None }
    }

    /// UUID, передаваемый в аргументы запуска игры.
    pub fn resolved_uuid(&self) -> String {
        match &self.uuid {
            Some(u) if !u.is_empty() => u.clone(),
            _ => offline_uuid(&self.username),
        }
    }
}

/// Minecraft accepts 3–16 characters: Latin letters, digits and `_`.
pub fn valid_nickname(name: &str) -> bool {
    (3..=16).contains(&name.chars().count()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What the game needs to start.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchAuth {
    pub username: String,
    pub uuid: String,
    /// Always `"0"`: offline mode has no access token.
    pub token: String,
}

/// Credentials for an offline launch.
pub fn launch_auth(account: &Account) -> Result<LaunchAuth, String> {
    if !valid_nickname(&account.username) {
        return Err(format!(
            "Ник «{}» не подходит: нужно 3–16 символов, латиница, цифры и _ (Настройки → Учётная запись)",
            account.username
        ));
    }
    Ok(LaunchAuth { username: account.username.clone(), uuid: account.resolved_uuid(), token: "0".to_string() })
}

/// Детерминированный офлайн-UUID по алгоритму Minecraft:
/// UUID v3 (MD5) от строки "OfflinePlayer:<name>", версия 3, вариант RFC 4122.
pub fn offline_uuid(name: &str) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(format!("OfflinePlayer:{name}").as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest);
    // Установить версию (v3) и вариант (RFC 4122).
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_stable_and_v3() {
        let a = offline_uuid("Player");
        assert_eq!(a, offline_uuid("Player"));
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), vec![8, 4, 4, 4, 12]);
        assert_eq!(&a[14..15], "3", "version 3");
        assert_ne!(offline_uuid("Player"), offline_uuid("player"), "case matters, like in Minecraft");
    }

    #[test]
    fn nickname_rules() {
        assert!(valid_nickname("Steve_01"));
        assert!(!valid_nickname("ab"));
        assert!(!valid_nickname("seventeen_chars_x"));
        assert!(!valid_nickname("Стив"));
        assert!(!valid_nickname("with space"));
    }

    #[test]
    fn launch_is_offline_with_nickname_uuid() {
        let mut acc = Account::offline_default();
        acc.username = "Steve_01".into();
        let a = launch_auth(&acc).unwrap();
        assert_eq!(a.token, "0");
        assert_eq!(a.uuid, offline_uuid("Steve_01"));
        acc.username = "x".into();
        assert!(launch_auth(&acc).unwrap_err().contains("3–16"));
    }

    #[test]
    fn old_state_files_still_load() {
        // a state written by the Microsoft-login versions
        let old = r#"{"provider":"Microsoft","username":"Notch","uuid":null,"access_token":"x"}"#;
        let a: Account = serde_json::from_str(old).unwrap();
        assert_eq!(a.username, "Notch");
    }
}
