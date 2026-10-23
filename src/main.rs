mod app;
mod assets;
mod core;
mod theme;

use std::sync::Arc;
use eframe::egui;

/// Decode the bundled favicon into egui's IconData for the OS window title bar icon.
fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/favicon-64.ico");
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
    // Debug helper: log a startup marker + panic backtrace to a file so we can
    // see whether the crash happens in our code or in the graphics layer.
    use std::io::Write;
    if let Ok(mut f) = std::fs::File::create(concat!(env!("CARGO_MANIFEST_DIR"), "/limecraft_start.log")) {
        let _ = writeln!(
            f,
            "start v0.1 t={}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
    }
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC: {info}\n{:?}", std::backtrace::Backtrace::force_capture());
        let _ = std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/limecraft_panic.log"), &msg);
        eprintln!("{msg}");
    }));
    eframe::run_native(
        "LimeCraft",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 800.0])
                .with_min_inner_size([960.0, 640.0])
                .with_title("LimeCraft")
                .with_icon(Arc::new(load_icon())),
            ..Default::default()
        },
        Box::new(|cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::LimeApp::new(cc)))
        }),
    )
}
