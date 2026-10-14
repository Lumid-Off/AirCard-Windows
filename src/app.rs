use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use eframe::egui;
use image::DynamicImage;

use crate::apple;
use crate::device::{ConnectionMode, DeviceInfo, DeviceTransport, list_connected_devices};
use crate::environment::{CheckStatus, EnvironmentCheck, ScanEvent, ScanRequest};
use crate::environment_setup::{SetupAction, SetupOutcome};
use crate::flasher::{flash_passcode_theme, flash_wallet_skin, restore_wallet_original};
use crate::i18n::Language;
use crate::image_skin::{PreparedSkin, crop_uv_for_card};
use crate::passthm::{PasscodeTheme, parse_passthm_file};
use crate::scanner::{SavedCard, load_saved_cards, scan_syslog_for_cards};
use crate::wallet_backup::backup_exists;

#[derive(PartialEq, Eq)]
enum AppTab {
    Wallet,
    Passcode,
    Environment,
    Help,
}

const ENVIRONMENT_TITLES: [&str; 6] = [
    crate::environment::APPLE_RUNTIME,
    "Apple Mobile Device Service",
    "Apple USB device channel",
    "Lockdown service access (BUID)",
    "Selected iPhone / transport",
    "Apple pairing / AirTraffic service",
];

enum BackgroundTaskMessage {
    Progress {
        step: usize,
        total: usize,
        message: String,
    },
    Log(String),
    CardFound {
        hash: String,
        name: String,
    },
    Done(Result<String, String>),
}

#[cfg(windows)]
fn current_timestamp() -> String {
    #[repr(C)]
    struct SystemTime {
        w_year: u16,
        w_month: u16,
        w_day_of_week: u16,
        w_day: u16,
        w_hour: u16,
        w_minute: u16,
        w_second: u16,
        w_milliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(lpSystemTime: *mut SystemTime);
    }
    let mut st = std::mem::MaybeUninit::<SystemTime>::uninit();
    unsafe {
        GetLocalTime(st.as_mut_ptr());
        let st = st.assume_init();
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            st.w_hour, st.w_minute, st.w_second, st.w_milliseconds
        )
    }
}

#[cfg(not(windows))]
fn current_timestamp() -> String {
    "00:00:00.000".to_string()
}

pub struct AirCardApp {
    current_tab: AppTab,
    language: Language,
    apple_status: String,
    apple_ready: bool,
    environment_checks: Vec<EnvironmentCheck>,
    environment_rx: Option<Receiver<ScanEvent>>,
    environment_checked_at: Option<String>,
    environment_scope: String,
    environment_stale: bool,
    environment_generation: u64,
    setup_rx: Option<Receiver<Result<SetupOutcome, String>>>,
    setup_notice: Option<(String, bool)>,
    setup_restart_required: bool,
    setup_reboot_required: bool,

    // Device management
    devices: Vec<DeviceInfo>,
    selected_udid: Option<String>,
    connection_mode: ConnectionMode,

    // Wallet tab
    card_hash: String,
    saved_cards: Vec<SavedCard>,
    source_path: Option<PathBuf>,
    source_image: Option<DynamicImage>,
    source_texture: Option<egui::TextureHandle>,
    crop_focus: [f32; 2],
    crop_dirty: bool,
    skin: Option<PreparedSkin>,
    scanning_syslog: bool,
    scan_stop_flag: Option<Arc<AtomicBool>>,

    // Passcode tab
    theme_path: Option<PathBuf>,
    loaded_theme: Option<PasscodeTheme>,
    forced_telephony_ver: String,
    keypad_language: String,
    passcode_bold: bool,
    keypad_textures: Vec<(String, egui::TextureHandle)>,

    // Worker thread & progress
    is_busy: bool,
    progress_step: usize,
    progress_total: usize,
    progress_msg: String,
    status_msg: String,
    task_rx: Option<Receiver<BackgroundTaskMessage>>,
    logs: Vec<String>,
    show_logs_window: bool,
}

