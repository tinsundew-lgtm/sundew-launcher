use egui::{Color32, Visuals, Style, FontId, RichText, Theme, CornerRadius, Shadow};

pub const BG: Color32 = Color32::from_rgb(0xF6, 0xFB, 0xF0);
pub const CARD: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const GREEN: Color32 = Color32::from_rgb(0x7F, 0xBF, 0x5F);
pub const GREEN_DARK: Color32 = Color32::from_rgb(0x5E, 0x9E, 0x4C);
pub const GREEN_PALE: Color32 = Color32::from_rgb(0xE8, 0xF4, 0xDC);
pub const YELLOW: Color32 = Color32::from_rgb(0xF0, 0xE3, 0x9B);
pub const YELLOW_DARK: Color32 = Color32::from_rgb(0xE2, 0xC9, 0x6A);
pub const TEXT: Color32 = Color32::from_rgb(0x2E, 0x3B, 0x2C);
pub const MUTED: Color32 = Color32::from_rgb(0x7C, 0x8B, 0x75);
pub const DIVIDER: Color32 = Color32::from_rgb(0xE3, 0xEE, 0xE0);
pub const HOVER_BG: Color32 = Color32::from_rgb(0xED, 0xF6, 0xE3);
pub const SUCCESS: Color32 = Color32::from_rgb(0x4F, 0xA3, 0x5B);
pub const DANGER: Color32 = Color32::from_rgb(0xC9, 0x6A, 0x5E);

pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::light();
    visuals.panel_fill = BG;
    visuals.window_fill = CARD;
    visuals.extreme_bg_color = CARD;
    visuals.faint_bg_color = BG;
    visuals.widgets.noninteractive.bg_fill = CARD;
    visuals.widgets.inactive.bg_fill = CARD;
    visuals.widgets.hovered.bg_fill = HOVER_BG;
    visuals.widgets.active.bg_fill = GREEN_PALE;
    visuals.widgets.inactive.fg_stroke.color = GREEN_DARK;
    visuals.widgets.hovered.fg_stroke.color = GREEN_DARK;
    visuals.widgets.active.fg_stroke.color = GREEN_DARK;
    visuals.selection.bg_fill = GREEN;
    visuals.selection.stroke.color = GREEN_DARK;
    visuals.hyperlink_color = GREEN_DARK;
    visuals.window_corner_radius = CornerRadius::same(14);
    visuals.window_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(30),
    };
    visuals.override_text_color = Some(TEXT);
    ctx.set_visuals(visuals);

    let style = Style {
        spacing: egui::Spacing {
            item_spacing: egui::vec2(10.0, 8.0),
            button_padding: egui::vec2(12.0, 8.0),
            ..Default::default()
        },
        ..Default::default()
    };
    ctx.set_style_of(Theme::Light, style);
}

pub fn heading(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn label(text: &str, color: Color32, size: f32) -> RichText {
    RichText::new(text).color(color).size(size)
}
