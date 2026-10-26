//! Small widget kit: icons from the bundled Phosphor font, buttons, pill
//! tabs and colored name tiles.

use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::theme;

/// Test hook: where each labelled widget was drawn in the last frame.
#[cfg(test)]
pub static REGISTRY: std::sync::Mutex<Vec<(String, Rect)>> = std::sync::Mutex::new(Vec::new());

/// Remember a labelled widget rect (no-op outside tests).
#[inline]
pub fn reg(label: &str, rect: Rect) {
    #[cfg(test)]
    REGISTRY.lock().unwrap().push((label.to_string(), rect));
    #[cfg(not(test))]
    let _ = (label, rect);
}

/// Font families of the icon set (Phosphor, MIT; see `assets/fonts`).
pub const ICON_FONT: &str = "phosphor";
pub const ICON_FONT_FILL: &str = "phosphor-fill";

macro_rules! icons {
    ($( $name:ident = $code:literal, $fill:literal ;)*) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Icon { $($name),* }

        impl Icon {
            /// (glyph, drawn with the filled font)
            pub(crate) fn glyph(self) -> (char, bool) {
                match self { $( Icon::$name => ($code, $fill) ),* }
            }
        }

        #[cfg(test)]
        pub const ALL_ICONS: &[Icon] = &[ $(Icon::$name),* ];
    };
}

icons! {
    Home = '\u{E2C2}', false;
    Compass = '\u{E1C8}', false;
    Gear = '\u{E272}', false;
    Plus = '\u{E3D4}', false;
    Play = '\u{E3D0}', true;
    Stop = '\u{E46C}', true;
    Trash = '\u{E4A6}', false;
    Folder = '\u{E24A}', false;
    FolderOpen = '\u{E256}', false;
    Search = '\u{E30C}', false;
    Download = '\u{E20C}', false;
    Refresh = '\u{E094}', false;
    Cube = '\u{E1DA}', false;
    Terminal = '\u{EAE8}', false;
    Dots = '\u{E208}', false;
    Up = '\u{E08E}', false;
    Back = '\u{E058}', false;
    Sliders = '\u{E434}', false;
    Check = '\u{E182}', false;
    Close = '\u{E4F6}', false;
    Package = '\u{E390}', false;
    Puzzle = '\u{E596}', false;
    Image = '\u{E2CA}', false;
    Palette = '\u{E6C8}', false;
    User = '\u{E4C2}', false;
    Wifi = '\u{E4EA}', false;
    Import = '\u{E61E}', false;
    Zip = '\u{E958}', false;
    Globe = '\u{E288}', false;
    Pin = '\u{E3E2}', false;
    Pencil = '\u{E3B4}', false;
    Info = '\u{E2CE}', false;
    Warning = '\u{E4E0}', false;
    Rainbow = '\u{E598}', false;
    Eye = '\u{E220}', false;
    Link = '\u{E2E2}', false;
    Rocket = '\u{E3FE}', false;
    Memory = '\u{E9C4}', false;
    Tag = '\u{E478}', false;
    Stack = '\u{E466}', false;
    Archive = '\u{E00C}', false;
    Shirt = '\u{E670}', false;
    Upload = '\u{E4C0}', false;
}

/// Draw `icon` centered in `rect`. The glyph is centered by its visible
/// bounds, so every icon looks aligned whatever its font metrics are.
pub fn paint_icon(ui: &Ui, icon: Icon, rect: Rect, color: Color32) {
    let (ch, fill) = icon.glyph();
    let size = rect.width().min(rect.height());
    let family = egui::FontFamily::Name(if fill { ICON_FONT_FILL } else { ICON_FONT }.into());
    let galley = ui.painter().layout_no_wrap(ch.to_string(), FontId::new(size, family), color);
    let visible = galley.rows.first().and_then(|row| {
        let g = row.glyphs.first()?;
        (!g.uv_rect.is_nothing()).then(|| Rect::from_min_size(row.pos + g.pos.to_vec2() + g.uv_rect.offset, g.uv_rect.size))
    });
    let origin = match visible {
        Some(v) => rect.center() - v.center().to_vec2(),
        None => rect.center() - galley.size() / 2.0,
    };
    ui.painter().galley(origin, galley, color);
}

