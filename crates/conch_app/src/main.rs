mod app;
mod fonts;
mod icons;
mod input;
#[cfg(target_os = "macos")]
mod macos_menu;
mod state;
mod terminal;
mod ui;

use std::sync::Arc;

use app::ConchApp;

fn load_app_icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../icons/app-icon.png"))
        .expect("Failed to decode app icon")
        .into_rgba8();
    let (w, h) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    }
}

fn main() -> eframe::Result<()> {
    env_logger::init();

    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime"),
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title_shown(true)
            .with_titlebar_shown(true)
            .with_icon(Arc::new(load_app_icon())),
        ..Default::default()
    };

    eframe::run_native(
        "Conch",
        options,
        Box::new(move |_cc| Ok(Box::new(ConchApp::new(rt)))),
    )
}
