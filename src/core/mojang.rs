use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::download;

pub const VERSION_MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Deserialize, Clone)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String, // release / snapshot / old_beta / old_alpha
    pub url: String,
    pub time: Option<String>,
    /// SHA-1 of the version json (manifest v2).
    #[serde(default)]
    pub sha1: Option<String>,
}

/// Фильтр списка версий для UI (Пункт 2): по умолчанию — только релизы,
/// с `include_snapshots = true` добавляются снапшоты. Беты/альфы не показываем.
pub fn filter_versions<'a>(entries: &'a [VersionEntry], include_snapshots: bool) -> Vec<&'a VersionEntry> {
    entries
        .iter()
        .filter(|v| v.kind == "release" || include_snapshots && v.kind == "snapshot")
        .collect()
}

#[derive(Debug, Deserialize, Clone)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(default, deserialize_with = "main_class_string")]
    pub main_class: Option<String>,
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub downloads: Option<GameDownloads>,
    /// Legacy asset index id (e.g. "1.21"), also `assets` in the json.
    pub assets: Option<String>,
    /// Structured asset_index metadata from the version json.
    #[serde(default)]
    pub asset_index: Option<AssetIndex>,
    pub arguments: Option<Arguments>,
    /// Pre-1.13 versions keep game arguments as one string.
    pub minecraft_arguments: Option<String>,
    pub java_version: Option<JavaVersion>,
}

impl VersionJson {
    /// Merge a loader profile (`self`) with its `inheritsFrom` parent, the way
    /// every launcher does: child values win, libraries/arguments are combined
    /// (child first so loader classes shadow vanilla ones).
    pub fn merged_with(&self, parent: &VersionJson) -> VersionJson {
        let mut out = parent.clone();
        out.id = self.id.clone();
        out.inherits_from = None;
        if self.main_class.is_some() {
            out.main_class = self.main_class.clone();
        }
        let mut libs = self.libraries.clone();
        for l in &parent.libraries {
            if !libs.iter().any(|x| x.name == l.name) {
                libs.push(l.clone());
            }
        }
        out.libraries = libs;
        if self.assets.is_some() {
            out.assets = self.assets.clone();
        }
        if self.asset_index.is_some() {
            out.asset_index = self.asset_index.clone();
        }
        if self.downloads.is_some() {
            out.downloads = self.downloads.clone();
        }
        if self.java_version.is_some() {
            out.java_version = self.java_version.clone();
        }
        if self.minecraft_arguments.is_some() {
            out.minecraft_arguments = self.minecraft_arguments.clone();
        }
        if let Some(ca) = &self.arguments {
            let pa = parent.arguments.clone().unwrap_or_default();
            out.arguments = Some(Arguments {
                game: concat_args(pa.game, ca.game.clone()),
                jvm: concat_args(pa.jvm, ca.jvm.clone()),
            });
        }
        out
    }
}

fn concat_args(a: Option<serde_json::Value>, b: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(serde_json::Value::Array(mut x)), Some(serde_json::Value::Array(y))) => {
            x.extend(y);
            Some(serde_json::Value::Array(x))
        }
        (x, _) => x,
    }
}

/// The `assetIndex` object in a version json: where to fetch the asset index file.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndex {
    pub id: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
    #[serde(default)]
    pub total_size: Option<u64>,
}

/// Parsed content of an assets index json (`{ "objects": { ... } }`).
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AssetIndexFile {
    pub objects: std::collections::HashMap<String, AssetObject>,
}