// ---------------------------------------------------------------- motion

/// Smooth follower of a yes/no state (hover, selected): 0..=1, eased over `secs`.
/// With animations off it jumps straight to the target.
pub fn anim(ctx: &egui::Context, id: egui::Id, on: bool, secs: f32) -> f32 {
    if !crate::fx::enabled() {
        return if on { 1.0 } else { 0.0 };
    }
    crate::fx::ease_in_out(ctx.animate_bool_with_time(id, on, secs))
}

#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    start: f64,
}

/// A number that glides to `target` (ease-out) over `secs` whenever the target changes.
/// The very first value is taken as it is (nothing to glide from).
pub fn tween(ctx: &egui::Context, id: egui::Id, target: f32, secs: f32) -> f32 {
    if !crate::fx::enabled() {
        return target;
    }
    let now = ctx.input(|i| i.time);
    let at = |t: Tween, now: f64| {
        let k = ((now - t.start) as f32 / secs.max(0.001)).clamp(0.0, 1.0);
        t.from + (t.to - t.from) * crate::fx::ease_out_cubic(k)
    };
    let mut tw = ctx.data(|d| d.get_temp::<Tween>(id)).unwrap_or(Tween { from: target, to: target, start: now });
    if (tw.to - target).abs() > 0.01 {
        tw = Tween { from: at(tw, now), to: target, start: now };
    }
    ctx.data_mut(|d| d.insert_temp(id, tw));
    if now - tw.start < secs as f64 {
        ctx.request_repaint();
    }
    at(tw, now)
}

/// A wave that spreads from the click point over a button (call after painting the face).
pub fn click_ripple(ui: &Ui, resp: &Response, clip: Rect, strength: f32) {
    if !crate::fx::enabled() {
        return;
    }
    let ctx = ui.ctx();
    let now = ctx.input(|i| i.time);
    let id = resp.id.with("ripple");
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            ctx.data_mut(|d| d.insert_temp(id, (now, pos.x, pos.y)));
        }
    }
    if let Some((t0, x, y)) = ctx.data(|d| d.get_temp::<(f64, f32, f32)>(id)) {
        let p = ((now - t0) / 0.5) as f32;
        if (0.0..1.0).contains(&p) {
            let radius = crate::fx::ease_out_cubic(p) * clip.width().max(clip.height()) * 0.9;
            ui.painter().with_clip_rect(clip).circle_filled(Pos2::new(x, y), radius, Color32::WHITE.gamma_multiply((1.0 - p) * strength));
            ctx.request_repaint();
        }
    }
}

// ---------------------------------------------------------------- pixel shapes

/// Font family of bold text (Manrope ExtraBold, or the system's bold font).
pub fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// Bold text of a given size.
pub fn bold(text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text.into()).size(size).family(bold_family())
}

pub fn lighten(c: Color32, t: f32) -> Color32 {
    theme::mix(c, Color32::WHITE, t)
}

pub fn darken(c: Color32, t: f32) -> Color32 {
    theme::mix(c, Color32::BLACK, t)
}

/// A rectangle with a small square cut out of every corner, like pixel art
/// (three plain rectangles, so the pieces never overlap).
pub fn notched_shapes(rect: Rect, step: f32, fill: Color32) -> Vec<Shape> {
    let s = step.min(rect.width() / 4.0).min(rect.height() / 4.0).max(0.0);
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| Shape::rect_filled(Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)), 0.0, fill);
    vec![
        r(rect.left() + s, rect.top(), rect.right() - s, rect.top() + s),
        r(rect.left(), rect.top() + s, rect.right(), rect.bottom() - s),
        r(rect.left() + s, rect.bottom() - s, rect.right() - s, rect.bottom()),
    ]
}

pub fn paint_notched(painter: &egui::Painter, rect: Rect, step: f32, fill: Color32) {
    painter.extend(notched_shapes(rect, step, fill));
}

/// Notched box with a border `width` px thick.
pub fn pixel_box_shapes(rect: Rect, step: f32, fill: Color32, border: Color32, width: f32) -> Vec<Shape> {
    let mut v = notched_shapes(rect, step, border);
    v.extend(notched_shapes(rect.shrink(width), (step - width).max(1.0), fill));
    v
}

