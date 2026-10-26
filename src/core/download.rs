use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};

use reqwest::blocking::Client;

/// Mark a downloaded file as executable (installers/binaries on Linux/macOS).
/// No-op on Windows where exec is implied by the loader.
#[cfg(unix)]
pub fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
    let mut perms = meta.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| format!("chmod +x {}: {e}", path.display()))
}

#[cfg(not(unix))]
pub fn set_executable(_path: &Path) -> Result<(), String> {
    // Windows: exec is implied by the PE loader, nothing to do.
    Ok(())
}

pub struct DownloadProgress {
    pub current: AtomicU64,
    pub total: AtomicU64,
    pub file: String,
    pub cancelled: AtomicBool,
}

impl DownloadProgress {
    pub fn new(file: impl Into<String>) -> Self {
        Self {
            current: AtomicU64::new(0),
            total: AtomicU64::new(0),
            file: file.into(),
            cancelled: AtomicBool::new(false),
        }
    }
}

const BMCL: &str = "https://bmclapi2.bangbang93.com";
const MCIM: &str = "https://mod.mcimirror.top";

/// Timeout for small API requests (JSON); downloads use the client's long timeout.
pub const API_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);

static MIRRORS_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Allow/forbid falling back to community mirrors when a main host is unreachable.
pub fn set_mirrors_enabled(on: bool) {
    MIRRORS_ENABLED.store(on, Ordering::Relaxed);
}

pub fn mirrors_enabled() -> bool {
    MIRRORS_ENABLED.load(Ordering::Relaxed)
}

/// Hosts that failed to connect during this run; they are tried last.
static DEAD_HOSTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("").to_lowercase()
}

fn mark_dead(url: &str) {
    if let Ok(mut d) = DEAD_HOSTS.lock() {
        let h = host_of(url);
        if !d.contains(&h) {
            d.push(h);
        }
    }
}

fn is_dead(url: &str) -> bool {
    DEAD_HOSTS.lock().map(|d| d.contains(&host_of(url))).unwrap_or(false)
}

/// Alternative URLs for hosts that are often unreachable (blocked CDNs):
/// the BMCLAPI mirror for Fabric/Forge/NeoForge and Maven Central for plain
/// Maven artifacts.
pub fn mirror_urls(url: &str) -> Vec<String> {
    let mut out = Vec::new();
    let rules: [(&str, &str); 4] = [
        ("https://meta.fabricmc.net/", "/fabric-meta/"),
        ("https://maven.fabricmc.net/", "/maven/"),
        ("https://maven.minecraftforge.net/", "/maven/"),
        ("https://maven.neoforged.net/releases/", "/maven/"),
    ];
    for (prefix, mirror) in rules {
        if let Some(rest) = url.strip_prefix(prefix) {
            out.push(format!("{BMCL}{mirror}{rest}"));
            if prefix.contains("maven.fabricmc.net") {
                out.push(format!("https://repo1.maven.org/maven2/{rest}"));
            }
        }
    }
    // Modrinth: API (search, projects, versions, hashes) and the file CDN
    if let Some(rest) = url.strip_prefix("https://api.modrinth.com/v2/") {
        out.push(format!("{MCIM}/modrinth/v2/{rest}"));
    }
    if let Some(rest) = url.strip_prefix("https://cdn.modrinth.com/") {
        out.push(format!("{MCIM}/{rest}"));
    }
    out
}

/// The URL itself plus its mirrors; hosts known to be down go last.
fn candidates(url: &str) -> Vec<String> {
    let mut v = vec![url.to_string()];
    if mirrors_enabled() {
        v.extend(mirror_urls(url));
    }
    if is_dead(url) {
        v.rotate_left(1);
    }
    v
}

/// Remember hosts we cannot even connect to, so later requests skip the wait.
fn note_failure(url: &str, e: &reqwest::Error) {
    if e.is_connect() || e.is_timeout() {
        mark_dead(url);
    }
}

/// Download a single file to `dest`, ensuring parent dir exists.
/// Optionally verifies sha1 hash.
pub fn download_file(
    client: &Client,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
) -> Result<(), String> {
    download_file_ex(client, url, dest, sha1_hex, None, None)
}