/// One object inside the asset index.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AssetObject {
    pub hash: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Arguments {
    pub game: Option<serde_json::Value>,
    pub jvm: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub major_version: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GameDownloads {
    pub client: Option<DownloadFile>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DownloadFile {
    pub url: String,
    pub sha1: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    pub rules: Option<Vec<Rule>>,
    pub natives: Option<std::collections::HashMap<String, String>>,
    pub classifiers: Option<std::collections::HashMap<String, String>>,
    /// Maven remote base URL (Fabric/Quilt profile json keeps libs as
    /// `name` + `url` + `sha1` instead of `downloads.artifact`).
    #[serde(default)] // handled by custom path below
    pub url: Option<String>,
    #[serde(default)]
    pub sha1: String,
}

impl Library {
    /// The Mojang-style remote artifact path for this library, derived from the
    /// Maven coordinate (`group:artifact:version[:classifier]`) when
    /// `downloads.artifact` is absent (Fabric/Quilt profiles).
    pub fn artifact_path(&self) -> Option<PathBuf> {
        if let Some(art) = self.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
            return Some(art.path.clone());
        }
        maven_to_path(&self.name)
    }

    /// Remote URL for this library (Mojang artifact url, or Maven base + path).
    pub fn artifact_url(&self) -> Option<String> {
        if let Some(art) = self.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
            if !art.url.is_empty() {
                return Some(art.url.clone());
            }
        }
        let path = self.artifact_path()?;
        if let Some(base) = &self.url {
            return Some(format!("{}/{}", base.trim_end_matches('/'), path.to_string_lossy().replace('\\', "/")));
        }
        None
    }

    /// SHA-1 hex from either top-level `sha1` (Fabric) or `downloads.artifact.sha1`.
    pub fn sha1_hex(&self) -> Option<String> {
        if !self.sha1.is_empty() {
            return Some(self.sha1.clone());
        }
        self.downloads.as_ref().and_then(|d| d.artifact.as_ref()).map(|a| a.sha1.clone()).filter(|s| !s.is_empty())
    }
}

/// Convert a Maven coordinate `group:artifact:version[:classifier]` into the
/// standard repository path `group/path/artifact/version/file`.
fn maven_to_path(name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).copied();
    let base = PathBuf::from(&group).join(artifact).join(version);
    let mut file = format!("{artifact}-{version}");
    if let Some(cl) = classifier {
        if !cl.is_empty() {
            file.push('-');
            file.push_str(cl);
        }
    }
    file.push_str(".jar");
    Some(base.join(file))
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct LibraryDownloads {
    pub artifact: Option<DownloadableArtifact>,
    pub classifiers: Option<std::collections::HashMap<String, DownloadableArtifact>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct DownloadableArtifact {
    pub path: PathBuf,
    pub url: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Rule {
    pub action: String,
    pub os: Option<Os>,
    /// Feature-gated rules (demo mode, custom resolution...) never match here.
    pub features: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Os {
    pub name: Option<String>,
    pub arch: Option<String>,
}

/// Serde helper: `mainClass` may be a plain string (`net.minecraft.client.Main`)
/// or an object `{ "client": "...", "server": "..." }` (modern Fabric/NeoForge
/// profile json). Return the client main class as a string, or None.
fn main_class_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = Option::<serde_json::Value>::deserialize(d)?;
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(serde_json::Value::Object(o)) => Ok(o
            .get("client")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                // some profiles use { "mainClass": ... } nested? be safe
                None
            })),
        _ => Ok(None),
    }
}

fn rule_matches(rule: &Rule, os: &str, arch: &str) -> bool {
    if rule.features.is_some() {
        return false;
    }
    match &rule.os {
        Some(o) => {
            let name_ok = o.name.is_none() || o.name.as_deref() == Some(os);
            // Mojang uses "x86"/"x86_64"/"arm64"; we only know 64-bit vs aarch64.
            let arch_ok = match o.arch.as_deref() {
                None => true,
                Some("x86") => false,
                Some("x86_64") => arch == "64",
                Some("arm64") | Some("aarch64") => arch == "aarch64",
                Some(other) => other == arch,
            };
            name_ok && arch_ok
        }
        None => true,
    }
}

/// Evaluate a Mojang rule list: no rules = allowed; otherwise the last
/// matching rule decides, default deny.
pub fn rules_allow(rules: &[Rule], os: &str, arch: &str) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for r in rules {
        if rule_matches(r, os, arch) {
            allowed = r.action == "allow";
        }
    }
    allowed
}

pub fn library_allowed(lib: &Library, os: &str, arch: &str) -> bool {
    match &lib.rules {
        Some(rules) => rules_allow(rules, os, arch),
        None => true,
    }
}

/// Fetch the version manifest (list of all game versions).
pub fn fetch_manifest(client: &reqwest::blocking::Client) -> Result<VersionManifest, String> {
    download::get_json(client, VERSION_MANIFEST_URL)
}

/// Flat list of all versions from a manifest.
pub fn list_versions(manifest: &VersionManifest) -> Vec<VersionEntry> {
    manifest.versions.clone()
}