/// Vertical gradient inside a notched rectangle (`top` colour to `bottom` colour).
pub fn paint_gradient_notched(painter: &egui::Painter, rect: Rect, step: f32, top: Color32, bottom: Color32) {
    let s = step.min(rect.width() / 4.0).min(rect.height() / 4.0).max(0.0);
    let at = |y: f32| {
        let t = ((y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color32::from_rgba_premultiplied(l(top.r(), bottom.r()), l(top.g(), bottom.g()), l(top.b(), bottom.b()), l(top.a(), bottom.a()))
    };
    let mut mesh = egui::Mesh::default();
    let mut quad = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let i = mesh.vertices.len() as u32;
        for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            mesh.colored_vertex(Pos2::new(x, y), at(y));
        }
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    };
    quad(rect.left() + s, rect.top(), rect.right() - s, rect.top() + s);
    quad(rect.left(), rect.top() + s, rect.right(), rect.bottom() - s);
    quad(rect.left() + s, rect.bottom() - s, rect.right() - s, rect.bottom());
    painter.add(Shape::mesh(mesh));
}

/// A pixel "key": a face with a darker lip under it that is pressed down on click.
/// Returns the rectangle of the face (where the label goes).
pub fn paint_key(painter: &egui::Painter, rect: Rect, face: Color32, lip: Color32, pressed: bool, highlight: f32) -> Rect {
    let step = 3.0;
    paint_notched(painter, rect, step, lip);
    let (top, bottom) = if pressed { (3.0, 1.0) } else { (0.0, 4.0) };
    let face_rect = Rect::from_min_max(rect.min + Vec2::new(0.0, top), rect.max - Vec2::new(0.0, bottom));
    paint_notched(painter, face_rect, step, face);
    if highlight > 0.0 {
        let line = Rect::from_min_size(face_rect.min + Vec2::new(step, 0.0), Vec2::new((face_rect.width() - 2.0 * step).max(0.0), 2.0));
        painter.rect_filled(line, 0.0, lighten(face, highlight));
    }
    face_rect
}

/// (face, lip, label colour, highlight) of a button in the pixel style.
pub fn key_colors(kind: BtnKind, enabled: bool, hover: bool) -> (Color32, Color32, Color32, f32) {
    if !enabled {
        let f = theme::button();
        return (f, darken(f, if theme::palette().dark { 0.4 } else { 0.12 }), theme::muted(), 0.0);
    }
    match kind {
        BtnKind::Primary => {
            let f = if hover { lighten(theme::green(), 0.12) } else { theme::green() };
            (f, darken(theme::green(), 0.38), theme::accent_fg(), 0.16)
        }
        BtnKind::Danger => {
            let f = if hover { lighten(theme::danger(), 0.12) } else { theme::danger() };
            (f, darken(theme::danger(), 0.38), Color32::WHITE, 0.14)
        }
        BtnKind::Secondary => {
            let f = if hover { theme::hover_bg() } else { theme::button() };
            (f, darken(theme::button(), if theme::palette().dark { 0.5 } else { 0.14 }), theme::text(), 0.06)
        }
    }
}

/// [`key_colors`] between the idle (`t` = 0) and the hovered (`t` = 1) look.
pub fn key_colors_t(kind: BtnKind, enabled: bool, t: f32) -> (Color32, Color32, Color32, f32) {
    let (a, b) = (key_colors(kind, enabled, false), key_colors(kind, enabled, true));
    (theme::mix(a.0, b.0, t), theme::mix(a.1, b.1, t), a.2, a.3 + (b.3 - a.3) * t)
}

// ---------------------------------------------------------------- widgets

