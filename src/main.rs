#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod afc;
mod airlift;
mod airtraffic;
mod app;
mod apple;
mod device;
mod environment;
mod environment_setup;
mod flasher;
mod i18n;
mod image_skin;
mod passthm;
mod scanner;
mod wallet_backup;

fn main() -> eframe::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--environment-worker") {
        std::process::exit(environment::run_worker());
    }
    let title = format!("AirCard v{}", env!("CARGO_PKG_VERSION"));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title(&title),
        ..Default::default()
    };

    eframe::run_native(
        &title,
        options,
        Box::new(|cc| Ok(Box::new(app::AirCardApp::new(cc)))),
    )
}
