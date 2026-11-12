#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]
use ripon_studio::{app::App, brand};

/// Writes every panic (with time and place) to `ripon-studio.log`, so a problem can be diagnosed later.
/// Background workers are wrapped in catch_unwind, so a panic there becomes an error message, not a crash.
fn install_log() {
    let dir = std::env::var_os("RIPON_STUDIO_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("RiponAIPhotoStudio"))).unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("ripon-studio.log");
    if std::fs::metadata(&file).map(|m| m.len() > 512 * 1024).unwrap_or(false) {
        let _ = std::fs::remove_file(&file);
    }
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&file) {
            let _ = writeln!(f, "[{}] v{} PANIC: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), brand::VERSION, info);
        }
    }));
}

fn main() -> eframe::Result<()> {
    install_log();
    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(brand::TITLE)
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([1120.0, 700.0])
            .with_icon(std::sync::Arc::new(brand::icon_data()))
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(brand::TITLE, opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