impl AirCardApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_custom_fonts(&cc.egui_ctx);
        setup_custom_theme(&cc.egui_ctx);

        let (apple_ready, apple_status) = match apple::verify_support() {
            Ok(msg) => (true, msg),
            Err(err) => (false, err.to_string()),
        };

        let language = Language::load();
        let mut app = Self {
            current_tab: if apple_ready && !std::env::args().any(|arg| arg == "--environment") {
                AppTab::Wallet
            } else {
                AppTab::Environment
            },
            language,
            apple_status,
            apple_ready,
            environment_checks: Vec::new(),
            environment_rx: None,
            environment_checked_at: None,
            environment_scope: String::new(),
            environment_stale: false,
            environment_generation: 0,
            setup_rx: None,
            setup_notice: None,
            setup_restart_required: false,
            setup_reboot_required: false,

            devices: Vec::new(),
            selected_udid: None,
            connection_mode: ConnectionMode::Auto,

            card_hash: String::new(),
            saved_cards: load_saved_cards(),
            source_path: None,
            source_image: None,
            source_texture: None,
            crop_focus: [0.5, 0.5],
            crop_dirty: false,
            skin: None,
            scanning_syslog: false,
            scan_stop_flag: None,

            theme_path: None,
            loaded_theme: None,
            forced_telephony_ver: "Auto (TelephonyUI-10)".to_string(),
            keypad_language: "English".to_string(),
            passcode_bold: false,
            keypad_textures: Vec::new(),

            is_busy: false,
            progress_step: 0,
            progress_total: 0,
            progress_msg: String::new(),
            status_msg: language
                .text("Ready. Connect iPhone via USB or paired WiFi and unlock it.")
                .to_string(),
            task_rx: None,
            logs: Vec::new(),
            show_logs_window: false,
        };

        app.add_log(format!(
            "AirCard Windows v{} initialized",
            env!("CARGO_PKG_VERSION")
        ));
        app.add_log(format!(
            "Apple Support Runtime: {}",
            if app.apple_ready {
                "Loaded and operational"
            } else {
                "Not found (iTunes required)"
            }
        ));
        app.add_log(format!(
            "Loaded {} saved card(s) from database",
            app.saved_cards.len()
        ));

        if app.apple_ready {
            app.refresh_devices();
        }

        app
    }

    fn add_log(&mut self, text: impl AsRef<str>) {
        let ts = current_timestamp();
        self.logs.push(format!("[{}] {}", ts, text.as_ref()));
        if self.logs.len() > 1000 {
            self.logs.remove(0);
        }
    }

    fn refresh_devices(&mut self) {
        self.environment_stale = true;
        self.add_log("Scanning for connected iOS devices via usbmuxd...");
        match list_connected_devices() {
            Ok(devs) => {
                self.devices = devs;
                let selection_still_exists = self.selected_udid.as_ref().is_some_and(|selected| {
                    self.devices
                        .iter()
                        .any(|device| device.udid.eq_ignore_ascii_case(selected))
                });
                if !selection_still_exists && !self.devices.is_empty() {
                    self.selected_udid = Some(self.devices[0].udid.clone());
                }
                if self.devices.is_empty() {
                    self.selected_udid = None;
                    self.add_log("No devices detected. Connect by USB, or enable WiFi sync after initial USB pairing.");
                    self.status_msg = self
                        .language
                        .text("No iPhone connected via USB or paired WiFi.")
                        .to_string();
                } else {
                    let dev_logs: Vec<String> = self
                        .devices
                        .iter()
                        .enumerate()
                        .map(|(i, d)| format!("Device #{}: {} - UDID: {}", i + 1, d, d.udid))
                        .collect();
                    for line in dev_logs {
                        self.add_log(line);
                    }
                    self.status_msg = format!(
                        "{} {} {} {}",
                        self.language.text("Found").as_ref(),
                        self.devices.len(),
                        self.language
                            .text("connected device(s); transport mode:")
                            .as_ref(),
                        self.language.text(self.connection_mode.label())
                    );
                }
            }
            Err(err) => {
                self.add_log(format!("Device scan error: {}", err));
                self.status_msg = format!(
                    "{} {}",
                    self.language.text("Could not enumerate devices:").as_ref(),
                    err
                );
            }
        }
    }

    fn run_environment_check(&mut self) {
        if self.is_busy || self.scanning_syslog || self.environment_working() {
            return;
        }
        self.add_log("Running environment checks...");
        self.environment_checks.clear();
        self.environment_generation += 1;
        self.environment_checked_at = Some(current_timestamp());
        self.environment_scope = format!(
            "{} | {}",
            self.connection_mode.label(),
            self.selected_udid
                .as_deref()
                .unwrap_or("First available iPhone")
        );
        self.environment_stale = false;
        self.status_msg = "Checking the environment... Keep the iPhone unlocked.".into();
        let request = ScanRequest {
            udid: self.selected_udid.clone(),
            mode: self.connection_mode,
        };
        let (tx, rx) = channel();
        self.environment_rx = Some(rx);
        thread::spawn(move || {
            let checks_tx = tx.clone();
            let result = crate::environment::scan_in_worker(request, move |check| {
                let _ = checks_tx.send(ScanEvent::Check(check));
            })
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(ScanEvent::Finished(result));
        });
    }

    fn handle_environment_messages(&mut self) {
        let Some(rx) = self.environment_rx.take() else {
            return;
        };
        loop {
            match rx.try_recv() {
                Ok(ScanEvent::Check(check)) => {
                    if check.title == crate::environment::APPLE_RUNTIME {
                        self.apple_ready = check.status == CheckStatus::Pass;
                        self.apple_status = check.detail.clone();
                    }
                    self.add_log(format!(
                        "Environment: {} - {}: {}",
                        check.title,
                        check.status.label(),
                        check.detail
                    ));
                    self.environment_checks.push(check);
                }
                Ok(ScanEvent::Finished(result)) => {
                    if let Err(error) = result {
                        self.add_log(format!("Environment check failed: {error}"));
                        self.environment_checks.push(EnvironmentCheck::new(
                            "Check completion",
                            CheckStatus::Fail,
                            error,
                            "Run the checks again after reconnecting the iPhone.",
                        ));
                    }
                    let failures = self
                        .environment_checks
                        .iter()
                        .filter(|check| check.status == CheckStatus::Fail)
                        .count();
                    let warnings = self
                        .environment_checks
                        .iter()
                        .filter(|check| {
                            check.status == CheckStatus::Warning && !environment_skipped(check)
                        })
                        .count();
                    self.status_msg = format!(
                        "Environment check completed: {failures} failure(s), {warnings} item(s) need attention."
                    );
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.environment_rx = Some(rx);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.environment_checks.push(EnvironmentCheck::new(
                        "Check completion",
                        CheckStatus::Fail,
                        "The diagnostic task stopped unexpectedly.",
                        "Run the checks again.",
                    ));
                    self.status_msg = "Error: Environment check stopped unexpectedly.".into();
                    break;
                }
            }
        }
    }

    fn selected_transport_available(&self) -> bool {
        if self.environment_working() || self.setup_restart_required {
            return false;
        }
        self.selected_udid.as_ref().is_some_and(|selected| {
            self.devices.iter().any(|device| {
                if !device.udid.eq_ignore_ascii_case(selected) {
                    return false;
                }
                if self.connection_mode == ConnectionMode::Wifi {
                    return device.has_transport(DeviceTransport::Wifi)
                        && !device.has_transport(DeviceTransport::Usb);
                }
                device.supports(self.connection_mode)
            })
        })
    }

    fn environment_working(&self) -> bool {
        self.environment_rx.is_some() || self.setup_rx.is_some()
    }

    fn run_environment_setup(&mut self, action: SetupAction) {
        if self.is_busy
            || self.scanning_syslog
            || self.environment_working()
            || self.setup_restart_required
        {
            return;
        }
        self.setup_notice = None;
        self.environment_stale = true;
        self.status_msg = "Configuring environment… Complete the Windows prompt if shown.".into();
        self.add_log(format!("Environment setup: {}", action.label()));
        let (tx, rx) = channel();
        self.setup_rx = Some(rx);
        thread::spawn(move || {
            let result =
                crate::environment_setup::run(action).map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
    }

    fn handle_setup_messages(&mut self) {
        let Some(rx) = self.setup_rx.take() else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.setup_rx = Some(rx);
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(
                "Environment setup stopped unexpectedly. Check again and copy the report.".into(),
            ),
        };
        let mut recheck = false;
        let (notice, failed) = match result {
            Ok(SetupOutcome::Recheck) => {
                recheck = true;
                ("Service configured. Checking again…".to_string(), false)
            }
            Ok(SetupOutcome::RestartRequired) => {
                self.setup_restart_required = true;
                ("Apple setup finished. Restart AirCard to reload the components. Restart Windows if the installer requests it.".to_string(), false)
            }
            Ok(SetupOutcome::RebootRequired) => {
                self.setup_restart_required = true;
                self.setup_reboot_required = true;
                ("Apple setup requires a Windows restart. Restart Windows, then reopen AirCard.".to_string(), false)
            }
            Ok(SetupOutcome::TrustRequired) => ("iTunes opened. Connect USB, unlock iPhone and tap Trust. Close iTunes, then check again.".to_string(), false),
            Err(error) => { recheck = true; (error, true) },
        };
        self.add_log(format!(
            "Environment setup {}: {notice}",
            if failed { "failed" } else { "finished" }
        ));
        self.status_msg = if failed {
            format!("Error: {notice}")
        } else {
            notice.clone()
        };
        self.setup_notice = Some((notice, failed));
        if recheck {
            self.refresh_devices();
            self.run_environment_check();
        }
    }

    fn restart_after_setup(&mut self, ctx: &egui::Context) {
        use std::os::windows::process::CommandExt;
        let result = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--environment")
                .creation_flags(0x08000000)
                .spawn()
        });
        match result {
            Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(error) => self.status_msg = format!("Error: Could not restart AirCard: {error}"),
        }
    }

    fn validate_selected_transport(&mut self, operation: &str) -> bool {
        if self.selected_udid.is_none() {
            self.add_log(format!(
                "{} failed: No connected iPhone selected.",
                operation
            ));
            self.status_msg = "Please select a connected iPhone.".to_string();
            return false;
        }
        if !self.selected_transport_available() {
            let wifi_has_usb_attached = self.connection_mode == ConnectionMode::Wifi
                && self.selected_udid.as_ref().is_some_and(|selected| {
                    self.devices.iter().any(|device| {
                        device.udid.eq_ignore_ascii_case(selected)
                            && device.has_transport(DeviceTransport::Wifi)
                            && device.has_transport(DeviceTransport::Usb)
                    })
                });
            self.add_log(format!(
                "{} failed: Selected device is unavailable in {} mode.",
                operation,
                self.connection_mode.label()
            ));
            self.status_msg = if wifi_has_usb_attached {
                self.language
                    .text("Disconnect the USB cable and refresh to guarantee the full AirTraffic path uses WiFi.")
                    .to_string()
            } else {
                format!(
                    "{} {} {}",
                    self.language.text("Selected iPhone has no").as_ref(),
                    self.language.text(self.connection_mode.label()),
                    self.language
                        .text("connection. Refresh devices or change transport mode.")
                        .as_ref()
                )
            };
            return false;
        }
        true
    }

    fn select_skin(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return;
        };

        self.add_log(format!("Opening skin image: {}", path.display()));
        match image::open(&path) {
            Ok(source_image) => {
                let source_width = source_image.width();
                let source_height = source_image.height();
                let skin = match PreparedSkin::from_image_with_focus(source_image.clone(), 0.5, 0.5)
                {
                    Ok(skin) => skin,
                    Err(error) => {
                        self.add_log(format!("Image preparation failed: {error:#}"));
                        self.status_msg = format!("Could not prepare image: {error:#}");
                        return;
                    }
                };
                let source_rgba = source_image.thumbnail(2048, 2048).to_rgba8();
                let source_preview = egui::ColorImage::from_rgba_unmultiplied(
                    [source_rgba.width() as usize, source_rgba.height() as usize],
                    source_rgba.as_raw(),
                );

                self.add_log(format!(
                    "Skin processed: source {}x{} resampled to 1536x969 PNG ({:.1} KB)",
                    skin.source_width,
                    skin.source_height,
                    skin.png.len() as f32 / 1024.0,
                ));
                self.source_texture = Some(ctx.load_texture(
                    "card-skin-source-preview",
                    source_preview,
                    egui::TextureOptions::LINEAR,
                ));
                self.status_msg = format!(
                    "Prepared {} ({}x{} -> 1536x969 PNG, {:.1} KB)",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("image"),
                    source_width,
                    source_height,
                    skin.png.len() as f32 / 1024.0,
                );
                self.source_path = Some(path);
                self.source_image = Some(source_image);
                self.crop_focus = [0.5, 0.5];
                self.crop_dirty = false;
                self.skin = Some(skin);
            }
            Err(error) => {
                self.add_log(format!("Image decode failed: {error:#}"));
                self.status_msg = format!("Could not decode image: {error:#}");
            }
        }
    }

    fn rebuild_skin_from_source(&mut self) {
        let Some(source_image) = self.source_image.as_ref() else {
            return;
        };

        match PreparedSkin::from_image_with_focus(
            source_image.clone(),
            self.crop_focus[0],
            self.crop_focus[1],
        ) {
            Ok(skin) => {
                self.add_log(format!(
                    "Crop updated: focus ({:.2}, {:.2}), prepared PNG {:.1} KB",
                    self.crop_focus[0],
                    self.crop_focus[1],
                    skin.png.len() as f32 / 1024.0,
                ));
                self.skin = Some(skin);
                self.crop_dirty = false;
                self.status_msg = self.language.text("Crop position updated.").to_string();
            }
            Err(error) => {
                self.add_log(format!("Crop preparation failed: {error:#}"));
                self.status_msg = format!("Could not update crop: {error:#}");
            }
        }
    }

    fn save_prepared_png(&mut self) {
        let Some(skin) = &self.skin else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("aircard-skin.png")
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, &skin.png) {
            Ok(()) => {
                self.add_log(format!(
                    "Exported prepared card skin PNG: {}",
                    path.display()
                ));
                self.status_msg = format!("Saved prepared PNG: {}", path.display());
            }
            Err(err) => {
                self.add_log(format!("Failed to save PNG: {err}"));
                self.status_msg = format!("Could not save PNG: {err}");
            }
        }
    }

    fn toggle_syslog_scan(&mut self) {
        if self.scanning_syslog {
            if let Some(flag) = self.scan_stop_flag.take() {
                flag.store(true, Ordering::Relaxed);
            }
            self.scanning_syslog = false;
            self.add_log("Syslog scanning stopped by user.");
            self.status_msg = self.language.text("Syslog scanning stopped.").to_string();
            return;
        }

        if !self.validate_selected_transport("Syslog scan") {
            return;
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        self.scan_stop_flag = Some(Arc::clone(&stop_flag));
        self.scanning_syslog = true;
        self.add_log("Initiating syslog monitor session...");
        self.status_msg = self
            .language
            .text("Scanning syslog... Open Wallet or tap your card on iPhone.")
            .to_string();

        let (tx, rx) = channel();
        self.task_rx = Some(rx);
        let udid = self.selected_udid.clone();
        let connection_mode = self.connection_mode;

        thread::spawn(move || {
            let tx_card = tx.clone();
            let tx_log = tx.clone();
            let res = scan_syslog_for_cards(
                udid.as_deref(),
                connection_mode,
                stop_flag,
                move |hash, name| {
                    let _ = tx_card.send(BackgroundTaskMessage::CardFound { hash, name });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg));
                },
            );
            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(
                        "Syslog scan finished".into()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(e.to_string())));
                }
            }
        });
    }

    fn flash_card(&mut self) {
        if !self.validate_selected_transport("Card flash") {
            return;
        }
        let Some(udid) = self.selected_udid.clone() else {
            return;
        };
        let hash = self.card_hash.trim().to_string();
        if hash.is_empty() {
            self.add_log("Flash failed: Target card hash is empty.");
            self.status_msg = self
                .language
                .text("Please enter or scan a target card hash.")
                .to_string();
            return;
        }
        let Some(skin) = self.skin.as_ref() else {
            self.add_log("Flash failed: No skin image prepared.");
            self.status_msg = self
                .language
                .text("Please choose a card skin image first.")
                .to_string();
            return;
        };

        let png_bytes = skin.png.clone();
        let pdf_bytes = skin.pdf.clone();
        if let Some(ref flag) = self.scan_stop_flag {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.scanning_syslog = false;
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = 3;
        self.progress_msg = "Initiating card flash...".to_string();
        self.status_msg = self
            .language
            .text("Writing card skin to iPhone...")
            .to_string();
        let connection_mode = self.connection_mode;
        let language = self.language;
        self.add_log(format!(
            "Starting card skin flash for hash: {} (UDID: {}, transport: {})",
            hash,
            udid,
            connection_mode.label()
        ));

        let (tx, rx) = channel();
        self.task_rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tx_log = tx.clone();
            let res = flash_wallet_skin(
                &udid,
                connection_mode,
                &hash,
                &png_bytes,
                &pdf_bytes,
                move |step, total, msg| {
                    let _ = tx_progress.send(BackgroundTaskMessage::Progress {
                        step,
                        total,
                        message: msg.to_string(),
                    });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg.to_string()));
                },
            );

            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(
                        language
                            .text("Card skin successfully flashed! Force quit Wallet on iPhone and reopen it.")
                            .into(),
                    )));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", e))));
                }
            }
        });
    }

    fn restore_original_card(&mut self) {
        if !self.validate_selected_transport("Restore original card") {
            return;
        }
        let Some(udid) = self.selected_udid.clone() else {
            return;
        };
        let hash = self.card_hash.trim().to_string();
        if hash.is_empty() {
            self.add_log("Restore failed: Target card hash is empty.");
            self.status_msg = self
                .language
                .text("Please enter or scan a target card hash.")
                .to_string();
            return;
        }
        if !backup_exists(&udid, &hash) {
            self.add_log("Restore failed: Original card face backup not found.");
            self.status_msg = self
                .language
                .text("Original card backup not found.")
                .to_string();
            return;
        }

        if let Some(ref flag) = self.scan_stop_flag {
            flag.store(true, Ordering::Relaxed);
        }
        self.scanning_syslog = false;
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = 3;
        self.progress_msg = "Restoring original card face...".to_string();
        self.status_msg = self
            .language
            .text("Restoring original card face...")
            .to_string();
        let connection_mode = self.connection_mode;
        let language = self.language;
        self.add_log(format!(
            "Starting original card face restore for hash: {} (UDID: {}, transport: {})",
            hash,
            udid,
            connection_mode.label()
        ));

        let (tx, rx) = channel();
        self.task_rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tx_log = tx.clone();
            let res = restore_wallet_original(
                &udid,
                connection_mode,
                &hash,
                move |step, total, msg| {
                    let _ = tx_progress.send(BackgroundTaskMessage::Progress {
                        step,
                        total,
                        message: msg.to_string(),
                    });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg.to_string()));
                },
            );

            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(language
                        .text("Original card face restored. Force close Wallet and reopen it.")
                        .into())));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", e))));
                }
            }
        });
    }

    fn select_theme_file(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Passcode Theme", &["passthm", "passtheme", "zip"])
            .pick_file()
        else {
            return;
        };

        self.load_theme_from_path(ctx, &path);
    }

    fn load_theme_from_path(&mut self, ctx: &egui::Context, path: &Path) {
        self.add_log(format!(
            "Opening passcode theme package: {}",
            path.display()
        ));
        let target_ver = match self.forced_telephony_ver.as_str() {
            "TelephonyUI-10" => Some("TelephonyUI-10"),
            "TelephonyUI-9" => Some("TelephonyUI-9"),
            "TelephonyUI-8" => Some("TelephonyUI-8"),
            _ => Some("TelephonyUI-10"),
        };

        match parse_passthm_file(path, target_ver, &self.keypad_language, self.passcode_bold) {
            Ok(theme) => {
                self.keypad_textures.clear();
                for (digit, bytes) in &theme.key_previews {
                    if let Ok(img) = image::load_from_memory(bytes) {
                        let rgba = img.to_rgba8();
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            &rgba,
                        );
                        let tex = ctx.load_texture(
                            format!("keypad-{}", digit),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );
                        self.keypad_textures.push((digit.clone(), tex));
                    }
                }
                self.keypad_textures.sort_by(|a, b| a.0.cmp(&b.0));

                self.add_log(format!(
                    "Passcode theme loaded: '{}' (telephony: {}, lang: {}, bold: {}, {} assets)",
                    theme.name,
                    theme.detected_version,
                    self.keypad_language,
                    self.passcode_bold,
                    theme.items.len()
                ));
                self.status_msg = format!(
                    "{} '{}' - {} {}, {} {}, {} {}, {} {}",
                    self.language.text("Loaded").as_ref(),
                    theme.name,
                    theme.items.len(),
                    self.language.text("assets").as_ref(),
                    self.language.text("target:").as_ref(),
                    theme.detected_version,
                    self.language.text("lang:").as_ref(),
                    self.keypad_language,
                    self.language.text("bold:").as_ref(),
                    self.language
                        .text(if self.passcode_bold { "ON" } else { "OFF" })
                );
                self.theme_path = Some(path.to_path_buf());
                self.loaded_theme = Some(theme);
            }
            Err(err) => {
                self.add_log(format!("Failed to parse theme: {err:#}"));
                self.status_msg = format!(
                    "{} {err:#}",
                    self.language.text("Failed to parse theme:").as_ref()
                );
            }
        }
    }

    fn flash_theme(&mut self) {
        if !self.validate_selected_transport("Theme flash") {
            return;
        }
        let Some(udid) = self.selected_udid.clone() else {
            return;
        };
        let Some(theme) = self.loaded_theme.as_ref() else {
            self.add_log("Theme flash failed: No .passthm theme loaded.");
            self.status_msg = self
                .language
                .text("Please select a .passthm theme file first.")
                .to_string();
            return;
        };

        let items = theme.items.clone();
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = items.len();
        self.progress_msg = "Starting passcode theme flash...".to_string();
        self.status_msg = self
            .language
            .text("Writing passcode theme buttons...")
            .to_string();
        let connection_mode = self.connection_mode;
        let language = self.language;
        self.add_log(format!(
            "Flashing passcode theme '{}' ({} button assets) to device {} over {}",
            theme.name,
            items.len(),
            udid,
            connection_mode.label()
        ));

        let (tx, rx) = channel();
        self.task_rx = Some(rx);

        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tx_log = tx.clone();
            let res = flash_passcode_theme(
                &udid,
                connection_mode,
                &items,
                move |step, total, msg| {
                    let _ = tx_progress.send(BackgroundTaskMessage::Progress {
                        step,
                        total,
                        message: msg.to_string(),
                    });
                },
                move |msg| {
                    let _ = tx_log.send(BackgroundTaskMessage::Log(msg.to_string()));
                },
            );

            match res {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(language
                        .text("Passcode theme applied! Lock your iPhone to view the new keypad.")
                        .into())));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", e))));
                }
            }
        });
    }

    fn handle_messages(&mut self) {
        let mut messages = Vec::new();
        if let Some(ref rx) = self.task_rx {
            while let Ok(msg) = rx.try_recv() {
                messages.push(msg);
            }
        }

        let mut finished = false;
        for msg in messages {
            match msg {
                BackgroundTaskMessage::Progress {
                    step,
                    total,
                    message,
                } => {
                    self.progress_step = step;
                    self.progress_total = total;
                    let localized_message = self.language.text(&message).to_string();
                    self.progress_msg = localized_message.clone();
                    let msg_str = format!("[{}/{}] {}", step, total, localized_message);
                    self.add_log(&msg_str);
                    self.status_msg = msg_str;
                }
                BackgroundTaskMessage::Log(log_line) => {
                    self.add_log(log_line);
                }
                BackgroundTaskMessage::CardFound { hash, name } => {
                    self.card_hash = hash.clone();
                    self.saved_cards = load_saved_cards();
                    let msg_str = format!(
                        "{}: {} ({})",
                        self.language.text("Card captured").as_ref(),
                        name,
                        hash
                    );
                    self.add_log(&msg_str);
                    self.status_msg = msg_str;
                }
                BackgroundTaskMessage::Done(res) => {
                    self.is_busy = false;
                    self.scanning_syslog = false;
                    finished = true;
                    match res {
                        Ok(ok_msg) => {
                            self.add_log(format!("Operation completed: {}", ok_msg));
                            self.status_msg = ok_msg;
                        }
                        Err(err_msg) => {
                            self.add_log(format!("Operation failed: {}", err_msg));
                            self.status_msg =
                                format!("{}{}", self.language.text("Error: ").as_ref(), err_msg);
                        }
                    }
                }
            }
        }
        if finished {
            self.task_rx = None;
        }
    }
}

