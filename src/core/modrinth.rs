use serde::Deserialize;
use std::io::Read;
use std::path::Path;

use super::download;

pub const DEFAULT_API_BASE: &str = "https://api.modrinth.com/v2";

/// Modrinth API base URL. `LIMECRAFT_MODRINTH_API` overrides it (tests, mirrors).
pub fn api_base() -> String {
    std::env::var("LIMECRAFT_MODRINTH_API")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchHit {
    pub project_id: String,
    pub project_type: String,
    pub slug: Option<String>,
    pub author: String,
    pub title: String,
    pub description: String,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
    pub versions: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub total_hits: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProjectFile {
    pub url: String,
    pub filename: String,
    pub primary: Option<bool>,
    #[serde(default)]
    pub hashes: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub version_type: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub files: Vec<ProjectFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Project {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub body: Option<String>,
    pub project_type: String,
    pub icon_url: Option<String>,
    pub downloads: Option<u64>,
    pub follows: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthIndex {
    pub format_version: u64,
    pub game: Option<String>,
    pub version_id: Option<String>,
    pub name: Option<String>,
    pub files: Vec<IndexFile>,
    pub dependencies: Option<std::collections::HashMap<String, String>>,
    pub overrides: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IndexFile {
    pub path: String,
    pub hashes: std::collections::HashMap<String, String>,
    pub downloads: Vec<String>,
    #[serde(default)]
    pub env: Option<FileEnv>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FileEnv {
    pub client: Option<String>,
    pub server: Option<String>,
}

impl IndexFile {
    /// Server-only files (`env.client = unsupported`) must not land on a client.
    pub fn client_supported(&self) -> bool {
        self.env.as_ref().and_then(|e| e.client.as_deref()) != Some("unsupported")
    }
}

/// Hosts the Modrinth spec allows for pack file downloads.
pub fn is_allowed_download_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_lowercase();
    ["cdn.modrinth.com", "github.com", "raw.githubusercontent.com", "gitlab.com"]
        .iter()
        .any(|h| host == *h)
}

/// Max size of a single extracted archive entry (zip-bomb guard).
const MAX_ENTRY_SIZE: u64 = 2 * 1024 * 1024 * 1024;

pub fn search(client: &reqwest::blocking::Client, query: &str, project_type: &str, limit: u32) -> Result<Vec<SearchHit>, String> {
    let base = api_base();
    let facets = format!("[[\"project_type:{project_type}\"]]");
    let url = format!(
        "{base}/search?query={}&facets={}&index=relevance&limit={}",
        percent_encode(query),
        percent_encode(&facets),
        limit
    );
    let resp: SearchResponse = download::get_json(client, &url)?;
    Ok(resp.hits)
}

fn percent_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

pub fn get_project(client: &reqwest::blocking::Client, id: &str) -> Result<Project, String> {
    let base = api_base();
    // id may be slug or actual id
    download::get_json(client, &format!("{base}/project/{id}"))
}

pub fn get_project_versions(client: &reqwest::blocking::Client, id: &str) -> Result<Vec<Version>, String> {
    let base = api_base();
    download::get_json(client, &format!("{base}/project/{id}/version"))
}

pub fn get_versions_for(client: &reqwest::blocking::Client, id: &str, game_version: &str, loaders: &[&str]) -> Result<Vec<Version>, String> {
    let base = api_base();
    let game = percent_encode(&format!("[\"{game_version}\"]"));
    let loaders_enc = percent_encode(&serde_json::to_string(loaders).unwrap_or_else(|_| "[]".into()));
    download::get_json(
        client,
        &format!("{base}/project/{id}/version?game_versions={game}&loaders={loaders_enc}"),
    )
}

pub fn get_versions_post(client: &reqwest::blocking::Client, ids: &[String]) -> Result<Vec<Version>, String> {
    let base = api_base();
    let body = serde_json::to_string(ids).map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{base}/versions"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} on /versions bulk", resp.status()));
    }
    resp.json().map_err(|e| e.to_string())
}

/// Search with compatibility filters (game version / loader). An empty query
/// browses by downloads, like the Modrinth site does.
pub fn search_filtered(
    client: &reqwest::blocking::Client,
    query: &str,
    project_type: &str,
    game_version: Option<&str>,
    loaders: &[&str],
    limit: u32,
    offset: u32,
) -> Result<Vec<SearchHit>, String> {
    let base = api_base();
    let mut facets = vec![serde_json::json!([format!("project_type:{project_type}")])];
    if let Some(v) = game_version.filter(|v| !v.is_empty() && *v != "latest") {
        facets.push(serde_json::json!([format!("versions:{v}")]));
    }
    if !loaders.is_empty() {
        // any of the loaders (OR inside one facet group)
        facets.push(serde_json::Value::Array(loaders.iter().map(|l| format!("categories:{l}").into()).collect()));
    }
    let index = if query.trim().is_empty() { "downloads" } else { "relevance" };
    let url = format!(
        "{base}/search?query={}&facets={}&index={index}&limit={limit}&offset={offset}",
        percent_encode(query.trim()),
        percent_encode(&serde_json::Value::Array(facets).to_string()),
    );
    let resp: SearchResponse = download::get_json(client, &url)?;
    Ok(resp.hits)
}

/// Fetch one version by id.
pub fn get_version(client: &reqwest::blocking::Client, id: &str) -> Result<Version, String> {
    let base = api_base();
    download::get_json(client, &format!("{base}/version/{id}"))
}

/// Fetch several projects at once.
pub fn get_projects(client: &reqwest::blocking::Client, ids: &[String]) -> Result<Vec<Project>, String> {
    let base = api_base();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let list = percent_encode(&serde_json::to_string(ids).map_err(|e| e.to_string())?);
    download::get_json(client, &format!("{base}/projects?ids={list}"))
}

/// Identify files by SHA-1: `hash -> Version` for every file Modrinth knows.
pub fn versions_by_hashes(
    client: &reqwest::blocking::Client,
    hashes: &[String],
) -> Result<std::collections::HashMap<String, Version>, String> {
    let base = api_base();
    if hashes.is_empty() {
        return Ok(Default::default());
    }
    let body = serde_json::json!({ "hashes": hashes, "algorithm": "sha1" });
    download::post_json(client, &format!("{base}/version_files"), &body)
}

/// Latest compatible version for each hash: `hash -> newest Version`.
pub fn latest_versions_for_hashes(
    client: &reqwest::blocking::Client,
    hashes: &[String],
    loaders: &[&str],
    game_versions: &[&str],
) -> Result<std::collections::HashMap<String, Version>, String> {
    let base = api_base();
    if hashes.is_empty() {
        return Ok(Default::default());
    }
    let body = serde_json::json!({
        "hashes": hashes, "algorithm": "sha1",
        "loaders": loaders, "game_versions": game_versions,
    });
    download::post_json(client, &format!("{base}/version_files/update"), &body)
}

/// Download the raw .mrpack file into `dest`.
pub fn download_mrpack(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> Result<(), String> {
    let parent = dest.parent().ok_or_else(|| format!("dest {} has no parent dir", dest.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    download::download_file(client, url, dest, None)
}

/// Resolve the actual .mrpack download URL for a modpack project.
///
/// The convenient `.../versions/latest.mrpack` CDN alias is not reliable
/// (it can answer 400), so we query the project versions through the API and
/// take the newest version that ships a `.mrpack` file.
pub fn latest_mrpack_url(client: &reqwest::blocking::Client, project_id: &str) -> Result<MrpackFile, String> {
    let versions = get_project_versions(client, project_id)?;
    for v in versions {
        for f in &v.files {
            if f.filename.to_lowercase().ends_with(".mrpack") {
                return Ok(MrpackFile {
                    url: f.url.clone(),
                    sha1: f.hashes.get("sha1").map(|h| h.to_lowercase()),
                    sha512: f.hashes.get("sha512").map(|h| h.to_lowercase()),
                });
            }
        }
    }
    Err(format!("у проекта {project_id} не найдено .mrpack файлов"))
}

/// A modpack archive and the hashes Modrinth publishes for it.
#[derive(Debug, Clone)]
pub struct MrpackFile {
    pub url: String,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

/// Extract a .mrpack (zip) into target dir. .mrpack is a zip with
/// modrinth.index.json at root plus files/ at top-level paths and `overrides/`.
pub fn extract_mrpack(mrpack: &Path, target: &Path) -> Result<ModrinthIndex, String> {
    let file = std::fs::File::open(mrpack).map_err(|e| format!("open mrpack: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("not a valid zip: {e}"))?;
    let mut index: Option<ModrinthIndex> = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("read zip entry #{i}: {e}"))?;
        let name = entry.name().to_string();
        if name == "modrinth.index.json" {
            let mut content = String::new();
            entry.read_to_string(&mut content).map_err(|e| format!("read modrinth.index.json: {e}"))?;
            index = Some(serde_json::from_str(&content).map_err(|e| format!("index.json parse: {e}"))?);
            break;
        }
    }
    let index = index.ok_or("modrinth.index.json not found in archive")?;
    drop(archive);

    // extract every file from the zip, keeping relative names
    let file = std::fs::File::open(mrpack).map_err(|e| format!("reopen mrpack: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("reopen mrpack: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("read zip entry #{i}: {e}"))?;
        let name = entry.name().to_string();
        // --- zip-slip guard: enclosed_name rejects `..`, absolute paths and
        // drive prefixes for both `/` and `\\` separators ---
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!("mrpack содержит небезопасный путь: {name}"));
        };
        let out = target.join(&rel);
        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&out);
            continue;
        }
        if entry.size() > MAX_ENTRY_SIZE {
            return Err(format!("mrpack: файл {name} слишком большой"));
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut dst = std::fs::File::create(&out).map_err(|e| format!("write {}: {e}", out.display()))?;
        std::io::copy(&mut (&mut entry).take(MAX_ENTRY_SIZE), &mut dst)
            .map_err(|e| format!("write {}: {e}", out.display()))?;
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mrpack_index_camel_case() {
        let idx: ModrinthIndex = serde_json::from_str(
            r#"{"formatVersion":1,"game":"minecraft","versionId":"1.0","name":"P",
            "files":[{"path":"mods/a.jar","hashes":{"sha1":"x"},"downloads":["https://cdn.modrinth.com/a.jar"],
            "env":{"client":"unsupported","server":"required"},"fileSize":10}],
            "dependencies":{"minecraft":"1.21.1","fabric-loader":"0.16.9"}}"#,
        )
        .unwrap();
        assert_eq!(idx.format_version, 1);
        assert_eq!(idx.version_id.as_deref(), Some("1.0"));
        assert!(!idx.files[0].client_supported());
        assert_eq!(idx.files[0].file_size, Some(10));
    }

    #[test]
    fn download_host_allowlist() {
        assert!(is_allowed_download_url("https://cdn.modrinth.com/data/x/a.jar"));
        assert!(!is_allowed_download_url("http://cdn.modrinth.com/a.jar"));
        assert!(!is_allowed_download_url("https://evil.com/a.jar"));
        assert!(!is_allowed_download_url("https://cdn.modrinth.com@evil.com/a.jar"));
    }
}
