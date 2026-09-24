#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod afc;
mod airlift;
mod airtraffic;
mod app;
mod apple;
// #--- V9 MODULES START ---
mod card_designer;
mod i18n;
mod settings;
// #--- V9 MODULES END ---
mod device;
mod flasher;
mod image_skin;
mod passthm;
mod scanner;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1060.0, 700.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("AirCard v1.2.2 Community v9"),
        ..Default::default()
    };

    eframe::run_native(
        "AirCard v1.2.2 Community v9",
        options,
        Box::new(|cc| Ok(Box::new(app::AirCardApp::new(cc)))),
    )
}
