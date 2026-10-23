//! Asset helpers: decode bundled images (include_bytes!) and register them as
//! egui textures. Any failure returns `None` so the UI falls back gracefully.

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};

/// Decode `bytes` (PNG/JPG/ICO via `image::load_from_memory`) into an egui texture.
fn from_bytes(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let size = [w as usize, h as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}

fn hero(ctx: &egui::Context) -> Option<TextureHandle> {
    from_bytes(ctx, "hero", include_bytes!("../assets/hero-1920x1080.jpg"))
}
fn badge(ctx: &egui::Context) -> Option<TextureHandle> {
    from_bytes(ctx, "badge", include_bytes!("../assets/badge-256.jpg"))
}
fn play_icon(ctx: &egui::Context) -> Option<TextureHandle> {
    from_bytes(ctx, "play", include_bytes!("../assets/play-256.jpg"))
}
fn empty_state(ctx: &egui::Context) -> Option<TextureHandle> {
    from_bytes(ctx, "empty", include_bytes!("../assets/empty-state.png"))
}
fn modpack_icon(ctx: &egui::Context) -> Option<TextureHandle> {
    from_bytes(ctx, "modpack", include_bytes!("../assets/modpack-256.png"))
}

/// All textures loaded once at startup.
pub struct Assets {
    pub hero: Option<TextureHandle>,
    pub badge: Option<TextureHandle>,
    pub play_icon: Option<TextureHandle>,
    pub empty_state: Option<TextureHandle>,
    pub modpack_icon: Option<TextureHandle>,
}

/// Fallback: a tiny 1x1 transparent texture so callers always have something.
fn blank(ctx: &egui::Context) -> TextureHandle {
    let color = ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255]);
    ctx.load_texture("blank", color, TextureOptions::LINEAR)
}

impl Assets {
    pub fn load(ctx: &egui::Context) -> Self {
        Self {
            hero: hero(ctx).or_else(|| Some(blank(ctx))),
            badge: badge(ctx).or_else(|| Some(blank(ctx))),
            play_icon: play_icon(ctx).or_else(|| Some(blank(ctx))),
            empty_state: empty_state(ctx).or_else(|| Some(blank(ctx))),
            modpack_icon: modpack_icon(ctx).or_else(|| Some(blank(ctx))),
        }
    }
}

/// Draw `tex` stretched over `rect`, preserving aspect, cropping overflow
/// (cover-fit) so a photo fills a panel without distortion.
pub fn paint_cover(painter: &egui::Painter, tex: &TextureHandle, rect: egui::Rect, tint: Color32) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let st = egui::load::SizedTexture::from_handle(tex);
    let (iw, ih) = (st.size.x, st.size.y);
    if iw <= 0.0 || ih <= 0.0 {
        return;
    }
    let img_aspect = iw / ih;
    let rect_aspect = rect.width() / rect.height();
    let outer = if img_aspect > rect_aspect {
        // image is wider than panel -> match height, the width overflows & gets cropped
        let w = rect.height() * img_aspect;
        egui::Rect::from_center_size(rect.center(), egui::vec2(w, rect.height()))
    } else {
        let h = rect.width() / img_aspect;
        egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), h))
    };
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(st.id, outer, uv, tint);
}
