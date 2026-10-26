//! Asset helpers: decode bundled images (include_bytes!) and register them as
//! egui textures. Any failure returns `None` so the UI falls back gracefully.

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

/// Decode `bytes` (PNG/ICO via `image::load_from_memory`) into an egui texture.
fn from_bytes(ctx: &egui::Context, name: &str, bytes: &[u8], options: TextureOptions) -> Option<TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let size = [w as usize, h as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(name, color, options))
}

/// All textures loaded once at startup.
pub struct Assets {
    /// The launcher icon (16x16 pixel art, drawn enlarged without smoothing).
    pub icon: Option<TextureHandle>,
    pub empty_state: Option<TextureHandle>,
    pub modpack_icon: Option<TextureHandle>,
}

/// Fallback: a tiny 1x1 white texture so callers always have something.
fn blank(ctx: &egui::Context) -> TextureHandle {
    let color = ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255]);
    ctx.load_texture("blank", color, TextureOptions::LINEAR)
}

impl Assets {
    pub fn load(ctx: &egui::Context) -> Self {
        let tex = |name, bytes: &[u8]| from_bytes(ctx, name, bytes, TextureOptions::LINEAR).or_else(|| Some(blank(ctx)));
        Self {
            icon: from_bytes(ctx, "icon", include_bytes!("../assets/icon-16.png"), TextureOptions::NEAREST).or_else(|| Some(blank(ctx))),
            empty_state: tex("empty", include_bytes!("../assets/empty-state.png")),
            modpack_icon: tex("modpack", include_bytes!("../assets/modpack-128.png")),
        }
    }
}

#[cfg(test)]
mod tests {
    /// The sizes the exe icon has to contain.
    const ICO_SIZES: [u32; 6] = [16, 32, 48, 64, 128, 256];

    #[test]
    fn the_icon_is_the_sharp_enlargement_of_the_16px_master() {
        let master = image::load_from_memory(include_bytes!("../assets/icon-16.png")).unwrap().to_rgba8();
        assert_eq!(master.dimensions(), (16, 16));
        let big = image::load_from_memory(include_bytes!("../assets/icon-256.png")).unwrap().to_rgba8();
        assert_eq!(big.dimensions(), (256, 256));
        // every master pixel became a 16x16 block of the same colour (no smoothing)
        for (x, y) in [(0u32, 0u32), (3, 5), (8, 8), (15, 15), (7, 12)] {
            let want = master.get_pixel(x, y);
            for (dx, dy) in [(0, 0), (15, 15), (7, 9)] {
                assert_eq!(big.get_pixel(x * 16 + dx, y * 16 + dy), want, "block {x},{y}");
            }
        }
        assert_eq!(master.get_pixel(0, 0)[3], 0, "the corner is transparent");
    }

    #[test]
    fn the_exe_icon_has_every_size_and_each_picture_is_valid() {
        let ico: &[u8] = include_bytes!("../assets/icon.ico");
        let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]) as usize;
        assert_eq!((u16_at(0), u16_at(2)), (0, 1), "an icon file");
        let count = u16_at(4);
        assert_eq!(count, ICO_SIZES.len());
        let mut seen = Vec::new();
        for i in 0..count {
            let e = 6 + 16 * i;
            let size = if ico[e] == 0 { 256 } else { ico[e] as u32 };
            let (len, offset) = (u32_at(e + 8), u32_at(e + 12));
            let img = image::load_from_memory(&ico[offset..offset + len]).expect("a PNG picture").to_rgba8();
            assert_eq!(img.dimensions(), (size, size));
            assert_eq!(img.get_pixel(0, 0)[3], 0, "transparent corner at {size} px");
            assert!(img.get_pixel(size / 2, size / 2)[3] == 255);
            seen.push(size);
        }
        assert_eq!(seen, ICO_SIZES);
    }
}