pub mod md3 {
    use eframe::egui::Color32;

    // M3 Dark scheme
    pub const SURFACE: Color32 = Color32::from_rgb(18, 18, 20);
    pub const SURFACE_CONTAINER: Color32 = Color32::from_rgb(33, 31, 36);
    pub const SURFACE_CONTAINER_HIGH: Color32 = Color32::from_rgb(43, 41, 48);
    pub const SURFACE_CONTAINER_HIGHEST: Color32 = Color32::from_rgb(54, 52, 59);
    pub const ON_SURFACE: Color32 = Color32::from_rgb(230, 225, 229);
    pub const ON_SURFACE_VARIANT: Color32 = Color32::from_rgb(196, 199, 197);
    pub const OUTLINE: Color32 = Color32::from_rgb(147, 143, 153);
    pub const OUTLINE_VARIANT: Color32 = Color32::from_rgb(73, 69, 79);

    // Primary
    pub const PRIMARY: Color32 = Color32::from_rgb(208, 188, 255);
    pub const ON_PRIMARY: Color32 = Color32::from_rgb(56, 30, 114);
    pub const PRIMARY_CONTAINER: Color32 = Color32::from_rgb(79, 55, 139);
    pub const ON_PRIMARY_CONTAINER: Color32 = Color32::from_rgb(234, 221, 255);

    // Secondary
    pub const SECONDARY_CONTAINER: Color32 = Color32::from_rgb(74, 68, 88);
    pub const ON_SECONDARY_CONTAINER: Color32 = Color32::from_rgb(232, 222, 248);

    // Tertiary
    pub const TERTIARY_CONTAINER: Color32 = Color32::from_rgb(99, 59, 72);
    pub const ON_TERTIARY_CONTAINER: Color32 = Color32::from_rgb(255, 216, 228);

    // Error
    pub const ERROR: Color32 = Color32::from_rgb(242, 184, 181);
    pub const ERROR_CONTAINER: Color32 = Color32::from_rgb(140, 29, 24);

    // Extra
    pub const SUCCESS: Color32 = Color32::from_rgb(120, 220, 120);
    pub const WARNING: Color32 = Color32::from_rgb(255, 205, 120);
}

fn draw_status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn setup_custom_fonts(ctx: &egui::Context) {
    let windows_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts_dir = windows_dir.join("Fonts");

    let mut fonts = egui::FontDefinitions::default();

    // Primary Latin / UI font (Segoe UI is standard on Windows)
    if let Ok(segoe_bytes) = std::fs::read(fonts_dir.join("segoeui.ttf")) {
        fonts.font_data.insert(
            "segoe-ui".to_owned(),
            egui::FontData::from_owned(segoe_bytes).into(),
        );
        if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            prop.insert(0, "segoe-ui".to_owned());
        }
    }

    // Fallback CJK font for Chinese characters
    let cjk_candidates = [
        fonts_dir.join("msyh.ttc"),
        fonts_dir.join("msyh.ttf"),
        fonts_dir.join("Deng.ttf"),
        fonts_dir.join("simhei.ttf"),
        fonts_dir.join("simsun.ttc"),
        fonts_dir.join("simsunb.ttf"),
    ];

    if let Some(cjk_bytes) = cjk_candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert(
            "windows-cjk".to_owned(),
            egui::FontData::from_owned(cjk_bytes).into(),
        );
        // Important: PUSH to the back as a fallback so it doesn't override English/Latin glyphs!
        if let Some(prop) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            prop.push("windows-cjk".to_owned());
        }
        if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            mono.push("windows-cjk".to_owned());
        }
    }

    ctx.set_fonts(fonts);
}

fn setup_custom_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = md3::SURFACE;
    visuals.window_fill = md3::SURFACE;
    visuals.extreme_bg_color = md3::SURFACE_CONTAINER;
    visuals.faint_bg_color = md3::SURFACE_CONTAINER;

    visuals.window_corner_radius = 16.into();
    visuals.menu_corner_radius = 12.into();

    visuals.widgets.noninteractive.corner_radius = 12.into();
    visuals.widgets.noninteractive.bg_fill = md3::SURFACE_CONTAINER;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_SURFACE);

    visuals.widgets.inactive.bg_fill = md3::SURFACE_CONTAINER_HIGH;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_SURFACE_VARIANT);
    visuals.widgets.inactive.corner_radius = 12.into();

    visuals.widgets.hovered.bg_fill = md3::SURFACE_CONTAINER_HIGHEST;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_SURFACE);
    visuals.widgets.hovered.corner_radius = 12.into();

    visuals.widgets.active.bg_fill = md3::PRIMARY_CONTAINER;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, md3::ON_PRIMARY_CONTAINER);
    visuals.widgets.active.corner_radius = 12.into();

    visuals.widgets.open.bg_fill = md3::SURFACE_CONTAINER_HIGHEST;
    visuals.widgets.open.corner_radius = 12.into();
    visuals.widgets.open.bg_stroke = egui::Stroke::NONE;

    visuals.selection.bg_fill = md3::PRIMARY_CONTAINER;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, md3::PRIMARY);

    ctx.set_visuals(visuals);

    ctx.style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(16.0, 8.0);
    });
}