/// Find a specific version entry by id.
pub fn find_version(manifest: &VersionManifest, id: &str) -> Option<VersionEntry> {
    manifest.versions.iter().find(|v| v.id == id).cloned()
}

/// Fetch the full version json for a manifest entry.
pub fn fetch_version_json(
    client: &reqwest::blocking::Client,
    entry: &VersionEntry,
) -> Result<VersionJson, String> {
    fetch_version_json_text(client, entry).map(|(v, _)| v)
}

/// Same, but also returns the original response text so the caller can store
/// the genuine Mojang JSON instead of a re-serialized copy.
pub fn fetch_version_json_text(
    client: &reqwest::blocking::Client,
    entry: &VersionEntry,
) -> Result<(VersionJson, String), String> {
    let text = download::get_text(client, &entry.url)?;
    if let Some(expected) = entry.sha1.as_deref().filter(|h| !h.is_empty()) {
        use sha1::{Digest, Sha1};
        let actual = hex::encode(Sha1::digest(text.as_bytes()));
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(format!("version json {}: контрольная сумма не совпала (ожидалось {expected}, получено {actual})", entry.id));
        }
    }
    let v = serde_json::from_str(&text).map_err(|e| format!("parse version json {}: {e}", entry.id))?;
    Ok((v, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_path_conversion() {
        assert_eq!(
            maven_to_path("org.ow2.asm:asm:9.7.1"),
            Some(PathBuf::from("org/ow2/asm/asm/9.7.1/asm-9.7.1.jar"))
        );
        assert_eq!(
            maven_to_path("org.lwjgl:lwjgl:3.3.3:natives-windows"),
            Some(PathBuf::from("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar"))
        );
    }

    #[test]
    fn library_remote_formats() {
        // Mojang style: downloads.artifact
        let mb: Library = serde_json::from_str(
            r#"{"name":"x:y:1","downloads":{"artifact":{"path":"com/x/y.jar","url":"https://a/b.jar","sha1":"abc"}}}"#,
        )
        .unwrap();
        assert_eq!(mb.artifact_path(), Some(PathBuf::from("com/x/y.jar")));
        assert_eq!(mb.sha1_hex(), Some("abc".to_string()));

        // Fabric/Quilt style: name + url + sha1
        let mvn: Library = serde_json::from_str(
            r#"{"name":"org.ow2.asm:asm:9.7.1","url":"https://maven.fabricmc.net/","sha1":"def"}"#,
        )
        .unwrap();
        assert_eq!(
            mvn.artifact_path(),
            Some(PathBuf::from("org/ow2/asm/asm/9.7.1/asm-9.7.1.jar"))
        );
        assert_eq!(
            mvn.artifact_url(),
            Some("https://maven.fabricmc.net/org/ow2/asm/asm/9.7.1/asm-9.7.1.jar".to_string())
        );
        assert_eq!(mvn.sha1_hex(), Some("def".to_string()));
    }

    #[test]
    fn main_class_forms() {
        // plain string
        let vj: VersionJson = serde_json::from_str(r#"{"id":"1.20.1","mainClass":"net.minecraft.client.Main","libraries":[]}"#)
            .unwrap();
        assert_eq!(vj.main_class.as_deref(), Some("net.minecraft.client.Main"));

        // object form { client, server } (fabric)
        let vj2: VersionJson = serde_json::from_str(
            r#"{"id":"f","mainClass":{"client":"net.fabricmc.loader.impl.launch.knot.KnotClient","server":"net.fabricmc.loader.impl.launch.knot.KnotServer"}}"#,
        )
        .unwrap();
        assert_eq!(vj2.main_class.as_deref(), Some("net.fabricmc.loader.impl.launch.knot.KnotClient"));
    }

    #[test]
    fn version_json_is_verified_by_sha1() {
        use crate::testsupport::{Mock, route};
        use sha1::{Digest, Sha1};
        let m = Mock::start();
        let body = r#"{"id":"26.3","libraries":[]}"#;
        m.set(vec![route("/v.json", 200, body)]);
        let c = reqwest::blocking::Client::new();
        let good = hex::encode(Sha1::digest(body.as_bytes()));
        let mut entry = VersionEntry { id: "26.3".into(), kind: "release".into(), url: format!("{}/v.json", m.url()), time: None, sha1: Some(good) };
        assert_eq!(fetch_version_json_text(&c, &entry).unwrap().0.id, "26.3");
        entry.sha1 = Some("0".repeat(40));
        assert!(fetch_version_json_text(&c, &entry).unwrap_err().contains("контрольная сумма"));
    }

    /// Real network: the hash check must accept genuine Mojang files.
    /// `cargo test --release -- --ignored real_mojang`
    #[test]
    #[ignore]
    fn real_mojang_version_json_passes_the_hash_check() {
        let c = crate::core::install::make_client();
        let manifest = fetch_manifest(&c).unwrap();
        for id in [manifest.latest.release.clone(), "1.21.1".to_string(), "1.12.2".to_string()] {
            let entry = find_version(&manifest, &id).unwrap();
            assert!(entry.sha1.is_some(), "manifest v2 carries sha1");
            let (v, _) = fetch_version_json_text(&c, &entry).unwrap();
            assert_eq!(v.id, id);
        }
    }

    #[test]
    fn vanilla_json_camel_case() {
        let vj: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.1","mainClass":"net.minecraft.client.main.Main","assets":"17",
            "assetIndex":{"id":"17","sha1":"a","size":1,"url":"https://x/17.json","totalSize":5},
            "javaVersion":{"component":"java-runtime-delta","majorVersion":21},
            "inheritsFrom":"1.21","libraries":[]}"#,
        )
        .unwrap();
        assert_eq!(vj.main_class.as_deref(), Some("net.minecraft.client.main.Main"));
        assert_eq!(vj.asset_index.as_ref().unwrap().total_size, Some(5));
        assert_eq!(vj.java_version.as_ref().unwrap().major_version, 21);
        assert_eq!(vj.inherits_from.as_deref(), Some("1.21"));
    }

    #[test]
    fn fabric_profile_merges_with_parent() {
        let parent: VersionJson = serde_json::from_str(
            r#"{"id":"1.21.1","mainClass":"net.minecraft.client.main.Main","assets":"17",
            "libraries":[{"name":"a:b:1"}],"arguments":{"game":["--x"],"jvm":["-Da"]}}"#,
        )
        .unwrap();
        let child: VersionJson = serde_json::from_str(
            r#"{"id":"fabric-loader-0.16.9-1.21.1","inheritsFrom":"1.21.1",
            "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries":[{"name":"org.ow2.asm:asm:9.7.1","url":"https://maven.fabricmc.net/"}],
            "arguments":{"game":[],"jvm":["-Db"]}}"#,
        )
        .unwrap();
        let m = child.merged_with(&parent);
        assert_eq!(m.main_class.as_deref(), Some("net.fabricmc.loader.impl.launch.knot.KnotClient"));
        assert_eq!(m.assets.as_deref(), Some("17"));
        assert_eq!(m.libraries.len(), 2);
        assert_eq!(m.arguments.unwrap().jvm.unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn rules_windows() {
        let lib: Library = serde_json::from_str(
            r#"{"name":"org.lwjgl:lwjgl-platform:3.3.3","rules":[{"action":"allow","os":{"name":"windows"}}]}"#,
        )
        .unwrap();
        assert!(library_allowed(&lib, "windows", "64"));
        assert!(!library_allowed(&lib, "linux", "64"));

        let any: Library = serde_json::from_str(r#"{"name":"a:b:1"}"#).unwrap();
        assert!(library_allowed(&any, "linux", "64"));
    }

    #[test]
    fn filter_versions_release_snap() {
        fn e(id: &str, kind: &str) -> VersionEntry {
            VersionEntry { id: id.to_string(), kind: kind.to_string(), url: "u".into(), time: None, sha1: None }
        }
        let list = vec![
            e("1.21.1", "release"),
            e("1.21.2-pre1", "snapshot"),
            e("b1.7.3", "old_beta"),
            e("1.5.2", "release"),
        ];
        let rel = filter_versions(&list, false);
        assert_eq!(rel.len(), 2);
        assert_eq!(rel[0].id, "1.21.1");
        assert_eq!(rel[1].id, "1.5.2");

        let all = filter_versions(&list, true);
        assert_eq!(all.len(), 3);
        // порядок сохранён (манифест уже отсортирован по убыванию дат)
        assert_eq!(all[1].id, "1.21.2-pre1");
        assert!(all.iter().all(|v| v.kind != "old_beta"));
    }
}