/// Clickable icon on a square background (toolbar buttons).
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    reg(tip, rect);
    if theme::pixel() {
        let hov = anim(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.12);
        let (face, lip, fg, hi) = if active {
            key_colors_t(BtnKind::Primary, true, hov)
        } else if hov > 0.01 {
            let (f, l, fg, hi) = key_colors_t(BtnKind::Secondary, true, 1.0);
            (f.gamma_multiply(hov), l.gamma_multiply(hov), theme::mix(theme::muted(), fg, hov), hi)
        } else {
            (Color32::TRANSPARENT, Color32::TRANSPARENT, theme::muted(), 0.0)
        };
        if face == Color32::TRANSPARENT {
            paint_icon(ui, icon, rect.shrink(size * 0.22), fg);
        } else {
            let f = paint_key(ui.painter(), rect, face, lip, resp.is_pointer_button_down_on(), hi);
            paint_icon(ui, icon, Rect::from_center_size(f.center(), Vec2::splat(size * 0.5)), fg);
        }
    } else {
        let (bg, fg) = if active {
            (theme::green_pale(), theme::green_dark())
        } else if resp.hovered() {
            (theme::hover_bg(), theme::text())
        } else {
            (Color32::TRANSPARENT, theme::muted())
        };
        ui.painter().rect_filled(rect, CornerRadius::same((size * 0.3) as u8), bg);
        paint_icon(ui, icon, rect.shrink(size * 0.24), fg);
    }
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    /// Filled accent.
    Primary,
    /// Grey / outlined.
    Secondary,
    /// Filled red.
    Danger,
}

/// Button with an optional icon. Returns the response (disabled buttons never click).
pub fn button(ui: &mut Ui, kind: BtnKind, icon: Option<Icon>, text: &str, enabled: bool) -> Response {
    let pixel = theme::pixel();
    let font = if pixel { FontId::new(14.5, bold_family()) } else { FontId::proportional(14.5) };
    let (mut fg, mut fill, mut stroke) = match (kind, enabled) {
        (_, false) => (theme::muted(), theme::card(), Stroke::new(1.0, theme::divider())),
        (BtnKind::Primary, _) => (theme::accent_fg(), theme::green(), Stroke::NONE),
        (BtnKind::Danger, _) => (Color32::WHITE, theme::danger(), Stroke::NONE),
        (BtnKind::Secondary, _) => (theme::text(), Color32::TRANSPARENT, Stroke::new(1.0, theme::divider())),
    };
    let icon_w = if icon.is_some() { 18.0 + 6.0 } else { 0.0 };
    let key = key_colors(kind, enabled, false);
    if pixel {
        fg = key.2;
    }
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = Vec2::new(galley.size().x + icon_w + if pixel { 32.0 } else { 24.0 }, if pixel { 38.0 } else { 34.0 });
    // a disabled button must not react to clicks at all, not just look grey
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    reg(text, rect);
    let hover = enabled && resp.hovered();
    let mut center_y = rect.center().y;
    if pixel {
        let t = anim(ui.ctx(), resp.id.with("hover"), hover, 0.12);
        let (face, lip, _, hi) = key_colors_t(kind, enabled, t);
        let pressed = enabled && resp.is_pointer_button_down_on();
        // the key rises a little under the pointer
        let lifted = rect.translate(Vec2::new(0.0, -1.5 * t));
        let f = paint_key(ui.painter(), lifted, face, lip, pressed, hi);
        center_y = f.center().y;
        if enabled {
            click_ripple(ui, &resp, f, if kind == BtnKind::Secondary { 0.14 } else { 0.38 });
        }
    } else {
        fill = match (kind, hover) {
            (BtnKind::Primary, true) => theme::green_dark(),
            (BtnKind::Secondary, true) => theme::hover_bg(),
            (BtnKind::Danger, true) => Color32::from_rgb(0xf0, 0x7a, 0x70),
            _ => fill,
        };
        let radius = CornerRadius::same(10);
        ui.painter().rect_filled(rect, radius, fill);
        if stroke != Stroke::NONE {
            ui.painter().rect_stroke(rect, radius, stroke, StrokeKind::Inside);
        }
        stroke = Stroke::NONE;
        let _ = stroke;
    }
    let mut x = rect.left() + if pixel { 16.0 } else { 12.0 };
    if let Some(ic) = icon {
        paint_icon(ui, ic, Rect::from_center_size(Pos2::new(x + 9.0, center_y), Vec2::splat(18.0)), fg);
        x += icon_w;
    }
    let pos = Pos2::new(x, center_y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, fg);
    if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp.on_hover_cursor(egui::CursorIcon::NotAllowed) }
}

