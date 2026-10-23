use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};

use reqwest::blocking::Client;

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

/// Download a single file to `dest`, ensuring parent dir exists.
/// Optionally verifies sha1 hash. Reports progress into `prog` (if any).
pub fn download_file(
    client: &Client,
    url: &str,
    dest: &Path,
    sha1_hex: Option<&str>,
) -> Result<(), String> {
    if let Some(expected) = sha1_hex {
        if dest.exists() {
            if let Ok(h) = file_sha1(dest) {
                if h == expected {
                    return Ok(());
                }
            }
        }
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    tmp_path(dest);
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {}", resp.status(), url));
    }
    let mut resp = resp;
    let mut tmp = File::create(tmp_path(dest)).map_err(|e| format!("Cannot write {}: {e}", tmp_path(dest).display()))?;
    let mut buf = [0u8; 8192];
    loop {
        let n = resp
            .read(&mut buf)
            .map_err(|e| format!("Stream error while downloading {}: {e}", url))?;
        if n == 0 {
            break;
        }
        tmp.write_all(&buf[..n]).map_err(|e| format!("Write error on {}: {e}", tmp_path(dest).display()))?;
    }
    tmp.flush().ok();
    drop(tmp);
    // verify sha1 after full write if requested
    if let Some(expected) = sha1_hex {
        let part = tmp_path(dest);
        if !part.exists() {
            return Err(format!("sha1 check for {}: temp file missing after download", url));
        }
        let actual = file_sha1(&part).unwrap_or_default();
        if actual != expected {
            let _ = std::fs::remove_file(&part);
            return Err(format!("sha1 mismatch for {}: expected {expected}, got {actual}", url));
        }
    }
    rename_ok(dest);
    Ok(())
}

pub fn tmp_path(dest: &Path) -> std::path::PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

pub fn rename_ok(dest: &Path) {
    let tmp = tmp_path(dest);
    if tmp.exists() {
        let _ = std::fs::rename(&tmp, dest);
    }
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

/// Download url to dest with a shared progress sink (Arc<DownloadProgress>).
pub fn download_file_with_progress(
    client: &Client,
    url: &str,
    dest: &Path,
    prog: Option<&Arc<DownloadProgress>>,
) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    tmp_path(dest);
    let resp = client.get(url).send().map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {}", resp.status(), url));
    }
    let mut resp = resp;
    let total = resp.content_length().unwrap_or(0);
    let mut tmp = File::create(tmp_path(dest)).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 8192];
    let mut got: u64 = 0;
    loop {
        if let Some(p) = prog {
            if p.cancelled.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(tmp_path(dest));
                return Err("Cancelled".into());
            }
            p.total.store(total, Ordering::Relaxed);
            p.current.store(got, Ordering::Relaxed);
        }
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        tmp.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
    }
    tmp.flush().ok();
    drop(tmp);
    rename_ok(dest);
    if let Some(p) = prog {
        p.current.store(got, Ordering::Relaxed);
    }
    Ok(())
}

/// Very small blocking fetch of JSON.
pub fn get_json<T: serde::de::DeserializeOwned>(client: &Client, url: &str) -> Result<T, String> {
    let resp = client.get(url).send().map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.json::<T>().map_err(|e| format!("JSON parse of {url}: {e}"))
}

pub fn get_text(client: &Client, url: &str) -> Result<String, String> {
    let resp = client.get(url).send().map_err(|e| format!("GET {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    resp.text().map_err(|e| format!("Text read of {url}: {e}"))
}
