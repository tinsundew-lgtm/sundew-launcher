//! User preferences for customization: look of the launcher, library view,
//! game window and what happens after launch. Stored in `launcher_state.json`
//! (`ui` field); every field has a default so older files keep loading.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    /// Follow the operating system.
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    #[default]
    Plain,
    Gradient,
    Image,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FontChoice {
    /// Manrope, bundled with the launcher (SIL OFL).
    #[default]
    Manrope,
    /// Segoe UI (or Arial) from Windows.
    System,
    /// egui's bundled font.
    Builtin,
}

/// Look of buttons, tabs and cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UiStyle {
    /// Chunky "pixel" widgets: notched corners, buttons with a lip, big bold text.
    #[default]
    Pixel,
    /// Flat widgets with rounded corners.
    Soft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LibraryView {
    #[default]
    Grid,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CardSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl CardSize {
    /// Landscape cover cards of the pixel style: (width, height).
    pub fn cover(self) -> (f32, f32) {
        match self {
            CardSize::Small => (168.0, 108.0),
            CardSize::Medium => (206.0, 132.0),
            CardSize::Large => (262.0, 168.0),
        }
    }

    /// (card width, card height, tile size)
    pub fn dims(self) -> (f32, f32, f32) {
        match self {
            CardSize::Small => (136.0, 168.0, 76.0),
            CardSize::Medium => (172.0, 200.0, 104.0),
            CardSize::Large => (212.0, 244.0, 136.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    #[default]
    LastPlayed,
    Name,
    Created,
    Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AfterLaunch {
    /// Leave the launcher as it is.
    #[default]
    Keep,
    /// Minimize, and bring it back when the game exits.
    Minimize,
    /// Close the launcher (the game keeps running).
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameWindow {
    pub custom_size: bool,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
}

impl Default for GameWindow {
    fn default() -> Self {
        Self { custom_size: false, width: 1280, height: 720, fullscreen: false }
    }
}

impl GameWindow {
    /// Extra game arguments for the window settings.
    pub fn game_args(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.custom_size && !self.fullscreen {
            v.extend(["--width".to_string(), self.width.clamp(320, 7680).to_string()]);
            v.extend(["--height".to_string(), self.height.clamp(240, 4320).to_string()]);
        }
        if self.fullscreen {
            v.push("--fullscreen".to_string());
        }
        v
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiPrefs {
    pub theme: ThemeMode,
    pub style: UiStyle,
    /// Accent color (RGB).
    pub accent: [u8; 3],
    pub background: Background,
    /// File name of the background picture inside `<data>/custom/`.
    pub bg_image: Option<String>,
    /// How much the background picture is darkened/lightened (0..=0.9).
    pub bg_dim: f32,
    /// Average brightness (0..=1) of the background picture, measured when it was chosen.
    pub bg_luma: Option<f32>,
    /// Pick light or dark text to match the background picture.
    pub auto_text: bool,
    /// The accent color slowly walks through all hues.
    pub rainbow: bool,
    /// Interface zoom (0.8..=1.5).
    pub scale: f32,
    pub compact: bool,
    pub font: FontChoice,
    /// Motion: page transitions, hover and press effects, cards flying in.
    pub animations: bool,
    /// Sand that crumbles off the settings page while it scrolls.
    pub sand: bool,

    pub library_view: LibraryView,
    pub card_size: CardSize,
    pub sort: SortBy,

    pub window: GameWindow,
    pub after_launch: AfterLaunch,
    /// Open the instance's live console when a game starts.
    pub console_on_launch: bool,
}

/// The default green (a fresh, slightly cooler tone than the old leaf green).
pub const DEFAULT_ACCENT: [u8; 3] = [0x5e, 0xc6, 0x4d];

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Dark,
            style: UiStyle::Pixel,
            accent: DEFAULT_ACCENT,
            background: Background::Plain,
            bg_image: None,
            bg_dim: 0.55,
            bg_luma: None,
            auto_text: true,
            rainbow: false,
            scale: 1.0,
            compact: false,
            font: FontChoice::Manrope,
            animations: true,
            sand: true,
            library_view: LibraryView::Grid,
            card_size: CardSize::Medium,
            sort: SortBy::LastPlayed,
            window: GameWindow::default(),
            after_launch: AfterLaunch::Keep,
            console_on_launch: false,
        }
    }
}

impl UiPrefs {
    /// Keep values in their allowed ranges (hand-edited files, old versions).
    pub fn sanitized(mut self) -> Self {
        self.scale = if self.scale.is_finite() { self.scale.clamp(0.8, 1.5) } else { 1.0 };
        self.bg_dim = if self.bg_dim.is_finite() { self.bg_dim.clamp(0.0, 0.9) } else { 0.55 };
        self.bg_luma = self.bg_luma.filter(|l| l.is_finite()).map(|l| l.clamp(0.0, 1.0));
        self
    }

    /// Dark palette (light text) or light palette (dark text), by the theme
    /// setting; with a background picture and `auto_text` it follows the
    /// picture's brightness instead.
    pub fn use_dark(&self, system_dark: bool) -> bool {
        if self.background == Background::Image && self.auto_text {
            if let (Some(luma), Some(_)) = (self.bg_luma, self.bg_image.as_ref()) {
                return dark_palette_for_image(luma, self.bg_dim);
            }
        }
        match self.theme {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => system_dark,
        }
    }
}

/// Which palette reads better over a picture of average brightness `luma`
/// that is covered with the palette's own background at strength `dim`.
pub fn dark_palette_for_image(luma: f32, dim: f32) -> bool {
    const DARK_BG: f32 = 0.09;
    const LIGHT_BG: f32 = 0.95;
    const LIGHT_TEXT: f32 = 0.9;
    const DARK_TEXT: f32 = 0.12;
    let under_dark = luma * (1.0 - dim) + DARK_BG * dim;
    let under_light = luma * (1.0 - dim) + LIGHT_BG * dim;
    LIGHT_TEXT - under_dark >= under_light - DARK_TEXT
}

/// Accent presets offered in the settings (name, RGB).
pub const ACCENT_PRESETS: [(&str, [u8; 3]); 9] = [
    ("Лайм", DEFAULT_ACCENT),
    ("Листва", [0x84, 0xcc, 0x3c]),
    ("Изумруд", [0x2e, 0xc2, 0x7e]),
    ("Бирюза", [0x22, 0xc1, 0xc3]),
    ("Небо", [0x4a, 0x9b, 0xf0]),
    ("Лаванда", [0x9b, 0x7b, 0xf0]),
    ("Роза", [0xe8, 0x6a, 0xa8]),
    ("Янтарь", [0xf0, 0xb4, 0x3c]),
    ("Коралл", [0xf0, 0x7a, 0x5a]),
];

/// Memory/JVM presets.
pub struct MemoryPreset {
    pub name: &'static str,
    pub hint: &'static str,
    pub memory_mb: u64,
    pub jvm_args: &'static str,
}

pub const MEMORY_PRESETS: [MemoryPreset; 3] = [
    MemoryPreset {
        name: "Слабый ПК",
        hint: "2 ГБ, ванилла и лёгкие моды",
        memory_mb: 2048,
        jvm_args: "-XX:+UseG1GC -XX:MaxGCPauseMillis=50",
    },
    MemoryPreset {
        name: "Стандарт",
        hint: "4 ГБ, большинство сборок",
        memory_mb: 4096,
        jvm_args: "-XX:+UseG1GC -XX:+ParallelRefProcEnabled -XX:MaxGCPauseMillis=200",
    },
    MemoryPreset {
        name: "Много модов",
        hint: "8 ГБ, тяжёлые модпаки",
        memory_mb: 8192,
        jvm_args: "-XX:+UseG1GC -XX:+ParallelRefProcEnabled -XX:MaxGCPauseMillis=200 -XX:+UnlockExperimentalVMOptions \
                   -XX:+DisableExplicitGC -XX:G1NewSizePercent=30 -XX:G1MaxNewSizePercent=40 -XX:G1HeapRegionSize=8M -XX:G1ReservePercent=20",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_files_get_defaults() {
        let p: UiPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(p, UiPrefs::default());
        assert_eq!((p.style, p.font), (UiStyle::Pixel, FontChoice::Manrope));
        assert!(p.animations && p.sand, "motion is on by default");
        let p: UiPrefs = serde_json::from_str(r#"{"style":"soft","font":"system"}"#).unwrap();
        assert_eq!((p.style, p.font), (UiStyle::Soft, FontChoice::System));
        let p: UiPrefs = serde_json::from_str(r#"{"theme":"light","scale":9.0}"#).unwrap();
        assert_eq!(p.theme, ThemeMode::Light);
        assert_eq!(p.sanitized().scale, 1.5);
    }

    #[test]
    fn text_color_follows_the_picture() {
        // bright picture, weak dimming -> dark text on a light palette
        assert!(!dark_palette_for_image(0.85, 0.2));
        // dark picture -> light text
        assert!(dark_palette_for_image(0.1, 0.2));
        // heavy dimming makes the palette's own background decide
        let mut p = UiPrefs { background: Background::Image, bg_image: Some("b.png".into()), bg_luma: Some(0.9), bg_dim: 0.2, ..UiPrefs::default() };
        assert!(!p.use_dark(true), "light picture beats the dark theme");
        p.auto_text = false;
        assert!(p.use_dark(false), "off: the theme setting decides (dark)");
        p.theme = ThemeMode::Light;
        assert!(!p.use_dark(true));
        p.theme = ThemeMode::System;
        assert!(p.use_dark(true) && !p.use_dark(false));
        // no picture chosen: nothing to measure
        let q = UiPrefs { background: Background::Image, bg_luma: Some(0.9), ..UiPrefs::default() };
        assert!(q.use_dark(true));
    }

    #[test]
    fn window_arguments() {
        let mut w = GameWindow::default();
        assert!(w.game_args().is_empty());
        w.custom_size = true;
        w.width = 1600;
        w.height = 900;
        assert_eq!(w.game_args(), vec!["--width", "1600", "--height", "900"]);
        w.fullscreen = true;
        assert_eq!(w.game_args(), vec!["--fullscreen"], "fullscreen ignores the size");
    }

    #[test]
    fn presets_are_sane() {
        assert!(MEMORY_PRESETS.windows(2).all(|w| w[0].memory_mb < w[1].memory_mb));
        assert!(MEMORY_PRESETS.iter().all(|p| p.jvm_args.split_whitespace().all(|a| a.starts_with("-XX:"))));
    }
}
