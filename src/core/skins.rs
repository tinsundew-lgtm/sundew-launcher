//! Offline skin library. Minecraft in offline mode cannot fetch skins from
//! Mojang, so the launcher keeps the skins itself: PNG files in `<data>/skins`,
//! a preview in the launcher, and, when a game starts, a copy of the chosen skin
//! in the folder where the CustomSkinLoader mod looks for local skins.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// Biggest skin file we read (real skins are a few KB).
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// A skin of the library (the picture is `<data>/skins/<id>.png`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skin {
    /// Short hash of the picture: the same file is never stored twice.
    pub id: String,
    pub name: String,
    /// Slim ("Alex") arms, 3 pixels wide.
    #[serde(default)]
    pub slim: bool,
}

pub fn skins_dir(root: &Path) -> PathBuf {
    root.join("skins")
}

pub fn skin_path(root: &Path, id: &str) -> PathBuf {
    skins_dir(root).join(format!("{id}.png"))
}

// ---------------------------------------------------------------- reading skins

/// Decode and check a skin picture (64×64, or the old 64×32), return it as an
/// RGBA image. Other sizes are refused: the game would not draw them.
pub fn decode_skin(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err("файл слишком большой для скина (больше 2 МБ)".into());
    }
    let img = image::load_from_memory(bytes).map_err(|_| "это не картинка (нужен PNG-файл скина)".to_string())?.to_rgba8();
    match img.dimensions() {
        (64, 64) | (64, 32) => Ok(img),
        (w, h) => Err(format!("скин должен быть 64×64 (или старый 64×32), а здесь {w}×{h}")),
    }
}

/// Arms look slim when the pixel column that a 4 px wide arm would use (x = 54,
/// back of the right arm) is completely transparent.
pub fn looks_slim(img: &image::RgbaImage) -> bool {
    img.height() == 64 && (20..32).all(|y| img.get_pixel(54, y)[3] == 0)
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Add a skin from picture bytes. The picture is re-encoded, so nothing but
/// pixels is kept from the original file.
pub fn add_bytes(root: &Path, bytes: &[u8], name: &str, slim: Option<bool>) -> Result<Skin, String> {
    let img = decode_skin(bytes)?;
    let png = encode_png(&img)?;
    let id = hex::encode(Sha1::digest(&png))[..12].to_string();
    std::fs::create_dir_all(skins_dir(root)).map_err(|e| format!("не удалось создать папку скинов: {e}"))?;
    std::fs::write(skin_path(root, &id), &png).map_err(|e| format!("не удалось сохранить скин: {e}"))?;
    let name = name.trim();
    Ok(Skin { id, name: if name.is_empty() { "Скин".into() } else { name.chars().take(40).collect() }, slim: slim.unwrap_or_else(|| looks_slim(&img)) })
}

/// Add a skin from a file on disk; it is named after the file.
pub fn add_file(root: &Path, file: &Path) -> Result<Skin, String> {
    let size = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?.len();
    if size > MAX_BYTES {
        return Err("файл слишком большой для скина (больше 2 МБ)".into());
    }
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let name = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    add_bytes(root, &bytes, &name, None)
}

pub fn remove_file(root: &Path, id: &str) {
    let _ = std::fs::remove_file(skin_path(root, id));
}

// ---------------------------------------------------------------- from Mojang by nickname

/// Where the public skin data lives (replaceable for tests).
pub struct Endpoints {
    pub profile: String,
    pub session: String,
    /// Skin pictures are only downloaded from addresses starting with this.
    pub textures: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            profile: get("LIMECRAFT_MOJANG_PROFILE_API", "https://api.mojang.com"),
            session: get("LIMECRAFT_MOJANG_SESSION_API", "https://sessionserver.mojang.com"),
            textures: get("LIMECRAFT_MOJANG_TEXTURES", "https://textures.minecraft.net/"),
        }
    }
}

