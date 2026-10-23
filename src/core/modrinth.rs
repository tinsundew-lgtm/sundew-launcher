use serde::Deserialize;
use std::path::Path;

use super::download;

pub const API_BASE: &str = "https://api.modrinth.com/v2";

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
    pub description: String,
    pub body: Option<String>,
    pub project_type: String,
    pub icon_url: Option<String>,
    pub downloads: Option<u64>,
    pub follows: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
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
pub struct IndexFile {
    pub path: String,
    pub hashes: std::collections::HashMap<String, String>,
    pub downloads: Vec<String>,
}

pub fn search(client: &reqwest::blocking::Client, query: &str, project_type: &str, limit: u32) -> Result<Vec<SearchHit>, String> {
    let facets = format!("[[\"project_type:{project_type}\"]]");
    let url = format!(
        "{API_BASE}/search?query={}&facets={}&index=relevance&limit={}",
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
    // id may be slug or actual id
    download::get_json(client, &format!("{API_BASE}/project/{id}"))
}

pub fn get_project_versions(client: &reqwest::blocking::Client, id: &str) -> Result<Vec<Version>, String> {
    download::get_json(client, &format!("{API_BASE}/project/{id}/version"))
}

pub fn get_versions_for(client: &reqwest::blocking::Client, id: &str, game_version: &str, loaders: &[&str]) -> Result<Vec<Version>, String> {
    let game = percent_encode(&format!("[\"{game_version}\"]"));
    let loaders_enc = percent_encode(&serde_json::to_string(loaders).unwrap_or_else(|_| "[]".into()));
    download::get_json(
        client,
        &format!("{API_BASE}/project/{id}/version?game_versions={game}&loaders={loaders_enc}"),
    )
}

pub fn get_versions_post(client: &reqwest::blocking::Client, ids: &[String]) -> Result<Vec<Version>, String> {
    let body = serde_json::to_string(ids).map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{API_BASE}/versions"))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} on /versions bulk", resp.status()));
    }
    resp.json().map_err(|e| e.to_string())
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
pub fn latest_mrpack_url(client: &reqwest::blocking::Client, project_id: &str) -> Result<String, String> {
    let versions = get_project_versions(client, project_id)?;
    for v in versions {
        for f in &v.files {
            if f.filename.to_lowercase().ends_with(".mrpack") {
                return Ok(f.url.clone());
            }
        }
    }
    Err(format!("у проекта {project_id} не найдено .mrpack файлов"))
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
            use std::io::Read;
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
        // --- zip-slip guard: никогда не выходим за пределы target ---
        let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
        if parts.iter().any(|p| *p == "..") {
            return Err(format!("mrpack содержит небезопасный путь: {name}"));
        }
        let rel = parts.join("/");
        let out = target.join(&rel);
        if entry.is_dir() {
            let _ = std::fs::create_dir_all(&out);
            continue;
        }
        // top-level index already handled above; files have deeper path
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let mut buf = Vec::new();
        use std::io::Read;
        entry.read_to_end(&mut buf).map_err(|e| format!("read {}: {e}", name))?;
        std::fs::write(&out, &buf).map_err(|e| format!("write {}: {e}", out.display()))?;
    }
    Ok(index)
}