pub fn primary(ui: &mut Ui, icon: Option<Icon>, text: &str) -> bool {
    button(ui, BtnKind::Primary, icon, text, true).clicked()
}

pub fn secondary(ui: &mut Ui, icon: Option<Icon>, text: &str) -> bool {
    button(ui, BtnKind::Secondary, icon, text, true).clicked()
}

/// Square key with an icon only (compact toolbars). The tip is also its test label.
pub fn icon_key(ui: &mut Ui, icon: Icon, kind: BtnKind, tip: &str) -> Response {
    if !theme::pixel() {
        return icon_button(ui, icon, 38.0, kind == BtnKind::Primary, tip);
    }
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(42.0, 38.0), Sense::click());
    reg(tip, rect);
    let hov = anim(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.12);
    let (face, lip, fg, hi) = key_colors_t(kind, true, hov);
    let f = paint_key(ui.painter(), rect, face, lip, resp.is_pointer_button_down_on(), hi);
    click_ripple(ui, &resp, f, if kind == BtnKind::Secondary { 0.14 } else { 0.38 });
    paint_icon(ui, icon, Rect::from_center_size(f.center(), Vec2::splat(20.0)), fg);
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Width of a row of [`icon_tabs`].
pub fn icon_tabs_width(count: usize) -> f32 {
    count as f32 * 42.0 + 8.0 * count.saturating_sub(1) as f32
}

/// Tabs with icons only (narrow windows). Returns true when the selection changed.
pub fn icon_tabs(ui: &mut Ui, tabs: &[(Icon, &str)], selected: &mut usize) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 8.0;
    ui.horizontal(|ui| {
        for (i, (icon, tip)) in tabs.iter().enumerate() {
            let kind = if *selected == i { BtnKind::Primary } else { BtnKind::Secondary };
            if icon_key(ui, *icon, kind, tip).clicked() && *selected != i {
                *selected = i;
                changed = true;
            }
        }
    });
    changed
}

/// Width of a tab bar made by [`pill_tabs`] (to center it).
pub fn pill_tabs_width(ui: &Ui, tabs: &[(Option<Icon>, &str)]) -> f32 {
    let pixel = theme::pixel();
    let font = if pixel { FontId::new(14.5, bold_family()) } else { FontId::proportional(14.5) };
    let gap = if pixel { 8.0 } else { 4.0 };
    let mut w = 0.0;
    for (icon, label) in tabs {
        let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
        w += galley.size().x + if icon.is_some() { 24.0 } else { 0.0 } + if pixel { 32.0 } else { 26.0 };
    }
    w + gap * tabs.len().saturating_sub(1) as f32 + if pixel { 0.0 } else { 8.0 }
}

/// Tab bar: chunky keys (pixel style, the selection key slides from tab to tab)
/// or a rounded pill (soft style). Returns true when the selection changed.
pub fn pill_tabs(ui: &mut Ui, tabs: &[(Option<Icon>, &str)], selected: &mut usize) -> bool {
    if theme::pixel() { pixel_tabs(ui, tabs, selected) } else { soft_tabs(ui, tabs, selected) }
}