/// Skin address and "slim" flag from the `textures` property of a profile.
pub fn parse_textures_property(base64_value: &str) -> Result<(String, bool), String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(base64_value.trim()).map_err(|_| "ответ Mojang не разобрать".to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&raw).map_err(|_| "ответ Mojang не разобрать".to_string())?;
    let skin = v.get("textures").and_then(|t| t.get("SKIN")).ok_or("у этого игрока нет своего скина")?;
    let url = skin.get("url").and_then(|u| u.as_str()).ok_or("у этого игрока нет своего скина")?.to_string();
    let slim = skin.get("metadata").and_then(|m| m.get("model")).and_then(|m| m.as_str()) == Some("slim");
    Ok((url, slim))
}

/// Download the current skin of a Minecraft player by nickname (public data, no sign-in).
pub fn fetch_by_nickname(client: &reqwest::blocking::Client, nickname: &str) -> Result<(Vec<u8>, bool), String> {
    let nick = nickname.trim();
    if !super::account::valid_nickname(nick) {
        return Err("ник: 3–16 символов, латиница, цифры и _".into());
    }
    let ep = Endpoints::from_env();
    let unreachable = |e: reqwest::Error| format!("Mojang недоступен из вашей сети: {}", super::download::describe_err(&e));

    let resp = client.get(format!("{}/users/profiles/minecraft/{nick}", ep.profile.trim_end_matches('/'))).send().map_err(unreachable)?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT || resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("игрок «{nick}» не найден"));
    }
    let profile: serde_json::Value = resp.error_for_status().map_err(|e| e.to_string())?.json().map_err(|_| "ответ Mojang не разобрать".to_string())?;
    let id = profile.get("id").and_then(|i| i.as_str()).filter(|i| i.len() == 32 && i.chars().all(|c| c.is_ascii_hexdigit())).ok_or("ответ Mojang не разобрать")?;

    let full: serde_json::Value = client
        .get(format!("{}/session/minecraft/profile/{id}", ep.session.trim_end_matches('/')))
        .send()
        .map_err(unreachable)?
        .error_for_status()
        .map_err(|e| format!("Mojang ответил: {e}"))?
        .json()
        .map_err(|_| "ответ Mojang не разобрать".to_string())?;
    let value = full
        .get("properties")
        .and_then(|p| p.as_array())
        .and_then(|p| p.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some("textures")))
        .and_then(|x| x.get("value"))
        .and_then(|v| v.as_str())
        .ok_or("у этого игрока нет своего скина")?;
    let (url, slim) = parse_textures_property(value)?;
    // the address comes from a server answer: only the official skin host is trusted
    let url = if url.starts_with("http://textures.minecraft.net/") { url.replacen("http://", "https://", 1) } else { url };
    if !url.starts_with(&ep.textures) {
        return Err("Mojang вернул неожиданный адрес скина — отказ".into());
    }
    let bytes = client
        .get(url.as_str())
        .send()
        .map_err(unreachable)?
        .error_for_status()
        .map_err(|e| format!("не удалось скачать скин: {e}"))?
        .bytes()
        .map_err(|e| e.to_string())?;
    Ok((bytes.to_vec(), slim))
}

// ---------------------------------------------------------------- in the game

/// Put the skin where the CustomSkinLoader mod reads local skins:
/// `<game folder>/CustomSkinLoader/LocalSkin/skins/<nickname>.png`.
pub fn apply_to_instance(mines: &Path, nickname: &str, skin_png: &Path) -> Result<(), String> {
    if !super::account::valid_nickname(nickname) {
        return Err("ник не подходит для скина".into());
    }
    let dir = mines.join("CustomSkinLoader").join("LocalSkin").join("skins");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::copy(skin_png, dir.join(format!("{nickname}.png"))).map_err(|e| format!("не удалось положить скин в игру: {e}"))?;
    Ok(())
}