/// Full-featured download: skips when `dest` already matches `sha1_hex`,
/// retries transient failures (3 attempts), honours `cancel`, verifies sha1
/// before the final rename.
pub fn download_file_ex(
    client: &Client,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
    cancel: Option<&AtomicBool>,
    prog: Option<&Arc<DownloadProgress>>,
) -> Result<(), String> {
    let expected = sha1_hex.map(|h| h.to_lowercase());
    if let Some(expected) = &expected {
        if dest.exists() {
            if let Ok(h) = file_sha1(dest) {
                if &h == expected {
                    return Ok(());
                }
            }
        }
    }
    let mut last_err = String::new();
    for cand in candidates(url) {
        for attempt in 0..2 {
            if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                return Err("Cancelled".into());
            }
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            match download_once(client, &cand, dest, expected.as_deref(), cancel, prog) {
                Ok(()) => return Ok(()),
                Err(e) if e == "Cancelled" => return Err(e),
                Err(e) => {
                    let connect_fail = is_dead(&cand);
                    last_err = e;
                    if connect_fail {
                        break; // no point retrying a host we cannot reach
                    }
                }
            }
        }
    }
    Err(last_err)
}

fn download_once(
    client: &Client,
    url: &str,
    dest: &Path,
    expected_sha1: Option<&str>,
    cancel: Option<&AtomicBool>,
    prog: Option<&Arc<DownloadProgress>>,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    let tmp = tmp_path(dest);
    let _ = std::fs::remove_file(&tmp);
    let mut resp = client.get(url).send().map_err(|e| {
        note_failure(url, &e);
        format!("GET {url}: {}", describe_err(&e))
    })?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {}", resp.status(), url));
    }
    let total = resp.content_length().unwrap_or(0);
    // Write into a `.part` temp next to dest so we never leave a truncated
    // final file behind.
    let write_result: Result<(), String> = (|| {
        let mut tmp_file = File::create(&tmp).map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
        let mut buf = [0u8; 16384];
        let mut got: u64 = 0;
        loop {
            if cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false)
                || prog.map(|p| p.cancelled.load(Ordering::Relaxed)).unwrap_or(false)
            {
                return Err("Cancelled".into());
            }
            if let Some(p) = prog {
                p.total.store(total, Ordering::Relaxed);
                p.current.store(got, Ordering::Relaxed);
            }
            let n = resp
                .read(&mut buf)
                .map_err(|e| format!("Stream error while downloading {url}: {e}"))?;
            if n == 0 {
                break;
            }
            tmp_file.write_all(&buf[..n]).map_err(|e| format!("Write error on {}: {e}", tmp.display()))?;
            got += n as u64;
        }
        tmp_file.flush().map_err(|e| format!("flush {}: {e}", tmp.display()))?;
        if let Some(p) = prog {
            p.current.store(got, Ordering::Relaxed);
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Some(expected) = expected_sha1 {
        let actual = file_sha1(&tmp).map_err(|e| format!("sha1 check for {url}: {e}"))?;
        if actual != expected {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("sha1 mismatch for {url}: expected {expected}, got {actual}"));
        }
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        // A concurrent download of the same file may have finalized it first.
        if !dest.exists() {
            return Err(format!("finalize {}: {e}", dest.display()));
        }
    }
    Ok(())
}

pub fn tmp_path(dest: &Path) -> std::path::PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    // Unique per call: the same asset hash can be queued twice and downloaded
    // concurrently, and a shared temp file made one thread delete the other's.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    name.push(format!(".{}.{}.part", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    dest.with_file_name(name)
}

pub fn file_sha1(path: &Path) -> Result<String, String> {
    use sha1::{Digest, Sha1};
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 32768];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn file_sha512(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha512};
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha512::new();
    let mut buf = [0u8; 32768];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn file_sha256(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 32768];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// The SHA-1 a Maven repository publishes next to an artifact (`<url>.sha1`).
pub fn maven_sha1(client: &Client, artifact_url: &str) -> Result<String, String> {
    let text = get_text(client, &format!("{artifact_url}.sha1"))?;
    let h = text.split_whitespace().next().unwrap_or("").to_lowercase();
    if h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(h)
    } else {
        Err(format!("неверная контрольная сумма для {artifact_url}"))
    }
}