fn pixel_tabs(ui: &mut Ui, tabs: &[(Option<Icon>, &str)], selected: &mut usize) -> bool {
    let mut changed = false;
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.x = 8.0;
    let slider_id = ui.id().with(("pilltabs", tabs.iter().map(|t| t.1).collect::<Vec<_>>().join("|")));
    ui.horizontal(|ui| {
        let font = FontId::new(14.5, bold_family());
        // 1. room for every tab
        let mut items = Vec::new();
        for (icon, label) in tabs {
            let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE);
            let icon_w = if icon.is_some() { 24.0 } else { 0.0 };
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + icon_w + 32.0, 38.0), Sense::click());
            reg(label, rect);
            items.push((rect, resp, galley, *icon));
        }
        let painter = ui.painter().clone();
        // 2. the idle keys
        for (rect, resp, ..) in &items {
            let hov = anim(&ctx, resp.id.with("hover"), resp.hovered(), 0.12);
            let (face, lip, _, hi) = key_colors_t(BtnKind::Secondary, true, hov);
            paint_key(&painter, *rect, face, lip, resp.is_pointer_button_down_on(), hi);
        }
        // 3. the selected key glides to the tab that is selected now
        let sel = (*selected).min(items.len().saturating_sub(1));
        let mut slider = Rect::NOTHING;
        if let Some((target, resp, ..)) = items.get(sel) {
            let x = tween(&ctx, slider_id.with("x"), target.left(), 0.26);
            let w = tween(&ctx, slider_id.with("w"), target.width(), 0.26);
            slider = Rect::from_min_size(Pos2::new(x, target.top()), Vec2::new(w, target.height()));
            let (face, lip, _, hi) = key_colors_t(BtnKind::Primary, true, 0.0);
            paint_key(&painter, slider, face, lip, resp.is_pointer_button_down_on(), hi);
        }
        // 4. labels: dark on the selected key, light elsewhere, mixed while it passes by
        for (i, (rect, resp, galley, icon)) in items.iter().enumerate() {
            let covered = ((slider.right().min(rect.right()) - slider.left().max(rect.left())).max(0.0) / rect.width()).clamp(0.0, 1.0);
            let fg = theme::mix(theme::text(), theme::accent_fg(), covered);
            let cy = rect.top() + (rect.height() - 4.0) / 2.0 + if resp.is_pointer_button_down_on() { 3.0 } else { 0.0 };
            let mut x = rect.left() + 16.0;
            if let Some(ic) = icon {
                paint_icon(ui, *ic, Rect::from_center_size(Pos2::new(x + 8.0, cy), Vec2::splat(16.0)), fg);
                x += 24.0;
            }
            painter.galley_with_override_text_color(Pos2::new(x, cy - galley.size().y / 2.0), galley.clone(), fg);
            if resp.clicked() && i != *selected {
                *selected = i;
                changed = true;
            }
        }
    });
    changed
}

fn soft_tabs(ui: &mut Ui, tabs: &[(Option<Icon>, &str)], selected: &mut usize) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(theme::card())
        .stroke(Stroke::new(1.0, theme::divider()))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                for (i, (icon, label)) in tabs.iter().enumerate() {
                    let active = *selected == i;
                    let fg = if active { theme::green_dark() } else { theme::muted() };
                    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(14.5), fg);
                    let icon_w = if icon.is_some() { 24.0 } else { 0.0 };
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + icon_w + 26.0, 30.0), Sense::click());
                    reg(label, rect);
                    let bg = if active {
                        theme::green_pale()
                    } else if resp.hovered() {
                        theme::hover_bg()
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, CornerRadius::same(10), bg);
                    let mut x = rect.left() + 13.0;
                    if let Some(ic) = icon {
                        paint_icon(ui, *ic, Rect::from_center_size(Pos2::new(x + 8.0, rect.center().y), Vec2::splat(16.0)), fg);
                        x += icon_w;
                    }
                    ui.painter().galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, fg);
                    if resp.clicked() && !active {
                        *selected = i;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Square tile with the initials of `name` on a color derived from the name.
pub fn paint_tile(ui: &Ui, rect: Rect, name: &str) {
    paint_tile_color(ui, rect, name, None)
}

/// Same, with a chosen tile colour (`None` = colour derived from the name).
pub fn paint_tile_color(ui: &Ui, rect: Rect, name: &str, color: Option<[u8; 3]>) {
    let (r, g, b) = tile_rgb(name, color);
    paint_tile_rgb(ui, rect, name, (r, g, b));
}

/// The colour of a tile: the chosen one, or one picked by the name.
pub fn tile_rgb(name: &str, color: Option<[u8; 3]>) -> (u8, u8, u8) {
    // palette derived from the asset colors (leaf, badge, yellow) plus a few harmonious ones
    const COLORS: [(u8, u8, u8); 8] = [
        (0x84, 0xcc, 0x3c),
        (0x60, 0x9c, 0x3c),
        (0xf0, 0xd8, 0x84),
        (0x5f, 0xb8, 0x8a),
        (0xc9, 0xa2, 0x5a),
        (0x7f, 0xa8, 0xd8),
        (0xd8, 0x8f, 0x6a),
        (0xa6, 0x9b, 0xd6),
    ];
    let mut h = name.bytes().fold(0x811c_9dc5u32, |a, b| (a ^ b as u32).wrapping_mul(0x0100_0193));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    match color {
        Some([r, g, b]) => (r, g, b),
        None => COLORS[(h % COLORS.len() as u32) as usize],
    }
}

fn paint_tile_rgb(ui: &Ui, rect: Rect, name: &str, (r, g, b): (u8, u8, u8)) {
    let dark_text = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0 > 0.5;
    if theme::pixel() {
        paint_notched(ui.painter(), rect, (rect.width() * 0.08).clamp(2.0, 6.0), Color32::from_rgb(r, g, b));
    } else {
        let radius = CornerRadius::same((rect.width() * 0.22) as u8);
        ui.painter().rect_filled(rect, radius, Color32::from_rgb(r, g, b));
    }
    let initials: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(|c| c.to_uppercase())
        .collect();
    let initials = if initials.is_empty() { "?".to_string() } else { initials };
    let galley = ui.painter().layout_no_wrap(initials, FontId::new(rect.width() * 0.4, bold_family()), if dark_text { Color32::from_rgb(0x14, 0x20, 0x0c) } else { Color32::WHITE });
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::BLACK);
}