fn m3_card<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(md3::SURFACE_CONTAINER)
        .corner_radius(16)
        .inner_margin(egui::Margin::same(20))
        .show(ui, |ui| ui.vertical(add_contents).inner)
        .inner
}

fn m3_button_filled(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(egui::RichText::new(label).size(13.0).color(md3::ON_PRIMARY))
        .fill(md3::PRIMARY)
        .corner_radius(20)
        .stroke(egui::Stroke::NONE);
    ui.add(btn).clicked()
}

fn m3_button_tonal(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(
        egui::RichText::new(label)
            .size(13.0)
            .color(md3::ON_SECONDARY_CONTAINER),
    )
    .fill(md3::SECONDARY_CONTAINER)
    .corner_radius(20)
    .stroke(egui::Stroke::NONE);
    ui.add(btn).clicked()
}

fn m3_button_outlined(ui: &mut egui::Ui, label: &str) -> bool {
    let btn = egui::Button::new(egui::RichText::new(label).size(12.5).color(md3::PRIMARY))
        .fill(egui::Color32::TRANSPARENT)
        .corner_radius(20)
        .stroke(egui::Stroke::new(1.0_f32, md3::OUTLINE));
    ui.add(btn).clicked()
}

fn m3_tab(ui: &mut egui::Ui, current: &mut AppTab, target: AppTab, label: &str) {
    let selected = *current == target;
    let (bg, fg) = if selected {
        (md3::SECONDARY_CONTAINER, md3::ON_SECONDARY_CONTAINER)
    } else {
        (egui::Color32::TRANSPARENT, md3::ON_SURFACE_VARIANT)
    };
    let btn = egui::Button::new(egui::RichText::new(label).size(12.5).color(fg))
        .fill(bg)
        .corner_radius(20)
        .stroke(egui::Stroke::NONE);
    if ui.add(btn).clicked() {
        *current = target;
    }
}

fn environment_skipped(check: &EnvironmentCheck) -> bool {
    check.status == CheckStatus::Warning && check.detail.starts_with("Not checked because")
}

fn environment_title(title: &str) -> &str {
    match title {
        crate::environment::APPLE_RUNTIME => "Apple components",
        "Apple Mobile Device Service" => "Background service",
        "Apple USB device channel" => "Device channel",
        "Lockdown service access (BUID)" => "Pairing storage",
        "Selected iPhone / transport" => "iPhone connection",
        "Apple pairing / AirTraffic service" => "Sync service",
        "Check completion" | "Environment worker" => "Check completion",
        _ => title,
    }
}

fn environment_status_color(status: Option<CheckStatus>, checking: bool) -> egui::Color32 {
    if checking && status.is_none() {
        md3::PRIMARY
    } else {
        match status {
            Some(CheckStatus::Pass) => md3::SUCCESS,
            Some(CheckStatus::Warning) => md3::WARNING,
            Some(CheckStatus::Fail) => md3::ERROR,
            None => md3::OUTLINE,
        }
    }
}

fn environment_ring(
    ui: &mut egui::Ui,
    color: egui::Color32,
    checking: bool,
    completed: usize,
    generation: u64,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(68.0, 68.0), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = 25.0;
    let background = md3::OUTLINE_VARIANT;
    painter.circle_stroke(center, radius, egui::Stroke::new(5.0_f32, background));
    let progress = ui.ctx().animate_value_with_time(
        egui::Id::new(("environment_progress", generation)),
        (completed as f32 / ENVIRONMENT_TITLES.len() as f32).clamp(0.0, 1.0),
        0.35,
    );
    if progress > 0.0 {
        let points = (0..=32)
            .map(|index| {
                let angle = -std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * progress * index as f32 / 32.0;
                center + egui::vec2(angle.cos() * radius, angle.sin() * radius)
            })
            .collect::<Vec<_>>();
        painter.add(egui::Shape::line(points, egui::Stroke::new(5.0_f32, color)));
    }
    if checking {
        let time = ui.input(|input| input.time) as f32;
        let phase = (time * 3.0 + generation as f32 * 0.45).sin() * 0.5 + 0.5;
        let angle = -std::f32::consts::FRAC_PI_2 + time * 2.4;
        let marker = center + egui::vec2(angle.cos() * radius, angle.sin() * radius);
        painter.circle_filled(marker, 5.0 + phase * 1.5, color.gamma_multiply(0.85));
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
    let count_label = format!("{completed}/{}", ENVIRONMENT_TITLES.len());
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        count_label,
        egui::FontId::proportional(14.0),
        color,
    );
}

fn environment_row(
    ui: &mut egui::Ui,
    language: Language,
    title: &str,
    check: Option<&EnvironmentCheck>,
    checking: bool,
    active: bool,
    generation: u64,
) {
    let skipped = check.is_some_and(environment_skipped);
    let status = check.filter(|_| !skipped).map(|item| item.status);
    let active = checking && active && check.is_none();
    let color = environment_status_color(status, active);
    let reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new(("environment_result", title, generation)),
        check.is_some(),
        0.3,
    );
    let status_label = if skipped {
        "Not checked"
    } else if let Some(check) = check {
        check.status.label()
    } else if active {
        "Checking"
    } else if checking {
        "Waiting"
    } else {
        "Not checked"
    };
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, 5))
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(22.0, 24.0), egui::Sense::hover());
                let center = rect.center();
                let pulse = if active {
                    0.6 + 0.3 * (ui.input(|input| input.time) as f32 * 3.0).sin()
                } else {
                    1.0
                };
                ui.painter().circle_filled(
                    center,
                    10.0,
                    color.gamma_multiply(0.12 + reveal * 0.08),
                );
                if active {
                    ui.painter()
                        .circle_filled(center, 4.0 + pulse, color.gamma_multiply(pulse));
                    ui.ctx().request_repaint_after(Duration::from_millis(33));
                } else if status == Some(CheckStatus::Pass) {
                    ui.painter().add(egui::Shape::line(
                        vec![
                            center + egui::vec2(-4.0, 0.0),
                            center + egui::vec2(-1.0, 3.0),
                            center + egui::vec2(5.0, -3.0),
                        ],
                        egui::Stroke::new(1.8_f32, color),
                    ));
                } else {
                    ui.painter().text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        if status.is_some() { "!" } else { "–" },
                        egui::FontId::proportional(13.0),
                        color,
                    );
                }
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(language.text(environment_title(title)).as_ref())
                                .strong()
                                .size(12.5),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            egui::Frame::new()
                                .fill(color.gamma_multiply(0.12))
                                .corner_radius(6)
                                .inner_margin(egui::Margin::symmetric(6, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new(language.text(status_label).as_ref())
                                            .size(10.0)
                                            .color(color),
                                    );
                                });
                        });
                    });
                    let detail = match check {
                        Some(check) => environment_summary(language, check),
                        None if active => language.text("Keep iPhone unlocked.").into_owned(),
                        None if checking => {
                            language.text("Waiting for previous checks").into_owned()
                        }
                        None => language.text("The check did not finish").into_owned(),
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(detail)
                                .size(11.0)
                                .color(md3::ON_SURFACE_VARIANT),
                        )
                        .wrap(),
                    );
                });
            });
        });
}

fn environment_summary(language: Language, check: &EnvironmentCheck) -> String {
    let key = if environment_skipped(check) {
        "Check the prerequisites first"
    } else {
        match (check.title.as_str(), check.status) {
            (crate::environment::APPLE_RUNTIME, CheckStatus::Pass) => "Apple libraries loaded",
            (crate::environment::APPLE_RUNTIME, _) => {
                "Install or repair 64-bit desktop iTunes from Apple."
            }
            ("Apple Mobile Device Service", CheckStatus::Pass) => "Service is running",
            ("Apple Mobile Device Service", CheckStatus::Warning) => "Service state is unknown",
            ("Apple Mobile Device Service", _) => "Service is stopped or missing",
            ("Apple USB device channel", CheckStatus::Pass) => "usbmuxd is responding",
            ("Apple USB device channel", _) => "Device channel unavailable",
            ("Lockdown service access (BUID)", CheckStatus::Pass) => "Service identity readable",
            ("Lockdown service access (BUID)", _) => "Service cannot read BUID",
            ("Selected iPhone / transport", CheckStatus::Pass) => "Device found on this connection",
            ("Selected iPhone / transport", _) => "No device on this connection",
            ("Apple pairing / AirTraffic service", CheckStatus::Pass) => {
                "Native session and service available"
            }
            ("Apple pairing / AirTraffic service", _) => "Native authentication failed",
            (_, _) if check.detail.contains("timed out") => {
                "Check timed out. Reconnect and try again."
            }
            _ => "The check did not finish",
        }
    };
    let mut summary = language.text(key).into_owned();
    if check.status == CheckStatus::Fail {
        if let Some(code) = check.detail.split("0x").nth(1) {
            let hex: String = code
                .chars()
                .take_while(|ch| ch.is_ascii_hexdigit())
                .collect();
            if !hex.is_empty() {
                summary.push_str(&format!(" · 0x{hex}"));
            }
        }
    }
    summary
}