/// Is a skin mod in the instance's `mods` folder (enabled)?
pub fn has_skin_loader(mines: &Path) -> bool {
    std::fs::read_dir(mines.join("mods"))
        .map(|d| {
            d.flatten().any(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                n.ends_with(".jar") && n.contains("customskinloader")
            })
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------- preview geometry

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Front,
    Back,
}

/// One rectangle of the flat preview: a piece of the skin picture drawn at a
/// place of the figure (a figure is 16 wide and 32 tall, in skin pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    /// x, y, width, height in the skin picture.
    pub uv: [f32; 4],
    /// x, y, width, height on the figure.
    pub at: [f32; 4],
    /// Mirror the piece (old skins reuse the right limbs for the left ones).
    pub flip: bool,
    /// Outer layer: drawn after the body, transparent where there is nothing.
    pub overlay: bool,
}

/// Pieces of a figure seen from `view`. `legacy` = old 64×32 picture.
pub fn parts(view: View, slim: bool, legacy: bool) -> Vec<Part> {
    let aw = if slim { 3.0 } else { 4.0 };
    let front = view == View::Front;
    let p = |uv: [f32; 4], at: [f32; 4], flip: bool, overlay: bool| Part { uv, at, flip, overlay };
    // viewer's left/right: from the front the figure's right arm/leg is on the viewer's left
    let (head, hat) = if front { ([8.0, 8.0], [40.0, 8.0]) } else { ([24.0, 8.0], [56.0, 8.0]) };
    let (body, jacket) = if front { ([20.0, 20.0], [20.0, 36.0]) } else { ([32.0, 20.0], [32.0, 36.0]) };

    // right arm / sleeve, left arm / sleeve (x of the face in the picture)
    let (r_arm_x, l_arm_x, l_arm_y) = if front {
        (44.0, 36.0, 52.0)
    } else if slim {
        (51.0, 43.0, 52.0)
    } else {
        (52.0, 44.0, 52.0)
    };
    let (r_sleeve_x, l_sleeve_x) = if front { (44.0, 52.0) } else if slim { (51.0, 59.0) } else { (52.0, 60.0) };
    let (r_leg_x, l_leg_x, l_pants_x) = if front { (4.0, 20.0, 4.0) } else { (12.0, 28.0, 12.0) };

    // positions on the figure: viewer's left slot, viewer's right slot
    let left_slot_arm = [if slim { 1.0 } else { 0.0 }, 8.0, aw, 12.0];
    let right_slot_arm = [12.0, 8.0, aw, 12.0];
    let left_slot_leg = [4.0, 20.0, 4.0, 12.0];
    let right_slot_leg = [8.0, 20.0, 4.0, 12.0];

    let mut v = vec![p([head[0], head[1], 8.0, 8.0], [4.0, 0.0, 8.0, 8.0], false, false), p([body[0], body[1], 8.0, 12.0], [4.0, 8.0, 8.0, 12.0], false, false)];

    // (slot, uv of the base layer, flip) for arms and legs, in viewer order
    let right_arm_uv = [r_arm_x, 20.0, aw, 12.0];
    let left_arm_uv = if legacy { right_arm_uv } else { [l_arm_x, l_arm_y, aw, 12.0] };
    let right_leg_uv = [r_leg_x, 20.0, 4.0, 12.0];
    let left_leg_uv = if legacy { right_leg_uv } else { [l_leg_x, 52.0, 4.0, 12.0] };
    if front {
        v.push(p(right_arm_uv, left_slot_arm, false, false));
        v.push(p(left_arm_uv, right_slot_arm, legacy, false));
        v.push(p(right_leg_uv, left_slot_leg, false, false));
        v.push(p(left_leg_uv, right_slot_leg, legacy, false));
    } else {
        v.push(p(left_arm_uv, left_slot_arm, legacy, false));
        v.push(p(right_arm_uv, right_slot_arm, false, false));
        v.push(p(left_leg_uv, left_slot_leg, legacy, false));
        v.push(p(right_leg_uv, right_slot_leg, false, false));
    }

    // outer layers
    v.push(p([hat[0], hat[1], 8.0, 8.0], [4.0, 0.0, 8.0, 8.0], false, true));
    if !legacy {
        v.push(p([jacket[0], jacket[1], 8.0, 12.0], [4.0, 8.0, 8.0, 12.0], false, true));
        let right_sleeve = [r_sleeve_x, 36.0, aw, 12.0];
        let left_sleeve = [l_sleeve_x, 52.0, aw, 12.0];
        let right_pants = [r_leg_x, 36.0, 4.0, 12.0];
        let left_pants = [l_pants_x, 52.0, 4.0, 12.0];
        if front {
            v.push(p(right_sleeve, left_slot_arm, false, true));
            v.push(p(left_sleeve, right_slot_arm, false, true));
            v.push(p(right_pants, left_slot_leg, false, true));
            v.push(p(left_pants, right_slot_leg, false, true));
        } else {
            v.push(p(left_sleeve, left_slot_arm, false, true));
            v.push(p(right_sleeve, right_slot_arm, false, true));
            v.push(p(left_pants, left_slot_leg, false, true));
            v.push(p(right_pants, right_slot_leg, false, true));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-skins-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
        encode_png(&img).unwrap()
    }

    #[test]
    fn only_real_skin_sizes_are_accepted() {
        assert!(decode_skin(&png(64, 64, |_, _| [9, 9, 9, 255])).is_ok());
        assert!(decode_skin(&png(64, 32, |_, _| [9, 9, 9, 255])).is_ok());
        assert!(decode_skin(&png(128, 128, |_, _| [9, 9, 9, 255])).unwrap_err().contains("128×128"));
        assert!(decode_skin(b"not a picture").is_err());
        assert!(decode_skin(&vec![0u8; 3 * 1024 * 1024]).unwrap_err().contains("большой"));
    }

    #[test]
    fn slim_arms_are_detected() {
        let classic = decode_skin(&png(64, 64, |_, _| [200, 100, 50, 255])).unwrap();
        assert!(!looks_slim(&classic));
        let slim = decode_skin(&png(64, 64, |x, _| if x == 54 { [0, 0, 0, 0] } else { [200, 100, 50, 255] })).unwrap();
        assert!(looks_slim(&slim));
        assert!(!looks_slim(&decode_skin(&png(64, 32, |_, _| [0, 0, 0, 0])).unwrap()), "old skins never are");
    }

    #[test]
    fn library_stores_each_picture_once() {
        let d = tmp("lib");
        let bytes = png(64, 64, |x, y| [x as u8, y as u8, 7, 255]);
        let a = add_bytes(&d, &bytes, "  Мой скин ", None).unwrap();
        let b = add_bytes(&d, &bytes, "other", Some(true)).unwrap();
        assert_eq!(a.id, b.id, "same picture, same id");
        assert_eq!((a.name.as_str(), a.slim, b.slim), ("Мой скин", false, true));
        assert!(skin_path(&d, &a.id).is_file());
        let file = d.join("Steve Remix.png");
        std::fs::write(&file, &bytes).unwrap();
        assert_eq!(add_file(&d, &file).unwrap().name, "Steve Remix");
        remove_file(&d, &a.id);
        assert!(!skin_path(&d, &a.id).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn skin_goes_where_the_skin_mod_looks() {
        let d = tmp("apply");
        let src = d.join("s.png");
        std::fs::write(&src, png(64, 64, |_, _| [1, 2, 3, 255])).unwrap();
        let mines = d.join("mines");
        apply_to_instance(&mines, "Steve_01", &src).unwrap();
        assert!(mines.join("CustomSkinLoader/LocalSkin/skins/Steve_01.png").is_file());
        assert!(apply_to_instance(&mines, "../evil", &src).is_err(), "no path tricks through the nickname");
        assert!(!has_skin_loader(&mines));
        std::fs::create_dir_all(mines.join("mods")).unwrap();
        std::fs::write(mines.join("mods/CustomSkinLoader_Fabric-14.28.jar"), b"x").unwrap();
        assert!(has_skin_loader(&mines));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mojang_textures_property_is_read() {
        use base64::Engine;
        let enc = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let ok = enc(r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/abc","metadata":{"model":"slim"}}}}"#);
        assert_eq!(parse_textures_property(&ok).unwrap(), ("http://textures.minecraft.net/texture/abc".to_string(), true));
        let classic = enc(r#"{"textures":{"SKIN":{"url":"http://textures.minecraft.net/texture/abc"}}}"#);
        assert!(!parse_textures_property(&classic).unwrap().1);
        assert!(parse_textures_property(&enc(r#"{"textures":{}}"#)).is_err());
        assert!(parse_textures_property("%%%").is_err());
    }

    #[test]
    fn nickname_download_follows_mojang_and_trusts_only_its_host() {
        use crate::testsupport::{Mock, route};
        use base64::Engine;
        let m = Mock::start();
        let skin = png(64, 64, |_, _| [5, 6, 7, 255]);
        let prop = |url: &str| base64::engine::general_purpose::STANDARD.encode(format!(r#"{{"textures":{{"SKIN":{{"url":"{url}","metadata":{{"model":"slim"}}}}}}}}"#));
        let session = |url: &str| format!(r#"{{"id":"{}","name":"Steve","properties":[{{"name":"textures","value":"{}"}}]}}"#, "a".repeat(32), prop(url));
        // SAFETY: only this test reads these variables
        unsafe {
            std::env::set_var("LIMECRAFT_MOJANG_PROFILE_API", m.url());
            std::env::set_var("LIMECRAFT_MOJANG_SESSION_API", m.url());
            std::env::set_var("LIMECRAFT_MOJANG_TEXTURES", format!("{}/texture/", m.url()));
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        }
        let c = reqwest::blocking::Client::new();
        m.set(vec![
            route("/users/profiles/minecraft/Steve", 200, format!(r#"{{"id":"{}","name":"Steve"}}"#, "a".repeat(32))),
            route("/session/minecraft/profile/", 200, session(&format!("{}/texture/abc", m.url()))),
            route("/texture/abc", 200, skin.clone()),
        ]);
        let (bytes, slim) = fetch_by_nickname(&c, "Steve").unwrap();
        assert!(slim && decode_skin(&bytes).is_ok());

        // an address on another host is refused, nothing is downloaded from it
        m.set(vec![
            route("/users/profiles/minecraft/Steve", 200, format!(r#"{{"id":"{}","name":"Steve"}}"#, "a".repeat(32))),
            route("/session/minecraft/profile/", 200, session("https://evil.example/skin.png")),
        ]);
        assert!(fetch_by_nickname(&c, "Steve").unwrap_err().contains("адрес"));

        // unknown player
        m.set(vec![route("/users/profiles/minecraft/Nobody", 204, "")]);
        assert!(fetch_by_nickname(&c, "Nobody").unwrap_err().contains("не найден"));
        assert!(fetch_by_nickname(&c, "a b").is_err(), "invalid nickname never reaches the network");
    }

    #[test]
    fn preview_pieces_stay_inside_the_picture() {
        for legacy in [false, true] {
            for slim in [false, true] {
                for view in [View::Front, View::Back] {
                    let h = if legacy { 32.0 } else { 64.0 };
                    for part in parts(view, slim, legacy) {
                        let [x, y, w, hh] = part.uv;
                        assert!(x >= 0.0 && y >= 0.0 && x + w <= 64.0 && y + hh <= h, "{view:?} slim={slim} legacy={legacy}: {part:?}");
                        let [ax, ay, aw, ah] = part.at;
                        assert!(ax >= 0.0 && ay >= 0.0 && ax + aw <= 16.0 && ay + ah <= 32.0, "{part:?}");
                        assert_eq!(part.uv[2] > 0.0 && part.uv[3] > 0.0, true);
                    }
                }
            }
        }
        assert!(parts(View::Front, false, true).iter().all(|p| !p.overlay || p.uv[0] == 40.0), "old skins only have the hat layer");
        assert!(parts(View::Front, false, false).len() > parts(View::Front, false, true).len());
    }
}
