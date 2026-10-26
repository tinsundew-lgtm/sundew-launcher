//! Runtime theme: a palette derived from the user's accent color and light/dark
//! mode (defaults come from the bundled assets: leaf `#84CC3C`, yellow `#F0D884`).
//! Widgets read colors through the functions below, so changes apply instantly.

use std::sync::RwLock;

use egui::{Color32, CornerRadius, Shadow, Stroke, Theme, Visuals};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Pixel style: chunky widgets with notched corners.
    pub pixel: bool,
    /// Face of secondary buttons and idle tabs.
    pub button: Color32,
    /// Background of text fields.
    pub input: Color32,
    pub bg: Color32,
    pub panel: Color32,
    pub card: Color32,
    pub hover_bg: Color32,
    pub divider: Color32,
    pub green: Color32,
    pub green_dark: Color32,
    pub green_pale: Color32,
    pub accent_fg: Color32,
    pub yellow: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub danger: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const DARK_DEFAULT: Palette = Palette {
    dark: true,
    pixel: false,
    button: rgb(0x21, 0x27, 0x23),
    input: rgb(0x15, 0x18, 0x16),
    bg: rgb(0x15, 0x18, 0x16),
    panel: rgb(0x1b, 0x20, 0x1d),
    card: rgb(0x21, 0x27, 0x23),
    hover_bg: rgb(0x2a, 0x32, 0x2d),
    divider: rgb(0x32, 0x3b, 0x35),
    green: rgb(0x84, 0xcc, 0x3c),
    green_dark: rgb(0x9b, 0xdb, 0x5a),
    green_pale: rgb(0x26, 0x36, 0x22),
    accent_fg: rgb(0x12, 0x20, 0x0a),
    yellow: rgb(0xf0, 0xd8, 0x84),
    text: rgb(0xe8, 0xed, 0xe6),
    muted: rgb(0x93, 0xa0, 0x8f),
    danger: rgb(0xe5, 0x64, 0x5a),
};

static PALETTE: RwLock<Palette> = RwLock::new(DARK_DEFAULT);

pub fn palette() -> Palette {
    *PALETTE.read().unwrap_or_else(|e| e.into_inner())
}