/// Card frame: a notched box (pixel style) or a rounded one (soft style).
pub struct Card {
    margin: egui::Margin,
    stroke: Option<Stroke>,
}

pub fn card() -> Card {
    Card { margin: egui::Margin::same(12), stroke: None }
}

impl Card {
    pub fn inner_margin(mut self, margin: egui::Margin) -> Self {
        self.margin = margin;
        self
    }

    pub fn stroke(mut self, stroke: Stroke) -> Self {
        self.stroke = Some(stroke);
        self
    }

    pub fn show<R>(self, ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
        let stroke = self.stroke.unwrap_or(Stroke::new(1.0, theme::divider()));
        // after a page change the cards fade in one after another
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let order = ctx.data_mut(|d| {
            let n = d.get_temp_mut_or_default::<usize>(egui::Id::new("fx-card-order"));
            *n += 1;
            *n - 1
        });
        let page_t0 = ctx.data(|d| d.get_temp::<f64>(egui::Id::new("fx-page-t0"))).unwrap_or(now - 100.0);
        let enter = crate::fx::ease_out_cubic(crate::fx::progress(now, page_t0, order.min(12) as f32 * 0.06, 0.34));
        let saved = ui.opacity();
        if enter < 1.0 {
            ui.set_opacity(saved * enter);
            ctx.request_repaint();
        }
        let out = if !theme::pixel() {
            egui::Frame::new().fill(theme::card()).stroke(stroke).corner_radius(CornerRadius::same(12)).inner_margin(self.margin).show(ui, add)
        } else {
            // reserve the background first, so it is drawn under the content
            let slot = ui.painter().add(Shape::Noop);
            let out = egui::Frame::new().inner_margin(self.margin).show(ui, add);
            let width = stroke.width.max(1.5);
            ui.painter().set(slot, Shape::Vec(pixel_box_shapes(out.response.rect, 4.0, theme::card(), stroke.color, width)));
            out
        };
        ui.set_opacity(saved);
        out
    }
}

/// Card heading with an icon.
pub fn section(ui: &mut Ui, icon: Icon, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
        paint_icon(ui, icon, rect, theme::green_dark());
        ui.label(bold(text, 16.0).color(theme::text()));
    });
}

/// Section heading.
pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(bold(text, 24.0).color(theme::text()));
}

/// Paint a picture with rounded corners (instance icons, avatars).
pub fn paint_picture(ui: &Ui, rect: Rect, tex: &egui::TextureHandle) {
    let radius = if theme::pixel() { CornerRadius::same(3) } else { CornerRadius::same((rect.width() * 0.22) as u8) };
    egui::Image::from_texture(egui::load::SizedTexture::from_handle(tex)).corner_radius(radius).paint_at(ui, rect);
}