/// Download url to dest with a shared progress sink (Arc<DownloadProgress>).
pub fn download_file_with_progress(
    client: &Client,
    url: &str,
    dest: &Path,
    prog: Option<&Arc<DownloadProgress>>,
) -> Result<(), String> {
    download_file_ex(client, url, dest, None, None, prog)
}

/// Error text including the whole cause chain (reqwest's top-level message
/// is just "error sending request"; the useful part is the source).
pub fn describe_err(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(x) = src {
        out.push_str(&format!(" ← {x}"));
        src = x.source();
    }
    out
}

/// GET with retries; falls back to mirrors when the host is unreachable.
/// HTTP error statuses other than 5xx/429 are returned immediately.
fn get_with_retry(client: &Client, url: &str) -> Result<reqwest::blocking::Response, String> {
    let mut last = String::new();
    for cand in candidates(url) {
        for attempt in 0..2u64 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(700));
            }
            match client.get(&cand).timeout(API_TIMEOUT).send() {
                Ok(r) if r.status().is_server_error() || r.status().as_u16() == 429 => {
                    last = format!("HTTP {} for {cand}", r.status());
                }
                Ok(r) => return Ok(r),
                Err(e) => {
                    last = format!("GET {cand}: {}", describe_err(&e));
                    note_failure(&cand, &e);
                    if is_dead(&cand) {
                        break;
                    }
                }
            }
        }
    }
    Err(last)
}

/// Very small blocking fetch of JSON.
pub fn get_json<T: serde::de::DeserializeOwned>(client: &Client, url: &str) -> Result<T, String> {
    let resp = get_with_retry(client, url)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.json::<T>().map_err(|e| format!("JSON parse of {url}: {}", describe_err(&e)))
}

/// POST a JSON body and parse a JSON answer (retries, mirrors when unreachable).
pub fn post_json<T: serde::de::DeserializeOwned>(client: &Client, url: &str, body: &serde_json::Value) -> Result<T, String> {
    let mut last = String::new();
    for cand in candidates(url) {
        for attempt in 0..2u64 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(700));
            }
            match client.post(&cand).timeout(API_TIMEOUT).json(body).send() {
                Ok(r) if r.status().is_success() => {
                    return r.json::<T>().map_err(|e| format!("JSON parse of {cand}: {}", describe_err(&e)));
                }
                Ok(r) if r.status().is_server_error() || r.status().as_u16() == 429 => last = format!("HTTP {} for {cand}", r.status()),
                Ok(r) => return Err(format!("HTTP {} for {cand}", r.status())),
                Err(e) => {
                    last = format!("POST {cand}: {}", describe_err(&e));
                    note_failure(&cand, &e);
                    if is_dead(&cand) {
                        break;
                    }
                }
            }
        }
    }
    Err(last)
}

pub fn get_text(client: &Client, url: &str) -> Result<String, String> {
    let resp = get_with_retry(client, url)?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.text().map_err(|e| format!("Text read of {url}: {}", describe_err(&e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_for_blocked_hosts() {
        let m = mirror_urls("https://meta.fabricmc.net/v2/versions/loader/26.3");
        assert_eq!(m, vec!["https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader/26.3"]);
        let m = mirror_urls("https://maven.fabricmc.net/net/fabricmc/x/1/x-1.jar");
        assert_eq!(m[0], "https://bmclapi2.bangbang93.com/maven/net/fabricmc/x/1/x-1.jar");
        assert_eq!(m[1], "https://repo1.maven.org/maven2/net/fabricmc/x/1/x-1.jar");
        assert!(mirror_urls("https://piston-meta.mojang.com/x").is_empty());
        assert_eq!(
            mirror_urls("https://api.modrinth.com/v2/search?query=a"),
            vec!["https://mod.mcimirror.top/modrinth/v2/search?query=a"]
        );
        assert_eq!(
            mirror_urls("https://cdn.modrinth.com/data/X/versions/Y/f.jar"),
            vec!["https://mod.mcimirror.top/data/X/versions/Y/f.jar"]
        );
    }
}