macro_rules! color_fns {
    ($($name:ident),*) => { $( #[inline] pub fn $name() -> Color32 { palette().$name } )* };
}
color_fns!(bg, panel, card, hover_bg, divider, green, green_dark, green_pale, accent_fg, yellow, text, muted, danger, button, input);

/// Pixel style on?
pub fn pixel() -> bool {
    palette().pixel
}

/// Success messages use the accent.
pub fn success() -> Color32 {
    palette().green
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// Color of the rainbow accent at `t` seconds (one full turn in about 20 s).
pub fn rainbow_accent(t: f64) -> [u8; 3] {
    let h = (t / 20.0).fract() as f32;
    hsv_to_rgb(h, 0.62, 0.95)
}

/// `h`, `s`, `v` in 0..=1.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let (i, f) = (h6.floor() as i32, h6 - h6.floor());
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8]
}

/// Palette for a mode and an accent color.
pub fn build_palette(dark: bool, accent: [u8; 3], pixel: bool) -> Palette {
    let accent = Color32::from_rgb(accent[0], accent[1], accent[2]);
    let base = if pixel && dark {
        // near-black neutral greys with a green accent
        Palette {
            pixel: true,
            bg: rgb(0x16, 0x17, 0x18),
            panel: rgb(0x1e, 0x1f, 0x20),
            card: rgb(0x1e, 0x1f, 0x20),
            hover_bg: rgb(0x2a, 0x2b, 0x2e),
            divider: rgb(0x2c, 0x2d, 0x30),
            button: rgb(0x26, 0x27, 0x2a),
            input: rgb(0x13, 0x13, 0x14),
            text: rgb(0xf2, 0xf3, 0xf2),
            muted: rgb(0x90, 0x94, 0x91),
            danger: rgb(0xe5, 0x48, 0x4d),
            ..DARK_DEFAULT
        }
    } else if pixel {
        Palette {
            dark: false,
            pixel: true,
            bg: rgb(0xef, 0xf0, 0xf2),
            panel: rgb(0xe4, 0xe6, 0xe9),
            card: rgb(0xff, 0xff, 0xff),
            hover_bg: rgb(0xe9, 0xeb, 0xee),
            divider: rgb(0xd2, 0xd5, 0xda),
            button: rgb(0xe0, 0xe3, 0xe7),
            input: rgb(0xff, 0xff, 0xff),
            yellow: rgb(0x9a, 0x74, 0x10),
            text: rgb(0x1b, 0x1d, 0x1f),
            muted: rgb(0x5f, 0x65, 0x6b),
            danger: rgb(0xc9, 0x3c, 0x33),
            ..DARK_DEFAULT
        }
    } else if dark {
        DARK_DEFAULT
    } else {
        Palette {
            dark: false,
            button: rgb(0xff, 0xff, 0xff),
            input: rgb(0xf8, 0xfa, 0xf7),
            bg: rgb(0xf3, 0xf6, 0xf1),
            panel: rgb(0xe6, 0xec, 0xe2),
            card: rgb(0xff, 0xff, 0xff),
            hover_bg: rgb(0xec, 0xf2, 0xe8),
            divider: rgb(0xd8, 0xe0, 0xd2),
            yellow: rgb(0x9a, 0x74, 0x10),
            text: rgb(0x1e, 0x28, 0x1b),
            muted: rgb(0x63, 0x70, 0x5c),
            danger: rgb(0xc9, 0x3c, 0x33),
            ..DARK_DEFAULT
        }
    };
    let white = Color32::WHITE;
    let black = Color32::BLACK;
    Palette {
        green: accent,
        // accent used as text: brighter on dark backgrounds, deeper on light ones
        green_dark: if dark { mix(accent, white, 0.18) } else { mix(accent, black, 0.32) },
        green_pale: mix(base.bg, accent, if dark { 0.16 } else { 0.18 }),
        accent_fg: if luminance(accent) > 0.55 { rgb(0x12, 0x20, 0x0a) } else { white },
        ..base
    }
}

/// Install the palette and egui visuals; `scale` is the interface zoom.
pub fn apply(ctx: &egui::Context, dark: bool, accent: [u8; 3], scale: f32, compact: bool, pixel: bool) {
    let p = build_palette(dark, accent, pixel);
    *PALETTE.write().unwrap_or_else(|e| e.into_inner()) = p;

    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.divider);
    v.extreme_bg_color = p.input; // text inputs
    v.faint_bg_color = p.card;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.green_dark;
    v.selection.bg_fill = p.green_pale;
    v.selection.stroke = Stroke::new(1.0, p.green);

    let radius = CornerRadius::same(if pixel { 3 } else { 8 });
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    v.widgets.noninteractive.bg_fill = p.card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);
    v.widgets.inactive.bg_fill = p.card;
    v.widgets.inactive.weak_bg_fill = if pixel { p.button } else { p.card };
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.divider);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.bg_fill = p.hover_bg;
    v.widgets.hovered.weak_bg_fill = p.hover_bg;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.green);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.active.bg_fill = p.green_pale;
    v.widgets.active.weak_bg_fill = p.green_pale;
    v.widgets.active.bg_stroke = Stroke::new(1.0, p.green);
    // also drives `.strong()` text
    v.widgets.active.fg_stroke = Stroke::new(1.0, if dark { Color32::WHITE } else { Color32::BLACK });
    v.window_corner_radius = CornerRadius::same(if pixel { 4 } else { 14 });
    v.window_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 40 }) };
    v.popup_shadow = v.window_shadow;
    ctx.set_visuals_of(Theme::Dark, v.clone());
    ctx.set_visuals_of(Theme::Light, v);
    ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });

    ctx.all_styles_mut(|style| {
        if compact {
            style.spacing.item_spacing = egui::vec2(7.0, 5.0);
            style.spacing.button_padding = egui::vec2(9.0, 4.0);
            style.spacing.interact_size.y = 24.0;
        } else {
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.spacing.interact_size.y = 30.0;
        }
    });
    ctx.set_zoom_factor(scale.clamp(0.8, 1.5));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rainbow_walks_through_hues() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(1.0 / 3.0, 1.0, 1.0), [0, 255, 0]);
        assert_eq!(hsv_to_rgb(2.0 / 3.0, 1.0, 1.0), [0, 0, 255]);
        let a = rainbow_accent(0.0);
        assert_ne!(a, rainbow_accent(5.0));
        assert_eq!(rainbow_accent(1.0), rainbow_accent(21.0), "it repeats");
    }

    #[test]
    fn palettes_follow_mode_and_accent() {
        let d = build_palette(true, [0x84, 0xcc, 0x3c], false);
        let l = build_palette(false, [0x84, 0xcc, 0x3c], false);
        assert!(luminance(d.bg) < 0.2 && luminance(l.bg) > 0.8);
        assert!(luminance(d.text) > 0.8 && luminance(l.text) < 0.2);
        let blue = build_palette(true, [0x4a, 0x9b, 0xf0], false);
        assert_eq!(blue.green, Color32::from_rgb(0x4a, 0x9b, 0xf0));
        // text on a dark accent button is white, on a bright one it is dark
        assert_eq!(build_palette(true, [0x30, 0x30, 0x90], false).accent_fg, Color32::WHITE);
        assert_ne!(build_palette(true, [0xf0, 0xf0, 0x80], false).accent_fg, Color32::WHITE);
        // pixel style: near-black greys, the same accent
        let px = build_palette(true, [0x5e, 0xc6, 0x4d], true);
        assert!(px.pixel && luminance(px.bg) < 0.12 && luminance(px.input) < luminance(px.bg));
        assert!(luminance(px.button) > luminance(px.bg));
        let pl = build_palette(false, [0x5e, 0xc6, 0x4d], true);
        assert!(pl.pixel && !pl.dark && luminance(pl.bg) > 0.85 && luminance(pl.text) < 0.2);
    }
}
