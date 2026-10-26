// В релизной сборке окно консоли не создаётся (в отладочной оно нужно для логов).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod assets;
mod core;
mod fx;
mod theme;
mod ui;
#[cfg(test)]
mod testsupport;

use std::sync::Arc;
use eframe::egui;

/// Decode the bundled launcher icon into egui's IconData (window, taskbar, Alt+Tab).
fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon-256.png");
    if let Ok(img) = image::load_from_memory(bytes) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return egui::IconData {
            rgba: rgba.into_raw(),
            width: w,
            height: h,
        };
    }
    egui::IconData { rgba: vec![0, 0, 0, 0], width: 1, height: 1 }
}

fn main() -> eframe::Result<()> {
    // Panic log goes to the launcher's data dir (never a compile-time path).
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC: {info}\n{:?}", std::backtrace::Backtrace::force_capture());
        let dir = core::instance::default_dir();
        if std::fs::write(dir.join("limecraft_panic.log"), &msg).is_err() {
            let _ = std::fs::write(std::env::temp_dir().join("limecraft_panic.log"), &msg);
        }
        eprintln!("{msg}");
    }));
    eframe::run_native(
        "LimeCraft",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 800.0])
                .with_min_inner_size([960.0, 640.0])
                .with_title("LimeCraft")
                .with_app_id("limecraft-launcher")
                .with_icon(Arc::new(load_icon())),
            ..Default::default()
        },
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx, true, core::prefs::DEFAULT_ACCENT, 1.0, false, true);
            Ok(Box::new(app::LimeApp::new(cc)))
        }),
    )
}