impl eframe::App for AirCardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();
        let language = self.language;
        self.handle_environment_messages();
        self.handle_setup_messages();

        if self.is_busy || self.scanning_syslog || self.environment_working() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }

        // Top bar
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(md3::SURFACE)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(language.text("AirCard").as_ref())
                            .strong()
                            .size(18.0)
                            .color(md3::ON_SURFACE),
                    );
                    ui.label(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );

                    ui.add_space(20.0);
                    m3_tab(
                        ui,
                        &mut self.current_tab,
                        AppTab::Wallet,
                        language.text("Wallet").as_ref(),
                    );
                    m3_tab(
                        ui,
                        &mut self.current_tab,
                        AppTab::Passcode,
                        language.text("Passcode").as_ref(),
                    );
                    m3_tab(
                        ui,
                        &mut self.current_tab,
                        AppTab::Environment,
                        language.text("Environment").as_ref(),
                    );
                    m3_tab(
                        ui,
                        &mut self.current_tab,
                        AppTab::Help,
                        language.text("Help").as_ref(),
                    );
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let controls_enabled =
                            !self.is_busy && !self.scanning_syslog && !self.environment_working();
                        let refresh = ui
                            .add_enabled_ui(controls_enabled, |ui| {
                                m3_button_outlined(ui, language.text("Refresh").as_ref())
                            })
                            .inner;
                        if refresh {
                            self.refresh_devices();
                        }

                        ui.add_space(6.0);
                        let mut next_mode = self.connection_mode;
                        ui.add_enabled_ui(controls_enabled, |ui| {
                            egui::ComboBox::from_id_salt("connection_mode_combo")
                                .selected_text(language.text(next_mode.label()).as_ref())
                                .width(135.0)
                                .show_ui(ui, |ui| {
                                    for mode in ConnectionMode::ALL {
                                        ui.selectable_value(
                                            &mut next_mode,
                                            mode,
                                            language.text(mode.label()).as_ref(),
                                        );
                                    }
                                });
                        });
                        if next_mode != self.connection_mode {
                            self.environment_stale = true;
                            self.connection_mode = next_mode;
                            self.add_log(format!(
                                "Transport mode changed to {}.",
                                self.connection_mode.label()
                            ));
                            self.status_msg =
                                format!("Transport mode: {}", self.connection_mode.label());
                        }

                        ui.add_space(6.0);
                        let mut next_udid = self.selected_udid.clone();
                        let selected_label = self
                            .devices
                            .iter()
                            .find(|device| Some(&device.udid) == self.selected_udid.as_ref())
                            .map(|device| {
                                format!("{} [{}]", device.name, device.transport_summary())
                            })
                            .unwrap_or_else(|| "No device".to_string());
                        ui.add_enabled_ui(controls_enabled && !self.devices.is_empty(), |ui| {
                            egui::ComboBox::from_id_salt("device_selector_combo")
                                .selected_text(language.text(&selected_label).as_ref())
                                .width(185.0)
                                .show_ui(ui, |ui| {
                                    for device in &self.devices {
                                        ui.selectable_value(
                                            &mut next_udid,
                                            Some(device.udid.clone()),
                                            format!(
                                                "{} [{}]",
                                                device.name,
                                                device.transport_summary()
                                            ),
                                        );
                                    }
                                });
                        });
                        if next_udid != self.selected_udid {
                            self.environment_stale = true;
                            self.selected_udid = next_udid;
                            if let Some(selected) = self.selected_udid.clone() {
                                self.add_log(format!("Selected device: {}", selected));
                            }
                        }

                        ui.add_space(8.0);
                        let connection_ready = self.selected_transport_available();
                        draw_status_dot(
                            ui,
                            if connection_ready {
                                md3::SUCCESS
                            } else {
                                md3::ERROR
                            },
                        );
                        ui.label(
                            egui::RichText::new(
                                language
                                    .text(if connection_ready {
                                        "Ready"
                                    } else {
                                        "Unavailable"
                                    })
                                    .as_ref(),
                            )
                            .size(12.0)
                            .color(if connection_ready {
                                md3::ON_SURFACE
                            } else {
                                md3::ON_SURFACE_VARIANT
                            }),
                        )
                        .on_hover_text(&self.apple_status);
                    });
                });
            });

        // Status bar
        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(md3::SURFACE)
                    .inner_margin(egui::Margin::symmetric(20, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let dot_col =
                        if self.is_busy || self.scanning_syslog || self.environment_working() {
                            md3::PRIMARY
                        } else if self.current_tab == AppTab::Environment
                            && self
                                .environment_checks
                                .iter()
                                .any(|check| check.status == CheckStatus::Fail)
                        {
                            md3::ERROR
                        } else if self.current_tab == AppTab::Environment
                            && (self.environment_stale
                                || self
                                    .environment_checks
                                    .iter()
                                    .any(|check| check.status == CheckStatus::Warning))
                        {
                            md3::WARNING
                        } else if self.status_msg.starts_with("Error")
                            || self.status_msg.starts_with("Failed")
                        {
                            md3::ERROR
                        } else {
                            md3::SUCCESS
                        };
                    draw_status_dot(ui, dot_col);
                    if self.is_busy || self.scanning_syslog || self.environment_working() {
                        ui.spinner();
                    }
                    ui.label(
                        egui::RichText::new(language.text(&self.status_msg).as_ref())
                            .size(11.5)
                            .color(md3::ON_SURFACE_VARIANT),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_text = if self.show_logs_window {
                            "Logs [x]"
                        } else {
                            "Logs"
                        };
                        let btn = egui::Button::new(
                            egui::RichText::new(language.text(&btn_text).as_ref())
                                .size(11.0)
                                .color(if self.show_logs_window {
                                    md3::ON_PRIMARY_CONTAINER
                                } else {
                                    md3::ON_SURFACE_VARIANT
                                }),
                        )
                        .fill(if self.show_logs_window {
                            md3::PRIMARY_CONTAINER
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .corner_radius(20)
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            if self.show_logs_window {
                                md3::PRIMARY
                            } else {
                                md3::OUTLINE_VARIANT
                            },
                        ));
                        if ui.add(btn).clicked() {
                            self.show_logs_window = !self.show_logs_window;
                        }

                        ui.add_space(8.0);

                        let mut next_language = self.language;
                        egui::ComboBox::from_id_salt("language_combo_bottom")
                            .selected_text(language.option_label(next_language))
                            .width(105.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut next_language,
                                    Language::English,
                                    language.option_label(Language::English),
                                );
                                ui.selectable_value(
                                    &mut next_language,
                                    Language::SimplifiedChinese,
                                    language.option_label(Language::SimplifiedChinese),
                                );
                            });
                        if next_language != self.language {
                            self.language = next_language;
                            if let Err(error) = self.language.save() {
                                self.status_msg =
                                    format!("Error: Could not save language: {error}");
                                self.add_log(&self.status_msg.clone());
                            } else {
                                self.status_msg = "Language changed.".into();
                            }
                            ctx.request_repaint();
                        }
                    });
                });
            });

        // Central - same SURFACE fill as header/status for flat look
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(md3::SURFACE)
                    .inner_margin(egui::Margin::same(16)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| match self.current_tab {
                    AppTab::Wallet => self.show_wallet_tab(ctx, ui),
                    AppTab::Passcode => self.show_passcode_tab(ctx, ui),
                    AppTab::Environment => self.show_environment_tab(ctx, ui),
                    AppTab::Help => self.show_help_tab(ui),
                });
            });

        let mut show_logs = self.show_logs_window;
        let mut file_saved_msg: Option<String> = None;
        if show_logs {
            egui::Window::new(language.text("Logs").as_ref())
                .open(&mut show_logs)
                .default_size([540.0, 300.0])
                .min_size([360.0, 180.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if m3_button_tonal(ui, language.text("Copy Logs").as_ref()) {
                            ctx.copy_text(self.logs.join("\n"));
                        }
                        if m3_button_outlined(ui, language.text("Save to File...").as_ref()) {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("aircard-diagnostics.log")
                                .add_filter("Log files", &["log", "txt"])
                                .save_file()
                            {
                                let content = self.logs.join("\r\n");
                                let _ = std::fs::write(&path, content);
                                file_saved_msg =
                                    Some(format!("Saved log file to {}", path.display()));
                            }
                        }
                        if m3_button_outlined(ui, language.text("Clear").as_ref()) {
                            self.logs.clear();
                        }
                        ui.label(
                            egui::RichText::new(
                                language
                                    .text(&format!("{} entries", self.logs.len()))
                                    .as_ref(),
                            )
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                        );
                    });
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(md3::SURFACE_CONTAINER_HIGH)
                        .corner_radius(12)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .stick_to_bottom(true)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.logs.is_empty() {
                                        ui.label(
                                            egui::RichText::new(
                                                language.text("No events logged yet.").as_ref(),
                                            )
                                            .size(11.0)
                                            .color(md3::ON_SURFACE_VARIANT),
                                        );
                                    } else {
                                        for line in &self.logs {
                                            ui.label(
                                                egui::RichText::new(line)
                                                    .size(10.5)
                                                    .monospace()
                                                    .color(md3::ON_SURFACE),
                                            );
                                        }
                                    }
                                });
                        });
                });
            self.show_logs_window = show_logs;
            if let Some(msg) = file_saved_msg {
                self.add_log(msg);
            }
        }
    }
}

