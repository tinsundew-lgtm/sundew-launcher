use std::fmt;

use serde::{Deserialize, Serialize};

/// Какой способ авторизации использует аккаунт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthProvider {
    /// Офлайн-режим: только ник, UUID выводится из ника.
    #[default]
    Offline,
    /// Заготовка под Microsoft-логин (пока не реализовано в этом сборке).
    Microsoft,
}

impl fmt::Display for AuthProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthProvider::Offline => write!(f, "Offline"),
            AuthProvider::Microsoft => write!(f, "Microsoft"),
        }
    }
}

/// Учётная запись лаунчера. Хранится в launcher_state.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Account {
    pub provider: AuthProvider,
    pub username: String,
    /// Заполняется для Microsoft-аккаунта (после реализации OAuth).
    #[serde(default)]
    pub uuid: Option<String>,
    /// Произвольный access token (для Microsoft — токен XSTS/Minecraft).
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

impl Account {
    /// Офлайн-аккаунт по умолчанию: стандартный ник «Player».
    pub fn offline_default() -> Self {
        Self {
            provider: AuthProvider::Offline,
            username: "Player".to_string(),
            uuid: None,
            access_token: None,
            refresh_token: None,
        }
    }

    /// UUID, передаваемый в аргументы запуска игры.
    pub fn resolved_uuid(&self) -> String {
        if let Some(u) = &self.uuid {
            if !u.is_empty() {
                return u.clone();
            }
        }
        offline_uuid(&self.username)
    }

    /// Токен для запуска: офлайн — «0», иначе сохранённый access token.
    pub fn resolved_token(&self) -> String {
        self.access_token
            .as_deref()
            .filter(|t| !t.is_empty())
            .unwrap_or("0")
            .to_string()
    }
}

/// Детерминированный офлайн-UUID по алгоритму Minecraft/bungee:
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
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_stable_and_v3() {
        let a = offline_uuid("Player");
        let b = offline_uuid("Player");
        assert_eq!(a, b);
        // Формат 8-4-4-4-12
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        // Версия = '3' в 13-м символе
        assert_eq!(&a[14..15], "3");
        // Известное значение для Player: 20df558d-8a7d-3b6b-8f5c-6e34a4f4e... — просто проверяем стабильность.
        // Ник пустой -> даём осмысленный uuid
        let c = offline_uuid("");
        assert_eq!(c.split('-').count(), 5);
    }
}
