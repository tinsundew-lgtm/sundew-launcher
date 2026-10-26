//! Pictures chosen by the user (instance icons, profile avatars, the window
//! background): copied into the launcher's data folder in a normalized form, so
//! the original file can be moved or deleted.

use std::path::Path;

/// Biggest file we accept as a picture.
const MAX_INPUT_BYTES: u64 = 40 * 1024 * 1024;

fn open(src: &Path) -> Result<image::DynamicImage, String> {
    let size = std::fs::metadata(src).map_err(|e| format!("{}: {e}", src.display()))?.len();
    if size > MAX_INPUT_BYTES {
        return Err("картинка слишком большая (больше 40 МБ)".into());
    }
    image::open(src).map_err(|e| format!("не удалось открыть картинку: {e}"))
}

/// Square icon: centre-cropped and resized to `size` px, saved as PNG.
pub fn save_square(src: &Path, dest: &Path, size: u32) -> Result<(), String> {
    let img = open(src)?;
    let side = img.width().min(img.height());
    let x = (img.width() - side) / 2;
    let y = (img.height() - side) / 2;
    let square = img.crop_imm(x, y, side, side).resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    square.to_rgba8().save_with_format(dest, image::ImageFormat::Png).map_err(|e| format!("сохранение {}: {e}", dest.display()))
}

/// Background picture: keeps the aspect ratio, at most `max_side` px, saved as PNG.
pub fn save_fitted(src: &Path, dest: &Path, max_side: u32) -> Result<(), String> {
    let img = open(src)?;
    let img = if img.width().max(img.height()) > max_side { img.resize(max_side, max_side, image::imageops::FilterType::Triangle) } else { img };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    img.to_rgba8().save_with_format(dest, image::ImageFormat::Png).map_err(|e| format!("сохранение {}: {e}", dest.display()))
}

/// Biggest remote icon we download.
const MAX_ICON_BYTES: u64 = 5 * 1024 * 1024;

/// Square RGBA thumbnail of picture bytes in any supported format.
pub fn thumbnail_bytes(bytes: &[u8], size: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("не удалось прочитать картинку: {e}"))?;
    let side = img.width().min(img.height()).max(1);
    let x = (img.width() - side) / 2;
    let y = (img.height() - side) / 2;
    let out = img.crop_imm(x, y, side, side).resize_exact(size, size, image::imageops::FilterType::Triangle).to_rgba8();
    Ok((out.into_raw(), size, size))
}

pub fn save_rgba_png(rgba: &[u8], w: u32, h: u32, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    image::save_buffer_with_format(dest, rgba, w, h, image::ColorType::Rgba8, image::ImageFormat::Png)
        .map_err(|e| format!("сохранение {}: {e}", dest.display()))
}

/// Cache file of a remote icon (one ready thumbnail per URL).
pub fn icon_cache_path(cache_dir: &Path, url: &str, size: u32) -> std::path::PathBuf {
    use sha1::{Digest, Sha1};
    let key = hex::encode(Sha1::digest(url.as_bytes()));
    cache_dir.join(format!("{}-{size}.png", &key[..24]))
}

/// Download a project icon once (mirrors are used when the CDN is blocked),
/// keep a square `size` px thumbnail in `cache_dir`, return its pixels.
pub fn cached_icon(client: &reqwest::blocking::Client, url: &str, cache_dir: &Path, size: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let cached = icon_cache_path(cache_dir, url, size);
    if cached.exists() {
        if let Ok(px) = load_rgba(&cached) {
            return Ok(px);
        }
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("неподдерживаемая ссылка на иконку: {url}"));
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let tmp = cached.with_extension("download");
    super::download::download_file_ex(client, url, &tmp, None, None, None)?;
    let result = (|| {
        let size_on_disk = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
        if size_on_disk > MAX_ICON_BYTES {
            return Err("иконка слишком большая".to_string());
        }
        let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
        let (rgba, w, h) = thumbnail_bytes(&bytes, size)?;
        save_rgba_png(&rgba, w, h, &cached)?;
        Ok((rgba, w, h))
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Average brightness (0..=1) of RGBA pixels; transparent ones count as black.
pub fn average_luma(rgba: &[u8]) -> f32 {
    let px = rgba.len() / 4;
    if px == 0 {
        return 0.5;
    }
    // look at about 40k pixels, not all of them
    let step = (px / 40_000).max(1);
    let (mut sum, mut n) = (0.0f64, 0u32);
    for p in rgba.chunks_exact(4).step_by(step) {
        let a = p[3] as f64 / 255.0;
        sum += (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0 * a;
        n += 1;
    }
    (sum / n as f64) as f32
}

/// RGBA pixels of a saved picture: (bytes, width, height).
pub fn load_rgba(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok((img.into_raw(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lc-img-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn square_icons_and_fitted_backgrounds() {
        let d = tmp("ok");
        let src = d.join("wide.jpg");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(300, 100, image::Rgb([200, 30, 30])))
            .save(&src)
            .unwrap();
        let icon = d.join("out/icon.png");
        save_square(&src, &icon, 128).unwrap();
        let (_, w, h) = load_rgba(&icon).unwrap();
        assert_eq!((w, h), (128, 128));

        let bg = d.join("bg.png");
        save_fitted(&src, &bg, 150).unwrap();
        let (_, w, h) = load_rgba(&bg).unwrap();
        assert_eq!((w, h), (150, 50), "aspect ratio is kept");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn remote_icons_are_cached_as_thumbnails() {
        use crate::testsupport::{Mock, route};
        let d = tmp("remote");
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(200, 100, image::Rgba([10, 200, 30, 255])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let m = Mock::start();
        m.set(vec![route("/icon.png", 200, png)]);
        let c = reqwest::blocking::Client::new();
        let url = format!("{}/icon.png", m.url());
        let (rgba, w, h) = cached_icon(&c, &url, &d, 64).unwrap();
        assert_eq!((w, h, rgba.len()), (64, 64, 64 * 64 * 4));
        assert!(icon_cache_path(&d, &url, 64).exists());
        // second time: from the disk cache, no request
        let before = m.requests().len();
        cached_icon(&c, &url, &d, 64).unwrap();
        assert_eq!(m.requests().len(), before);
        // a broken file is an error, not a panic
        m.set(vec![route("/bad.png", 200, "nope")]);
        assert!(cached_icon(&c, &format!("{}/bad.png", m.url()), &d, 64).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn brightness_of_pictures() {
        let white: Vec<u8> = std::iter::repeat([255u8, 255, 255, 255]).take(100).flatten().collect();
        let black: Vec<u8> = std::iter::repeat([0u8, 0, 0, 255]).take(100).flatten().collect();
        assert!(average_luma(&white) > 0.99 && average_luma(&black) < 0.01);
        let half: Vec<u8> = white.iter().chain(black.iter()).copied().collect();
        assert!((average_luma(&half) - 0.5).abs() < 0.02);
        assert_eq!(average_luma(&[]), 0.5);
    }

    #[test]
    fn webp_is_supported() {
        let mut webp = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32, 32, image::Rgba([1, 2, 3, 255])))
            .write_to(&mut std::io::Cursor::new(&mut webp), image::ImageFormat::WebP)
            .unwrap();
        assert_eq!(thumbnail_bytes(&webp, 16).unwrap().1, 16);
    }

    #[test]
    fn non_images_are_rejected() {
        let d = tmp("bad");
        let src = d.join("x.png");
        std::fs::write(&src, b"not an image").unwrap();
        assert!(save_square(&src, &d.join("o.png"), 64).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