impl AirCardApp {
    fn show_environment_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let language = self.language;
        let setup_action = crate::environment_setup::recommended_action(&self.environment_checks);
        let setup_enabled = !self.is_busy
            && !self.scanning_syslog
            && !self.environment_working()
            && !self.environment_stale
            && !self.setup_restart_required;
        if self.environment_checked_at.is_none() && !self.is_busy && !self.scanning_syslog {
            self.run_environment_check();
        }
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(language.text("Environment check").as_ref())
                        .strong()
                        .size(22.0),
                );
                ui.label(
                    egui::RichText::new(
                        language
                            .text("Connection essentials, at a glance.")
                            .as_ref(),
                    )
                    .size(12.0)
                    .color(md3::ON_SURFACE_VARIANT),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(
                    !self.environment_working() && !self.is_busy && !self.scanning_syslog,
                    |ui| {
                        if m3_button_outlined(ui, language.text("Check again").as_ref()) {
                            self.run_environment_check();
                        }
                    },
                );
                ui.add_enabled_ui(
                    !self.environment_working() && !self.environment_checks.is_empty(),
                    |ui| {
                        if m3_button_outlined(ui, language.text("Copy report").as_ref()) {
                            self.copy_environment_report(ctx);
                        }
                    },
                );
                if let Some(action) = setup_action {
                    ui.add_enabled_ui(setup_enabled, |ui| {
                        if m3_button_filled(ui, language.text("One-click setup").as_ref()) {
                            self.run_environment_setup(action);
                        }
                    });
                }
            });
        });
        if self.setup_rx.is_some() {
            ui.add_space(12.0);
            egui::Frame::new().fill(md3::PRIMARY_CONTAINER).corner_radius(12)
                .inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(language.text("Configuring environment… Complete the Windows prompt if shown.").as_ref());
                    });
                });
        } else if let Some((notice, failed)) = self.setup_notice.clone() {
            ui.add_space(12.0);
            let notice_color = if failed { md3::ERROR } else { md3::PRIMARY };
            egui::Frame::new()
                .fill(notice_color.gamma_multiply(0.1))
                .corner_radius(12)
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(language.text(&notice).as_ref()).color(notice_color),
                    );
                    if self.setup_restart_required
                        && !self.setup_reboot_required
                        && m3_button_filled(ui, language.text("Restart AirCard").as_ref())
                    {
                        self.restart_after_setup(ctx);
                    }
                });
        }
        ui.add_space(16.0);
        let checking = self.environment_rx.is_some();
        let failures = self
            .environment_checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .count();
        let completed = ENVIRONMENT_TITLES
            .iter()
            .filter(|title| {
                self.environment_checks
                    .iter()
                    .any(|check| &check.title == *title)
            })
            .count();
        let skipped = self
            .environment_checks
            .iter()
            .filter(|check| environment_skipped(check))
            .count()
            + if checking {
                0
            } else {
                ENVIRONMENT_TITLES.len() - completed
            };
        let warnings = self
            .environment_checks
            .iter()
            .filter(|check| check.status == CheckStatus::Warning && !environment_skipped(check))
            .count();
        let passed = self
            .environment_checks
            .iter()
            .filter(|check| check.status == CheckStatus::Pass)
            .count();
        let color = if checking || self.setup_rx.is_some() {
            md3::PRIMARY
        } else if failures > 0 {
            md3::ERROR
        } else if warnings > 0 || skipped > 0 || self.environment_stale {
            md3::WARNING
        } else if self.environment_checks.is_empty() {
            md3::ON_SURFACE_VARIANT
        } else {
            md3::SUCCESS
        };
        let headline =
            if self.setup_rx.is_some() {
                "Configuring environment"
            } else if self.setup_reboot_required {
                "Restart Windows"
            } else if self.setup_restart_required {
                "Restart AirCard"
            } else if checking {
                "Checking your setup"
            } else if self.environment_stale {
                "Check this connection again"
            } else if failures > 0 {
                "Setup needs attention"
            } else if self.environment_checks.iter().any(|check| {
                check.title == ENVIRONMENT_TITLES[4] && check.status != CheckStatus::Pass
            }) {
                "Connect your iPhone"
            } else if warnings > 0 || skipped > 0 {
                "Setup needs attention"
            } else if !self.environment_checks.is_empty() && skipped == 0 {
                "Environment ready"
            } else {
                "Finish the current operation first."
            };
        egui::Frame::new().fill(md3::SURFACE_CONTAINER).corner_radius(18)
            .inner_margin(egui::Margin::same(18)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    environment_ring(ui, color, checking, completed, self.environment_generation);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(language.text(headline).as_ref()).strong().size(18.0).color(color));
                        let summary = if checking { language.text("Keep iPhone unlocked. Maximum 20 seconds.").into_owned() }
                            else { language.choose(
                                &format!("{passed} passed · {failures} failed · {warnings} to resolve · {skipped} not checked"),
                                &format!("{passed} 项通过 · {failures} 项失败 · {warnings} 项待处理 · {skipped} 项未检查")
                            ).to_owned() };
                        ui.label(egui::RichText::new(summary).size(12.0).color(md3::ON_SURFACE_VARIANT));
                        if let Some(time) = &self.environment_checked_at {
                            ui.label(egui::RichText::new(format!("{} {} · {}", language.choose("Checked", "检测时间"), &time[..8], language.text(self.connection_mode.label())))
                                .size(10.5).color(md3::OUTLINE));
                        }
                    });
                });
            });
        if self.environment_stale
            && self.setup_rx.is_none()
            && !self.setup_restart_required
            && self.setup_notice.is_none()
        {
            ui.add_space(6.0);
            ui.colored_label(
                md3::WARNING,
                language.text("Selection changed. Check again.").as_ref(),
            );
        }
        ui.add_space(14.0);
        let width = ui.available_width();
        let side_width = (width * 0.32).clamp(250.0, 300.0);
        let main_width = width - side_width - 16.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(main_width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(main_width);
                egui::Frame::new().fill(md3::SURFACE_CONTAINER).corner_radius(16)
                    .inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                        ui.set_width(main_width - 28.0);
                        ui.label(egui::RichText::new(language.text("Checks").as_ref()).strong().size(13.0));
                        ui.add_space(6.0);
                        for (index, title) in ENVIRONMENT_TITLES.iter().enumerate() {
                            let check = self.environment_checks.iter().find(|check| check.title == *title);
                            environment_row(ui, language, title, check, checking, index == self.environment_checks.len(), self.environment_generation);
                            if index < 5 { ui.separator(); }
                        }
                        for check in self.environment_checks.iter().filter(|check| !ENVIRONMENT_TITLES.contains(&check.title.as_str())) {
                            ui.separator();
                            environment_row(ui, language, &check.title, Some(check), false, false, self.environment_generation);
                        }
                    });
                ui.add_space(8.0);
                ui.label(egui::RichText::new(language.text("Check the result on iPhone after applying artwork.").as_ref()).size(10.5).color(md3::OUTLINE));
            });
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(egui::vec2(side_width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(side_width);
                let next = self.environment_checks.iter().find(|check| check.status == CheckStatus::Fail)
                    .or_else(|| self.environment_checks.iter().find(|check| check.status == CheckStatus::Warning && !environment_skipped(check)));
                if let Some(check) = next.cloned() {
                    let next_title = environment_title(&check.title).to_owned();
                    let next_action = check.action.clone();
                    let can_open_services = check.title.contains("Service") && check.title != ENVIRONMENT_TITLES[5];
                    egui::Frame::new().fill(color.gamma_multiply(0.10)).stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.30)))
                        .corner_radius(16).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                            ui.set_width(side_width - 32.0);
                            ui.label(egui::RichText::new(language.text("Next step").as_ref()).size(10.5).color(color));
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(language.text(&next_title).as_ref()).strong().size(15.0));
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(language.text(&next_action).as_ref()).size(12.0));
                            if let Some(action) = setup_action {
                                ui.add_space(10.0);
                                ui.add_enabled_ui(setup_enabled, |ui| {
                                    if m3_button_filled(ui, language.text(action.label()).as_ref()) {
                                        self.run_environment_setup(action);
                                    }
                                });
                                ui.add(egui::Label::new(egui::RichText::new(language.text(action.description()).as_ref()).size(10.5).color(md3::ON_SURFACE_VARIANT)).wrap());
                            }
                            if can_open_services {
                                ui.add_space(8.0);
                                if m3_button_tonal(ui, language.text("Open Services").as_ref()) { self.open_services(); }
                            }
                        });
                    ui.add_space(12.0);
                }
                egui::Frame::new().fill(md3::SURFACE_CONTAINER).corner_radius(16)
                    .inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                        ui.set_width(side_width - 32.0);
                        ui.label(egui::RichText::new(language.text("Quick setup").as_ref()).strong().size(13.0));
                        for (number, title, description) in [
                            ("1", "Install Apple support", "Install 64-bit desktop iTunes."),
                            ("2", "Connect & trust", "Connect USB, open iTunes, then tap Trust on iPhone."),
                            ("3", "Close iTunes", "Keep iPhone unlocked before applying artwork."),
                        ] {
                            ui.add_space(12.0);
                            ui.horizontal_top(|ui| {
                                ui.label(egui::RichText::new(number).strong().size(12.0).color(md3::PRIMARY));
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new(language.text(title).as_ref()).strong().size(12.0));
                                    ui.label(egui::RichText::new(language.text(description).as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                                });
                            });
                        }
                        ui.add_space(12.0);
                        ui.hyperlink_to(language.text("Apple setup guide").as_ref(), crate::environment::APPLE_HELP_URL);
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(language.text("Use WiFi").as_ref()).show(ui, |ui| {
                            ui.label(language.text("Enable WiFi sync, use the same network, then unplug USB and refresh.").as_ref());
                        });
                    });
            });
        });
        ui.add_space(12.0);
        egui::CollapsingHeader::new(language.text("Technical details").as_ref())
            .id_salt("environment_technical_details")
            .show(ui, |ui| {
                for check in &self.environment_checks {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(&check.title).strong().size(12.0));
                    ui.add(
                        egui::Label::new(egui::RichText::new(&check.detail).monospace().size(11.0))
                            .wrap(),
                    );
                    if !check.action.is_empty() {
                        ui.add(egui::Label::new(language.text(&check.action).as_ref()).wrap());
                    }
                }
            });
        egui::CollapsingHeader::new(language.text("Advanced help").as_ref()).show(ui, |ui| {
            ui.add_enabled_ui(!self.environment_working() && !self.is_busy && !self.scanning_syslog && !self.setup_restart_required, |ui| {
                if m3_button_tonal(ui, language.text(SetupAction::RepairApple.label()).as_ref()) {
                    self.run_environment_setup(SetupAction::RepairApple);
                }
            });
            ui.label(language.text(SetupAction::RepairApple.description()).as_ref());
            ui.add_space(8.0);
            ui.label(language.text("Restart in Administrator PowerShell:").as_ref());
            ui.horizontal_wrapped(|ui| {
                ui.monospace("Restart-Service -Name 'Apple Mobile Device Service'");
                if ui.small_button(language.text("Copy command").as_ref()).clicked() {
                    ctx.copy_text("Restart-Service -Name 'Apple Mobile Device Service'".into());
                }
                if ui.small_button(language.text("Open Services").as_ref()).clicked() { self.open_services(); }
            });
            ui.add_space(6.0);
            ui.label(language.text("BUID failures can cause 0xE8000003 or a sync timeout. Check the service Log On account and its Lockdown access.").as_ref());
            ui.label(language.text("Pairing files are in %ProgramData%\\Apple\\Lockdown. Repair Apple support if permissions are wrong.").as_ref());
            ui.horizontal_wrapped(|ui| {
                for (label, folder) in [
                    ("Open Apple folder", crate::apple::locate_support_dir()),
                    ("Open Lockdown folder", std::env::var_os("PROGRAMDATA").map(|root| PathBuf::from(root).join("Apple").join("Lockdown"))),
                ] {
                    if let Some(folder) = folder.filter(|path| path.is_dir()) {
                        if m3_button_outlined(ui, language.text(label).as_ref()) {
                            if let Err(error) = std::process::Command::new("explorer.exe").arg(folder).spawn() {
                                self.status_msg = format!("Error: Could not open folder: {error}");
                            }
                        }
                    }
                }
            });
        });
    }

    fn copy_environment_report(&mut self, ctx: &egui::Context) {
        let mut checks = self
            .environment_checks
            .iter()
            .map(|check| {
                format!(
                    "{}: {}\n{}\nNext: {}",
                    check.title,
                    if environment_skipped(check) {
                        "Not checked"
                    } else {
                        check.status.label()
                    },
                    check.detail,
                    check.action
                )
            })
            .collect::<Vec<_>>();
        for title in ENVIRONMENT_TITLES {
            if !self
                .environment_checks
                .iter()
                .any(|check| check.title == title)
            {
                checks.push(format!(
                    "{title}: Not checked\nNo result was returned.\nNext: Run the checks again."
                ));
            }
        }
        ctx.copy_text(format!(
            "AirCard v{} | Windows {}\nChecked: {}\nScope: {}\nSelection changed: {}\nSetup: {}\n\n{}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::ARCH,
            self.environment_checked_at
                .as_deref()
                .unwrap_or("Not checked"),
            self.environment_scope,
            self.environment_stale,
            self.setup_notice.as_ref().map(|(notice, _)| notice.as_str()).unwrap_or("Not run"),
            checks.join("\n\n")
        ));
        self.status_msg = "Environment report copied to clipboard.".into();
    }

    fn open_services(&mut self) {
        match std::process::Command::new("mmc.exe")
            .arg("services.msc")
            .spawn()
        {
            Ok(_) => self.status_msg = "Services opened. Find Apple Mobile Device Service.".into(),
            Err(error) => self.status_msg = format!("Error: Could not open Services: {error}"),
        }
    }

    fn show_wallet_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let language = self.language;
        if self.scanning_syslog {
            egui::Frame::new()
                .fill(md3::TERTIARY_CONTAINER)
                .corner_radius(16)
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(language.text("Scanning syslog...").as_ref())
                                    .strong()
                                    .size(13.0)
                                    .color(md3::ON_TERTIARY_CONTAINER),
                            );
                            ui.label(
                                egui::RichText::new(
                                    language
                                        .text("Open Wallet on iPhone and tap your card")
                                        .as_ref(),
                                )
                                .size(11.5)
                                .color(md3::ON_TERTIARY_CONTAINER),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let btn = egui::Button::new(
                                egui::RichText::new(language.text("Stop").as_ref())
                                    .size(12.0)
                                    .color(md3::ON_SURFACE),
                            )
                            .fill(md3::ERROR_CONTAINER)
                            .corner_radius(20)
                            .stroke(egui::Stroke::NONE);
                            if ui.add(btn).clicked() {
                                self.toggle_syslog_scan();
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }

        ui.columns(2, |cols| {
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(
                    egui::RichText::new(language.text("Card Configuration").as_ref())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        language
                            .text("Target your card and choose replacement artwork")
                            .as_ref(),
                    )
                    .size(12.0)
                    .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(16.0);

                // Target Card Hash
                ui.label(
                    egui::RichText::new(language.text("Target Card Hash").as_ref())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let btn_w = 90.0;
                    let text_w = (ui.available_width() - btn_w - 12.0).max(150.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.card_hash)
                            .hint_text(language.text("Base64 pass hash...").as_ref())
                            .desired_width(text_w),
                    );

                    let scan_label = if self.scanning_syslog { "Stop" } else { "Scan" };
                    let scan_bg = if self.scanning_syslog {
                        md3::ERROR_CONTAINER
                    } else {
                        md3::PRIMARY_CONTAINER
                    };
                    let scan_fg = if self.scanning_syslog {
                        md3::ERROR
                    } else {
                        md3::ON_PRIMARY_CONTAINER
                    };
                    let scan_btn = egui::Button::new(
                        egui::RichText::new(language.text(&scan_label).as_ref())
                            .size(12.0)
                            .color(scan_fg),
                    )
                    .fill(scan_bg)
                    .corner_radius(20)
                    .stroke(egui::Stroke::NONE);
                    if ui
                        .add_enabled(
                            !self.environment_working() && !self.setup_restart_required,
                            scan_btn,
                        )
                        .clicked()
                    {
                        self.toggle_syslog_scan();
                    }
                });

                if !self.saved_cards.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(language.text("Saved cards").as_ref())
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.add_space(2.0);
                    let combo_w = (ui.available_width() - 4.0).max(150.0);
                    let sel_label = self
                        .saved_cards
                        .iter()
                        .find(|c| c.hash == self.card_hash)
                        .map(|c| format!("{} ({})", c.name, &c.hash[..8.min(c.hash.len())]))
                        .unwrap_or_else(|| language.text("Select...").into());

                    egui::ComboBox::from_id_salt("saved_cards_box")
                        .width(combo_w)
                        .selected_text(
                            egui::RichText::new(language.text(&sel_label).as_ref())
                                .color(md3::ON_SURFACE),
                        )
                        .show_ui(ui, |ui| {
                            for card in &self.saved_cards {
                                let is_selected = self.card_hash == card.hash;
                                let label = format!(
                                    "{} ({}...)",
                                    card.name,
                                    &card.hash[..8.min(card.hash.len())]
                                );
                                let text = egui::RichText::new(label)
                                    .color(if is_selected {
                                        md3::ON_PRIMARY_CONTAINER
                                    } else {
                                        md3::ON_SURFACE
                                    })
                                    .strong();
                                if ui.selectable_label(is_selected, text).clicked() {
                                    self.card_hash = card.hash.clone();
                                }
                            }
                        });
                }

                ui.add_space(16.0);

                // Card Skin
                ui.label(
                    egui::RichText::new(language.text("Card Skin Artwork").as_ref())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.label(
                    egui::RichText::new(
                        language
                            .text("PNG, JPG, WebP - auto-scaled to 1536x969")
                            .as_ref(),
                    )
                    .size(11.0)
                    .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(
                        language
                            .text("Drag inside the preview to reposition the crop.")
                            .as_ref(),
                    )
                    .size(11.0)
                    .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if m3_button_filled(ui, language.text("Choose Image...").as_ref()) {
                        self.select_skin(ctx);
                    }
                    if self.skin.is_some() {
                        if m3_button_tonal(ui, language.text("Export PNG").as_ref()) {
                            self.save_prepared_png();
                        }
                    }
                });

                if let Some(skin) = &self.skin {
                    ui.add_space(4.0);
                    let fname = self
                        .source_path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("image");
                    ui.label(
                        egui::RichText::new(format!(
                            "{} - 1536x969 - {:.0} KB",
                            fname,
                            skin.png.len() as f32 / 1024.0
                        ))
                        .size(11.0)
                        .color(md3::PRIMARY),
                    );
                }

                ui.add_space(16.0);

                // Apply
                ui.label(
                    egui::RichText::new(language.text("Write to iPhone").as_ref())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);

                let can_flash = !self.is_busy
                    && self.selected_transport_available()
                    && !self.card_hash.trim().is_empty()
                    && self.skin.is_some();
                let flash_btn = egui::Button::new(
                    egui::RichText::new(language.text("Apply Card Skin").as_ref())
                        .strong()
                        .size(14.0)
                        .color(if can_flash {
                            md3::ON_PRIMARY
                        } else {
                            md3::ON_SURFACE_VARIANT
                        }),
                )
                .fill(if can_flash {
                    md3::PRIMARY
                } else {
                    md3::SURFACE_CONTAINER_HIGH
                })
                .corner_radius(20)
                .stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 40.0));

                let resp = ui.add_enabled(can_flash, flash_btn);
                if resp.clicked() {
                    self.flash_card();
                }
                if !can_flash {
                    let mut r = Vec::new();
                    if self.selected_udid.is_none() {
                        r.push(language.text("connect iPhone").into_owned());
                    } else if !self.selected_transport_available() {
                        r.push(language.text("choose available transport").into_owned());
                    }
                    if self.card_hash.trim().is_empty() {
                        r.push(language.text("enter card hash").into_owned());
                    }
                    if self.skin.is_none() {
                        r.push(language.text("choose image").into_owned());
                    }
                    if !r.is_empty() {
                        resp.on_disabled_hover_text(format!(
                            "{}{}",
                            language.text("Need: ").as_ref(),
                            r.join(", ")
                        ));
                    }
                }

                ui.add_space(8.0);
                let can_restore = !self.is_busy
                    && !self.environment_working()
                    && !self.scanning_syslog
                    && self.selected_transport_available()
                    && !self.card_hash.trim().is_empty()
                    && self
                        .selected_udid
                        .as_ref()
                        .is_some_and(|udid| backup_exists(udid, self.card_hash.trim()));
                let restore_btn = egui::Button::new(
                    egui::RichText::new(language.text("Restore Original").as_ref())
                        .strong()
                        .size(13.0)
                        .color(if can_restore {
                            md3::ON_SECONDARY_CONTAINER
                        } else {
                            md3::ON_SURFACE_VARIANT
                        }),
                )
                .fill(if can_restore {
                    md3::SECONDARY_CONTAINER
                } else {
                    md3::SURFACE_CONTAINER_HIGH
                })
                .corner_radius(20)
                .stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 36.0));
                let restore_resp = ui.add_enabled(can_restore, restore_btn);
                if restore_resp.clicked() {
                    self.restore_original_card();
                }
                if !can_restore && !self.card_hash.trim().is_empty() {
                    restore_resp.on_disabled_hover_text(
                        language
                            .text("Apply a card skin once to create an original backup.")
                            .as_ref(),
                    );
                }

                if self.is_busy {
                    ui.add_space(8.0);
                    if self.progress_total > 0 {
                        ui.add(
                            egui::ProgressBar::new(
                                self.progress_step as f32 / self.progress_total as f32,
                            )
                            .animate(true),
                        );
                    }
                    ui.label(
                        egui::RichText::new(language.text(&self.progress_msg).as_ref())
                            .size(11.0)
                            .color(md3::PRIMARY),
                    );
                }
            });

            // Right: preview
            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(
                    egui::RichText::new(language.text("Wallet Preview").as_ref())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(language.text("1536 x 969 px pass canvas").as_ref())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(12.0);

                let pass_w = (ui.available_width() - 8.0).clamp(250.0, 400.0);
                let pass_h = pass_w * (969.0 / 1536.0);
                ui.vertical_centered(|ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::drag());
                    let source_dimensions = self
                        .source_image
                        .as_ref()
                        .map(|image| (image.width(), image.height()));
                    if response.dragged() {
                        if let Some((source_width, source_height)) = source_dimensions {
                            let crop_uv = crop_uv_for_card(
                                source_width,
                                source_height,
                                self.crop_focus[0],
                                self.crop_focus[1],
                            );
                            let visible_width = crop_uv[2] - crop_uv[0];
                            let visible_height = crop_uv[3] - crop_uv[1];
                            if rect.width() > 0.0 {
                                self.crop_focus[0] = (self.crop_focus[0]
                                    - response.drag_motion().x / rect.width()
                                        * (1.0 - visible_width))
                                    .clamp(0.0, 1.0);
                            }
                            if rect.height() > 0.0 {
                                self.crop_focus[1] = (self.crop_focus[1]
                                    - response.drag_motion().y / rect.height()
                                        * (1.0 - visible_height))
                                    .clamp(0.0, 1.0);
                            }
                            self.crop_dirty = true;
                            ctx.request_repaint();
                        }
                    }
                    if response.drag_stopped() && self.crop_dirty {
                        self.rebuild_skin_from_source();
                    }

                    let painter = ui.painter();
                    if let (Some(tex), Some((source_width, source_height))) =
                        (self.source_texture.as_ref(), source_dimensions)
                    {
                        let crop_uv = crop_uv_for_card(
                            source_width,
                            source_height,
                            self.crop_focus[0],
                            self.crop_focus[1],
                        );
                        painter.image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(
                                egui::pos2(crop_uv[0], crop_uv[1]),
                                egui::pos2(crop_uv[2], crop_uv[3]),
                            ),
                            egui::Color32::WHITE,
                        );
                        painter.rect_stroke(
                            rect,
                            16.0,
                            egui::Stroke::new(
                                1.0_f32,
                                egui::Color32::from_rgba_premultiplied(255, 255, 255, 30),
                            ),
                            egui::StrokeKind::Inside,
                        );
                    } else {
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            language.text("No artwork loaded").as_ref(),
                            egui::FontId::proportional(14.0),
                            md3::ON_SURFACE_VARIANT,
                        );
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(language.text("1536x969").as_ref())
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new(language.text("|").as_ref())
                            .size(11.0)
                            .color(md3::OUTLINE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new(language.text("1.585 ratio").as_ref())
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new(language.text("|").as_ref())
                            .size(11.0)
                            .color(md3::OUTLINE_VARIANT),
                    );
                    if self.skin.is_some() {
                        ui.label(
                            egui::RichText::new(language.text("Ready").as_ref())
                                .size(11.0)
                                .color(md3::SUCCESS),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(language.text("No image").as_ref())
                                .size(11.0)
                                .color(md3::ON_SURFACE_VARIANT),
                        );
                    }
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        language
                            .text("After applying, force close Apple Wallet and reopen it.")
                            .as_ref(),
                    )
                    .size(11.0)
                    .color(md3::ON_SURFACE_VARIANT),
                );
            });
        });
    }

    fn show_passcode_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let language = self.language;
        ui.columns(2, |cols| {
            // Left: config
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(egui::RichText::new(language.text("Passcode Theme").as_ref()).strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(language.text("Custom lockscreen keypad from Cowabunga or Nugget").as_ref()).size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                // Theme file
                ui.label(egui::RichText::new(language.text("Theme Package").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new(language.text("Choose a .passthm archive containing dialer artwork").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                if m3_button_filled(ui, language.text("Choose .passthm...").as_ref()) { self.select_theme_file(ctx); }

                if let Some(theme) = &self.loaded_theme {
                    let fname = self.theme_path.as_ref()
                        .and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("theme.passthm");
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("{} · {} {}", fname, theme.items.len(), language.choose("assets", "个资源"))).size(11.0).color(md3::PRIMARY));
                }

                ui.add_space(16.0);

                // iOS version
                ui.label(egui::RichText::new(language.text("Target iOS Cache").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new(language.text("Select cache format based on connected iOS version").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                let combo_w = (ui.available_width() - 4.0).max(150.0);
                let mut ver_changed = false;
                egui::ComboBox::from_id_salt("telephony_combo")
                    .width(combo_w)
                    .selected_text(language.text(&self.forced_telephony_ver).as_ref())
                    .show_ui(ui, |ui| {
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "Auto (TelephonyUI-10)".into(), language.text("Auto (TelephonyUI-10)").as_ref()).clicked();
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "TelephonyUI-10".into(), language.text("TelephonyUI-10 (iOS 18+)").as_ref()).clicked();
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "TelephonyUI-9".into(), language.text("TelephonyUI-9 (iOS 16-17)").as_ref()).clicked();
                        ver_changed |= ui.selectable_value(&mut self.forced_telephony_ver, "TelephonyUI-8".into(), language.text("TelephonyUI-8 (Legacy)").as_ref()).clicked();
                    });

                if ver_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(16.0);

                // Keypad Language
                ui.label(egui::RichText::new(language.text("Keypad Language").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.label(egui::RichText::new(language.text("Subtext alphabet layout (English, Russian, Ukrainian, Japanese, or Universal)").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(4.0);
                let mut lang_changed = false;
                egui::ComboBox::from_id_salt("keypad_lang_combo")
                    .width(combo_w)
                    .selected_text(egui::RichText::new(language.text(&self.keypad_language).as_ref()).color(md3::ON_SURFACE))
                    .show_ui(ui, |ui| {
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "English".into(), language.text("English").as_ref()).clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "Russian".into(), language.text("Russian").as_ref()).clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "Ukrainian".into(), language.text("Ukrainian").as_ref()).clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "Japanese".into(), language.text("Japanese").as_ref()).clicked();
                        lang_changed |= ui.selectable_value(&mut self.keypad_language, "All Languages (Universal)".into(), language.text("All Languages (Universal)").as_ref()).clicked();
                    });

                if lang_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(10.0);

                // Bold Font Toggle
                let mut bold_changed = false;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.passcode_bold, egui::RichText::new(language.text("Bold Text (iOS Accessibility)").as_ref()).strong().size(12.0).color(md3::ON_SURFACE)).changed() {
                        bold_changed = true;
                    }
                });
                ui.label(
                    egui::RichText::new(language.text("Generates *-bold.png for devices with Bold Text turned ON in iPhone Settings -> Display").as_ref())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );

                if bold_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(16.0);

                // Apply
                ui.label(egui::RichText::new(language.text("Write to iPhone").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);

                let can_flash = !self.is_busy
                    && self.selected_transport_available()
                    && self.loaded_theme.is_some();
                let flash_btn = egui::Button::new(
                    egui::RichText::new(language.text("Apply Passcode Theme").as_ref()).strong().size(14.0)
                        .color(if can_flash { md3::ON_PRIMARY } else { md3::ON_SURFACE_VARIANT }),
                )
                .fill(if can_flash { md3::PRIMARY } else { md3::SURFACE_CONTAINER_HIGH })
                .corner_radius(20).stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 40.0));

                let resp = ui.add_enabled(can_flash, flash_btn);
                if resp.clicked() { self.flash_theme(); }
                if !can_flash {
                    let mut r = Vec::new();
                    if self.selected_udid.is_none() { r.push(language.text("connect iPhone").into_owned()); }
                    else if !self.selected_transport_available() { r.push(language.text("choose available transport").into_owned()); }
                    if self.loaded_theme.is_none() { r.push(language.text("select theme").into_owned()); }
                    if !r.is_empty() { resp.on_disabled_hover_text(format!("{}{}", language.choose("Need: ", "需要："), r.join(language.choose(", ", "、")))); }
                }

                if self.is_busy {
                    ui.add_space(8.0);
                    if self.progress_total > 0 {
                        ui.add(egui::ProgressBar::new(self.progress_step as f32 / self.progress_total as f32).animate(true));
                    }
                    ui.label(egui::RichText::new(language.text(&self.progress_msg).as_ref()).size(11.0).color(md3::PRIMARY));
                }
            });

            // Right: preview
            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(egui::RichText::new(language.text("Keypad Preview").as_ref()).strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(language.text("Dialer button artwork").as_ref()).size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(12.0);

                let pass_w = (ui.available_width() - 8.0).clamp(240.0, 360.0);
                let pass_h = 265.0;

                ui.vertical_centered(|ui| {
                    if self.keypad_textures.is_empty() {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        painter.text(rect.center(), egui::Align2::CENTER_CENTER,
                            language.text("No theme loaded").as_ref(), egui::FontId::proportional(14.0), md3::ON_SURFACE_VARIANT);
                    } else {
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add_space(10.0);
                                const DIALER_LAYOUT: &[&[&str]] = &[
                                    &["1", "2", "3"],
                                    &["4", "5", "6"],
                                    &["7", "8", "9"],
                                    &["", "0", ""],
                                ];
                                egui::Grid::new("keypad_grid")
                                    .spacing([18.0, 6.0])
                                    .show(ui, |ui| {
                                        for row in DIALER_LAYOUT {
                                            for &d in *row {
                                                if d.is_empty() {
                                                    ui.allocate_exact_size(egui::vec2(44.0, 50.0), egui::Sense::hover());
                                                } else if let Some((_, tex)) = self.keypad_textures.iter().find(|(k, _)| k == d) {
                                                    ui.vertical_centered(|ui| {
                                                        egui::Frame::new()
                                                            .fill(md3::SURFACE)
                                                            .corner_radius(12)
                                                            .inner_margin(3)
                                                            .show(ui, |ui| { ui.image((tex.id(), egui::vec2(40.0, 40.0))); });
                                                        ui.label(egui::RichText::new(d).size(9.5).color(md3::ON_SURFACE_VARIANT));
                                                    });
                                                } else {
                                                    ui.vertical_centered(|ui| {
                                                        egui::Frame::new()
                                                            .fill(md3::SURFACE)
                                                            .corner_radius(12)
                                                            .inner_margin(3)
                                                            .show(ui, |ui| {
                                                                let (btn_rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                                                                ui.painter().rect_filled(btn_rect, 8.0, md3::SURFACE_CONTAINER);
                                                                ui.painter().text(btn_rect.center(), egui::Align2::CENTER_CENTER, d, egui::FontId::proportional(14.0), md3::ON_SURFACE_VARIANT);
                                                            });
                                                        ui.label(egui::RichText::new(d).size(9.5).color(md3::ON_SURFACE_VARIANT));
                                                    });
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    });
                            });
                        });
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(language.text("3x4 Keypad").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.label(egui::RichText::new(language.text("|").as_ref()).size(11.0).color(md3::OUTLINE_VARIANT));
                    ui.label(egui::RichText::new(language.text("TelephonyUI").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                    ui.label(egui::RichText::new(language.text("|").as_ref()).size(11.0).color(md3::OUTLINE_VARIANT));
                    if self.loaded_theme.is_some() {
                        ui.label(egui::RichText::new(language.text("Ready").as_ref()).size(11.0).color(md3::SUCCESS));
                    } else {
                        ui.label(egui::RichText::new(language.text("No theme").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
                    }
                });
                ui.add_space(8.0);
                ui.label(egui::RichText::new(language.text("After applying, lock your iPhone to see the new keypad.").as_ref()).size(11.0).color(md3::ON_SURFACE_VARIANT));
            });
        });
    }

    fn show_help_tab(&mut self, ui: &mut egui::Ui) {
        let language = self.language;
        ui.columns(2, |cols| {
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(egui::RichText::new(language.text("Setup & Card Hash Guide").as_ref()).strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(language.text("Everything you need to connect and capture your card").as_ref()).size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                ui.label(egui::RichText::new(language.text("Prerequisites").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(language.text("- 64-bit iTunes or Apple Mobile Device Support installed").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("- First-time setup: connect by USB and tap \"Trust this Computer\"").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("- WiFi: enable WiFi sync, then use the same local network").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("- Select Auto, USB only, or WiFi only in the top bar").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));

                ui.add_space(18.0);

                ui.label(egui::RichText::new(language.text("Finding Your Card Hash").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(language.text("1. Click \"Scan\" in the Wallet tab").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("2. Open Apple Wallet on your iPhone").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("3. Tap the card you want to customize").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("4. AirCard captures the pass hash automatically").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("5. Click \"Stop\" once detected").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
            });

            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(egui::RichText::new(language.text("Activation & Theme Guide").as_ref()).strong().size(16.0).color(md3::ON_SURFACE));
                ui.add_space(4.0);
                ui.label(egui::RichText::new(language.text("Applying skins and dialer keypad packages").as_ref()).size(12.0).color(md3::ON_SURFACE_VARIANT));
                ui.add_space(16.0);

                ui.label(egui::RichText::new(language.text("Activating Apple Wallet Skin").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(language.text("1. Click \"Apply Card Skin\" and wait for completion").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("2. Open App Switcher on iPhone (swipe up from bottom)").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("3. Force close Apple Wallet by swiping up on it").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("4. Reopen Wallet - your new skin appears!").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));

                ui.add_space(18.0);

                ui.label(egui::RichText::new(language.text("Passcode Themes (.passthm)").as_ref()).strong().size(12.0).color(md3::ON_SURFACE));
                ui.add_space(6.0);
                ui.label(egui::RichText::new(language.text("- Compatible with Cowabunga & Nugget theme packages").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("- iOS 18+: Select \"Auto (TelephonyUI-10)\"").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("- iOS 16-17: Select \"TelephonyUI-9\"").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
                ui.label(egui::RichText::new(language.text("- Lock screen to verify your updated keypad artwork").as_ref()).size(11.5).color(md3::ON_SURFACE_VARIANT));
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_custom_fonts() {
        let ctx = egui::Context::default();
        setup_custom_fonts(&ctx);
        // egui compiles font textures during run()
        let _ = ctx.run(Default::default(), |ctx| {
            ctx.fonts_mut(|fonts| {
                let w1 = fonts.glyph_width(&egui::FontId::proportional(14.0), 'A');
                let w2 = fonts.glyph_width(&egui::FontId::proportional(14.0), '中');
                assert!(w1 > 0.0);
                assert!(w2 > 0.0);
            });
        });
    }
}
