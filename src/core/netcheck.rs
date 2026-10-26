//! Reachability check for every server the launcher talks to. Helps to tell
//! "a host is blocked or flaky in your network" apart from a launcher bug.

use std::time::{Duration, Instant};

use reqwest::blocking::Client;

use super::download::describe_err;

/// (name, url, what it is used for)
pub const HOSTS: &[(&str, &str, &str)] = &[
    ("Modrinth API", "https://api.modrinth.com/v2/", "поиск и установка модов, модпаков"),
    ("Modrinth CDN", "https://cdn.modrinth.com/", "скачивание файлов модов"),
    ("Mojang: версии", "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json", "список версий Minecraft"),
    ("Mojang: ресурсы", "https://resources.download.minecraft.net/", "ассеты игры"),
    ("Fabric meta", "https://meta.fabricmc.net/v2/versions/loader", "установка Fabric"),
    ("Fabric Maven", "https://maven.fabricmc.net/", "библиотеки Fabric"),
    ("Forge Maven", "https://maven.minecraftforge.net/", "установка Forge"),
    ("NeoForge Maven", "https://maven.neoforged.net/releases/", "установка NeoForge"),
    ("Adoptium", "https://api.adoptium.net/v3/info/available_releases", "автоскачивание Java"),
    ("Зеркало BMCLAPI", "https://bmclapi2.bangbang93.com/", "запасной сервер для Fabric/Forge"),
    ("Зеркало MCIM", "https://mod.mcimirror.top/", "запасной сервер для Modrinth"),
];

#[derive(Clone, Debug)]
pub struct CheckResult {
    pub name: String,
    pub url: String,
    pub purpose: String,
    /// True when the server answered with any HTTP response (even 404/405).
    pub reachable: bool,
    pub status: Option<u16>,
    pub millis: u128,
    pub detail: String,
}

fn check_one(client: &Client, name: &str, url: &str, purpose: &str) -> CheckResult {
    let t = Instant::now();
    match client.get(url).timeout(Duration::from_secs(8)).send() {
        Ok(r) => {
            let status = r.status().as_u16();
            CheckResult {
                name: name.into(),
                url: url.into(),
                purpose: purpose.into(),
                reachable: true,
                status: Some(status),
                millis: t.elapsed().as_millis(),
                detail: String::new(),
            }
        }
        Err(e) => CheckResult {
            name: name.into(),
            url: url.into(),
            purpose: purpose.into(),
            reachable: false,
            status: None,
            millis: t.elapsed().as_millis(),
            detail: describe_err(&e),
        },
    }
}

/// Check all hosts in parallel (≈ the slowest one, at most 8 s).
pub fn check_all(client: &Client) -> Vec<CheckResult> {
    let results = std::sync::Mutex::new(Vec::<(usize, CheckResult)>::new());
    std::thread::scope(|s| {
        for (i, (name, url, purpose)) in HOSTS.iter().enumerate() {
            let results = &results;
            s.spawn(move || {
                let r = check_one(client, name, url, purpose);
                results.lock().unwrap_or_else(|e| e.into_inner()).push((i, r));
            });
        }
    });
    let mut v = results.into_inner().unwrap_or_default();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_list_is_sane() {
        assert!(HOSTS.iter().all(|(_, u, _)| u.starts_with("https://")));
        assert!(HOSTS.iter().any(|(n, _, _)| n.contains("Modrinth API")));
    }

    #[test]
    fn unreachable_host_reports_cause() {
        // nothing listens on this port
        let c = reqwest::blocking::Client::new();
        let r = check_one(&c, "x", "http://127.0.0.1:9/", "test");
        assert!(!r.reachable && r.status.is_none());
        assert!(!r.detail.is_empty());
    }
}
