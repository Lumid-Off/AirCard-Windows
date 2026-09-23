#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod afc;
mod pairing;
mod usbmux;
mod wallet_connection;
mod service_protocol;
mod native_stream;
mod airlift;
mod airtraffic;
mod native_sync;
mod app;
mod apple;
mod device;
mod flasher;
mod image_skin;
mod passthm;
mod scanner;

fn main() -> eframe::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--native-sync-worker") {
        std::process::exit(airtraffic::run_native_worker());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([850.0, 560.0])
            .with_title("AirCard v1.2.1"),
        ..Default::default()
    };

    eframe::run_native(
        "AirCard v1.2.1",
        options,
        Box::new(|cc| Ok(Box::new(app::AirCardApp::new(cc)))),
    )
}
