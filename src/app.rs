use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use eframe::egui;

use crate::apple;
use crate::card_designer::{CardDesigner, DesignerLayerKind, ImageMask, ShapeKind};
use crate::device::{ConnectionMode, DeviceInfo, DeviceTransport, list_connected_devices};
use crate::flasher::{flash_passcode_theme, flash_wallet_skin, restore_wallet_pdf};
use crate::i18n::{I18n, ensure_runtime_locales};
use crate::image_skin::PreparedSkin;
use crate::passthm::{PasscodeTheme, parse_passthm_file};
use crate::scanner::{SavedCard, load_saved_cards, scan_syslog_for_cards};
use crate::settings::AppSettings;

#[derive(PartialEq, Eq)]
enum AppTab {
    Wallet,
    Passcode,
    Help,
}

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
    apple_status: String,
    apple_ready: bool,

    // #--- JSON I18N / SETTINGS START ---
    i18n: I18n,
    settings: AppSettings,
    // #--- JSON I18N / SETTINGS END ---

    // Device management
    devices: Vec<DeviceInfo>,
    selected_udid: Option<String>,
    connection_mode: ConnectionMode,

    // Wallet tab
    card_hash: String,
    saved_cards: Vec<SavedCard>,
    source_path: Option<PathBuf>,
    skin: Option<PreparedSkin>,
    skin_texture: Option<egui::TextureHandle>,
    // #--- INTEGRATED CARD DESIGNER START ---
    designer_open: bool,
    card_designer: CardDesigner,
    designer_preview_texture: Option<egui::TextureHandle>,
    // #--- INTEGRATED CARD DESIGNER END ---
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

        // #--- JSON I18N / SETTINGS START ---
        let settings = AppSettings::load();
        let _ = ensure_runtime_locales();
        let i18n = I18n::new(&settings.language);
        let mut card_designer = CardDesigner::default();
        if let Some(draft) = settings.designer_draft.as_ref() {
            let _ = card_designer.restore_draft(draft);
        }
        let restored_card_hash = settings.last_card_hash.clone();
        // #--- JSON I18N / SETTINGS END ---

        let (apple_ready, apple_status) = match apple::verify_support() {
            Ok(msg) => (true, msg),
            Err(err) => (false, err.to_string()),
        };

        let mut app = Self {
            current_tab: AppTab::Wallet,
            apple_status,
            apple_ready,
            i18n,
            settings,

            devices: Vec::new(),
            selected_udid: None,
            connection_mode: ConnectionMode::Auto,

            card_hash: restored_card_hash,
            saved_cards: load_saved_cards(),
            source_path: None,
            skin: None,
            skin_texture: None,
            designer_open: false,
            card_designer,
            designer_preview_texture: None,
            scanning_syslog: false,
            scan_stop_flag: None,

            theme_path: None,
            loaded_theme: None,
            forced_telephony_ver: "Auto (TelephonyUI-10)".to_string(),
            keypad_language: "All Languages (Universal)".to_string(),
            passcode_bold: false,
            keypad_textures: Vec::new(),

            is_busy: false,
            progress_step: 0,
            progress_total: 0,
            progress_msg: String::new(),
            status_msg: String::new(),
            task_rx: None,
            logs: Vec::new(),
            show_logs_window: false,
        };

        app.status_msg = app.tr("status.ready");
        app.add_log("AirCard Windows v1.2.2 community v9 initialized");
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

    // #--- JSON I18N / SETTINGS START ---
    fn tr(&self, key: &str) -> String {
        self.i18n.t(key)
    }

    fn save_settings_snapshot(&mut self) {
        self.settings.language = self.i18n.language().to_string();
        self.settings.last_card_hash = self.card_hash.trim().to_string();
        self.settings.designer_draft = if self.card_designer.has_background() {
            Some(self.card_designer.draft())
        } else {
            None
        };
        if let Err(err) = self.settings.save() {
            self.add_log(format!("Could not save settings: {}", err));
        }
    }
    // #--- JSON I18N / SETTINGS END ---

    // #--- INTEGRATED CARD DESIGNER APP BRIDGE START ---
    fn refresh_designer_preview(&mut self, ctx: &egui::Context) {
        let rgba = self.card_designer.render_preview();
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        );
        self.designer_preview_texture =
            Some(ctx.load_texture("card-designer-preview", color, egui::TextureOptions::LINEAR));
        self.card_designer.dirty = false;
    }

    fn apply_designer_to_skin(&mut self, ctx: &egui::Context) {
        match self.card_designer.prepared_skin() {
            Ok(skin) => {
                self.skin_texture = Some(ctx.load_texture(
                    "card-skin-preview",
                    skin.preview.clone(),
                    egui::TextureOptions::LINEAR,
                ));
                self.source_path = self.card_designer.background_path.clone();
                self.skin = Some(skin);
                self.designer_open = false;
                self.status_msg = self.tr("wallet.prepared");
                self.add_log(
                    "Card designer output prepared: @3x 1536x969, @2x 1024x646, plus PDF.",
                );
                self.save_settings_snapshot();
            }
            Err(err) => {
                self.status_msg = format!("Designer export failed: {err:#}");
                self.add_log(self.status_msg.clone());
            }
        }
    }

    fn export_designer_png(&mut self) {
        let Ok(rgba) = self.card_designer.render_export() else {
            self.status_msg = self.tr("designer.noBackground");
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name("card-design-1536x969.png")
            .save_file()
        else {
            return;
        };
        match rgba.save(&path) {
            Ok(()) => {
                self.status_msg = format!("Saved {}", path.display());
                self.add_log(self.status_msg.clone());
            }
            Err(err) => {
                self.status_msg = format!("Could not save PNG: {err}");
                self.add_log(self.status_msg.clone());
            }
        }
    }
    // #--- INTEGRATED CARD DESIGNER APP BRIDGE END ---

    fn refresh_devices(&mut self) {
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
                    self.status_msg = self.tr("status.noDevice");
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
                        "Found {} connected device(s); transport mode: {}",
                        self.devices.len(),
                        self.connection_mode.label()
                    );
                }
            }
            Err(err) => {
                self.add_log(format!("Device scan error: {}", err));
                self.status_msg = format!("Could not enumerate devices: {}", err);
            }
        }
    }

    fn selected_transport_available(&self) -> bool {
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

    fn validate_selected_transport(&mut self, operation: &str) -> bool {
        if self.selected_udid.is_none() {
            self.add_log(format!(
                "{} failed: No connected iPhone selected.",
                operation
            ));
            self.status_msg = self.tr("status.selectDevice");
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
                "Disconnect the USB cable and refresh to guarantee the full AirTraffic path uses WiFi."
                    .to_string()
            } else {
                format!(
                    "Selected iPhone has no {} connection. Refresh devices or change transport mode.",
                    self.connection_mode.label()
                )
            };
            return false;
        }
        true
    }

    // #--- INTEGRATED CARD DESIGNER IMAGE IMPORT START ---
    fn select_skin(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file()
        else {
            return;
        };

        self.add_log(format!(
            "Opening image in integrated card designer: {}",
            path.display()
        ));
        match self.card_designer.load_background(&path) {
            Ok(()) => {
                self.source_path = Some(path.clone());
                self.skin = None;
                self.skin_texture = None;
                self.designer_open = true;
                self.refresh_designer_preview(ctx);
                self.status_msg = format!("{}: {}", self.tr("designer.title"), path.display());
                self.save_settings_snapshot();
            }
            Err(error) => {
                self.add_log(format!("Designer image load failed: {error:#}"));
                self.status_msg = format!("Could not load image: {error:#}");
            }
        }
    }
    // #--- INTEGRATED CARD DESIGNER IMAGE IMPORT END ---

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
            self.status_msg = self.tr("status.scanStopped");
            return;
        }

        if !self.validate_selected_transport("Syslog scan") {
            return;
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        self.scan_stop_flag = Some(Arc::clone(&stop_flag));
        self.scanning_syslog = true;
        self.add_log("Initiating syslog monitor session...");
        self.status_msg = self.tr("status.scanning");

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
            self.status_msg = self.tr("status.needHash");
            return;
        }
        let Some(skin) = self.skin.as_ref() else {
            self.add_log("Flash failed: No skin image prepared.");
            self.status_msg = self.tr("status.needStatic");
            return;
        };

        let png_3x = skin.png.clone();
        let png_2x = skin.png_2x.clone();
        let pdf_bytes = skin.pdf.clone();
        if let Some(ref flag) = self.scan_stop_flag {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.scanning_syslog = false;
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = 3;
        self.progress_msg = self.tr("status.prepareStatic");
        self.status_msg = self.tr("status.writeStatic");
        let connection_mode = self.connection_mode;
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
                &png_3x,
                &png_2x,
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
                        "Card skin successfully flashed! Force quit Wallet on iPhone and reopen it.".into(),
                    )));
                }
                Err(e) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", e))));
                }
            }
        });
    }

    // #--- ORIGINAL WALLET PDF RESTORE UI LOGIC START ---
    fn restore_original_wallet_pdf(&mut self) {
        if !self.validate_selected_transport("Restore original Wallet PDF") {
            return;
        }
        let Some(udid) = self.selected_udid.clone() else {
            return;
        };
        let hash = self.card_hash.trim().to_string();
        if hash.is_empty() {
            self.status_msg = self.tr("status.needHash");
            return;
        }

        let Some(path) = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_title("Select original cardBackgroundCombined.pdf")
            .pick_file()
        else {
            return;
        };
        let pdf_bytes = match std::fs::read(&path) {
            Ok(data) => data,
            Err(err) => {
                self.status_msg = format!("Could not read PDF: {err}");
                return;
            }
        };
        if !pdf_bytes.starts_with(b"%PDF-") {
            self.status_msg = "Selected file is not a valid PDF.".to_string();
            return;
        }

        if let Some(ref flag) = self.scan_stop_flag {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.scanning_syslog = false;
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = 3;
        self.progress_msg = self.tr("status.restoringPdf");
        self.status_msg = self.progress_msg.clone();
        let connection_mode = self.connection_mode;
        self.add_log(format!(
            "Starting original Wallet PDF restore: {} -> hash {} ({} bytes)",
            path.display(),
            hash,
            pdf_bytes.len()
        ));

        let (tx, rx) = channel();
        self.task_rx = Some(rx);
        thread::spawn(move || {
            let tx_progress = tx.clone();
            let tx_log = tx.clone();
            let result = restore_wallet_pdf(
                &udid,
                connection_mode,
                &hash,
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
            match result {
                Ok(()) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(
                        "Original Wallet PDF restored. Force-close Wallet and reopen it.".into(),
                    )));
                }
                Err(err) => {
                    let _ = tx.send(BackgroundTaskMessage::Done(Err(format!("{:#}", err))));
                }
            }
        });
    }
    // #--- ORIGINAL WALLET PDF RESTORE UI LOGIC END ---

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
                    "Loaded '{}' with {} assets (target: {}, lang: {}, bold: {})",
                    theme.name,
                    theme.items.len(),
                    theme.detected_version,
                    self.keypad_language,
                    if self.passcode_bold { "ON" } else { "OFF" }
                );
                self.theme_path = Some(path.to_path_buf());
                self.loaded_theme = Some(theme);
            }
            Err(err) => {
                self.add_log(format!("Failed to parse theme: {err:#}"));
                self.status_msg = format!("Failed to parse theme: {err:#}");
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
            self.status_msg = self.tr("status.needTheme");
            return;
        };

        let items = theme.items.clone();
        self.is_busy = true;
        self.progress_step = 0;
        self.progress_total = items.len();
        self.progress_msg = self.tr("status.prepareTheme");
        self.status_msg = self.tr("status.writeTheme");
        let connection_mode = self.connection_mode;
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
                    let _ = tx.send(BackgroundTaskMessage::Done(Ok(
                        "Passcode theme applied! Lock your iPhone to view the new keypad.".into(),
                    )));
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
                    self.progress_msg = message.clone();
                    let msg_str = format!("[{}/{}] {}", step, total, message);
                    self.add_log(&msg_str);
                    self.status_msg = msg_str;
                }
                BackgroundTaskMessage::Log(log_line) => {
                    self.add_log(log_line);
                }
                BackgroundTaskMessage::CardFound { hash, name } => {
                    self.card_hash = hash.clone();
                    self.saved_cards = load_saved_cards();
                    let msg_str = format!("Card captured: {} ({})", name, hash);
                    self.add_log(&msg_str);
                    self.status_msg = msg_str;
                    self.save_settings_snapshot();
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
                            self.status_msg = format!("Error: {}", err_msg);
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

// #--- PERSIST SETTINGS ON EXIT START ---
impl Drop for AirCardApp {
    fn drop(&mut self) {
        self.settings.language = self.i18n.language().to_string();
        self.settings.last_card_hash = self.card_hash.trim().to_string();
        self.settings.designer_draft = if self.card_designer.has_background() {
            Some(self.card_designer.draft())
        } else {
            None
        };
        let _ = self.settings.save();
    }
}
// #--- PERSIST SETTINGS ON EXIT END ---

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
}

fn draw_status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn setup_custom_fonts(ctx: &egui::Context) {
    // #--- V9 MULTILINGUAL UI FONT FALLBACK START ---
    // Community v9 exposes many UI languages. Load a Windows fallback chain instead of
    // relying on a single Traditional Chinese face, while keeping all fonts local/system-owned.
    let candidates = [
        ("aircard-ui", r"C:\Windows\Fonts\segoeui.ttf"),
        ("aircard-zh-tw", r"C:\Windows\Fonts\msjh.ttc"),
        ("aircard-zh-cn", r"C:\Windows\Fonts\msyh.ttc"),
        ("aircard-ja", r"C:\Windows\Fonts\YuGothR.ttc"),
        ("aircard-ko", r"C:\Windows\Fonts\malgun.ttf"),
        ("aircard-indic", r"C:\Windows\Fonts\Nirmala.ttf"),
        ("aircard-thai", r"C:\Windows\Fonts\LeelawUI.ttf"),
        ("aircard-arabic", r"C:\Windows\Fonts\arial.ttf"),
    ];

    let mut fonts = egui::FontDefinitions::default();
    let mut loaded = Vec::new();
    for (name, path) in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
        loaded.push(name.to_owned());
    }

    if !loaded.is_empty() {
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            for name in loaded.iter().rev() {
                family.insert(0, name.clone());
            }
        }
        if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            for name in &loaded {
                family.push(name.clone());
            }
        }
        ctx.set_fonts(fonts);
    }
    // #--- V9 MULTILINGUAL UI FONT FALLBACK END ---
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
    let btn = egui::Button::new(egui::RichText::new(label).size(13.0).color(md3::PRIMARY))
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

impl eframe::App for AirCardApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages();

        if self.is_busy || self.scanning_syslog {
            ctx.request_repaint();
        }

        let wallet_tab_label = self.tr("app.walletTab");
        let passcode_tab_label = self.tr("app.passcodeTab");
        let help_tab_label = self.tr("app.helpTab");
        let refresh_label = self.tr("app.refresh");

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
                        egui::RichText::new("AirCard")
                            .strong()
                            .size(18.0)
                            .color(md3::ON_SURFACE),
                    );
                    ui.label(
                        egui::RichText::new("v1.2.2")
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );

                    ui.add_space(20.0);
                    m3_tab(ui, &mut self.current_tab, AppTab::Wallet, &wallet_tab_label);
                    m3_tab(
                        ui,
                        &mut self.current_tab,
                        AppTab::Passcode,
                        &passcode_tab_label,
                    );
                    m3_tab(ui, &mut self.current_tab, AppTab::Help, &help_tab_label);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if m3_button_outlined(ui, &refresh_label) {
                            self.refresh_devices();
                        }
                        ui.add_space(4.0);

                        // #--- V9 JSON I18N SELECTOR START ---
                        let mut next_language = self.i18n.language().to_string();
                        let selected_language_label = self.i18n.language_label().to_string();
                        egui::ComboBox::from_id_salt("app_language_combo")
                            .selected_text(selected_language_label)
                            .width(150.0)
                            .show_ui(ui, |ui| {
                                for (code, label) in I18n::languages() {
                                    ui.selectable_value(
                                        &mut next_language,
                                        (*code).to_string(),
                                        *label,
                                    );
                                }
                            });
                        if next_language != self.i18n.language() {
                            self.i18n.set_language(&next_language);
                            self.save_settings_snapshot();
                            self.status_msg = self.tr("status.ready");
                        }
                        // #--- V9 JSON I18N SELECTOR END ---

                        ui.add_space(4.0);
                        let controls_enabled = !self.is_busy && !self.scanning_syslog;
                        let mut next_mode = self.connection_mode;
                        ui.add_enabled_ui(controls_enabled, |ui| {
                            egui::ComboBox::from_id_salt("connection_mode_combo")
                                .selected_text(next_mode.label())
                                .width(145.0)
                                .show_ui(ui, |ui| {
                                    for mode in ConnectionMode::ALL {
                                        ui.selectable_value(&mut next_mode, mode, mode.label());
                                    }
                                });
                        });
                        if next_mode != self.connection_mode {
                            self.connection_mode = next_mode;
                            self.add_log(format!(
                                "Transport mode changed to {}.",
                                self.connection_mode.label()
                            ));
                            self.status_msg =
                                format!("Transport mode: {}", self.connection_mode.label());
                        }

                        ui.add_space(4.0);
                        let mut next_udid = self.selected_udid.clone();
                        let selected_label = self
                            .devices
                            .iter()
                            .find(|device| Some(&device.udid) == self.selected_udid.as_ref())
                            .map(|device| {
                                format!("{} [{}]", device.name, device.transport_summary())
                            })
                            .unwrap_or_else(|| self.tr("status.noConnectedDevice"));
                        ui.add_enabled_ui(controls_enabled && !self.devices.is_empty(), |ui| {
                            egui::ComboBox::from_id_salt("device_selector_combo")
                                .selected_text(selected_label)
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
                            self.selected_udid = next_udid;
                            if let Some(selected) = self.selected_udid.clone() {
                                self.add_log(format!("Selected device: {}", selected));
                            }
                        }

                        ui.add_space(4.0);
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
                            egui::RichText::new(if connection_ready {
                                self.tr("app.ready")
                            } else {
                                self.tr("app.unavailable")
                            })
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
                    let dot_col = if self.is_busy || self.scanning_syslog {
                        md3::PRIMARY
                    } else if self.status_msg.starts_with("Error")
                        || self.status_msg.starts_with("Failed")
                        || self.status_msg.starts_with("錯誤")
                        || self.status_msg.starts_with("失敗")
                    {
                        md3::ERROR
                    } else {
                        md3::SUCCESS
                    };
                    draw_status_dot(ui, dot_col);
                    if self.is_busy || self.scanning_syslog {
                        ui.spinner();
                    }
                    ui.label(
                        egui::RichText::new(&self.status_msg)
                            .size(11.5)
                            .color(md3::ON_SURFACE_VARIANT),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn_text = if self.show_logs_window {
                            format!("{} [x]", self.tr("app.logs"))
                        } else {
                            self.tr("app.logs")
                        };
                        let btn =
                            egui::Button::new(egui::RichText::new(btn_text).size(11.0).color(
                                if self.show_logs_window {
                                    md3::ON_PRIMARY_CONTAINER
                                } else {
                                    md3::ON_SURFACE_VARIANT
                                },
                            ))
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
                    AppTab::Help => self.show_help_tab(ui),
                });
            });

        self.show_card_designer_window(ctx);

        let mut show_logs = self.show_logs_window;
        let mut file_saved_msg: Option<String> = None;
        if show_logs {
            egui::Window::new(self.tr("app.logs"))
                .open(&mut show_logs)
                .default_size([540.0, 300.0])
                .min_size([360.0, 180.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        if m3_button_tonal(ui, &self.tr("logs.copy")) {
                            ctx.copy_text(self.logs.join("\n"));
                        }
                        if m3_button_outlined(ui, &self.tr("logs.save")) {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("aircard-diagnostics.log")
                                .add_filter("Log", &["log", "txt"])
                                .save_file()
                            {
                                let content = self.logs.join("\r\n");
                                let _ = std::fs::write(&path, content);
                                file_saved_msg =
                                    Some(format!("Saved log file to {}", path.display()));
                            }
                        }
                        if m3_button_outlined(ui, &self.tr("logs.clear")) {
                            self.logs.clear();
                        }
                        ui.label(
                            egui::RichText::new(
                                self.tr("logs.entries")
                                    .replace("{count}", &self.logs.len().to_string()),
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
                                            egui::RichText::new(self.tr("logs.empty"))
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
    // #--- V9 LAYER CARD DESIGNER UI START ---
    fn show_card_designer_window(&mut self, ctx: &egui::Context) {
        if !self.designer_open {
            return;
        }
        if self.card_designer.dirty || self.designer_preview_texture.is_none() {
            self.refresh_designer_preview(ctx);
        }

        let mut open = self.designer_open;
        let mut refresh_preview = false;
        let mut apply_design = false;
        let mut export_png = false;
        let mut save_draft = false;
        let mut delete_layer = false;
        let mut duplicate_layer = false;
        let mut move_layer: isize = 0;
        let mut bring_front = false;
        let mut send_back = false;

        let d_title = self.tr("designer.title");
        let d_subtitle = self.tr("designer.subtitle");
        let d_background = self.tr("designer.background");
        let d_choose_background = self.tr("designer.chooseBackground");
        let d_reset = self.tr("designer.reset");
        let d_zoom = self.tr("designer.zoom");
        let d_horizontal = self.tr("designer.horizontal");
        let d_vertical = self.tr("designer.vertical");
        let d_reference = self.tr("designer.reference");
        let d_choose_reference = self.tr("designer.chooseReference");
        let d_clear_reference = self.tr("designer.clearReference");
        let d_show_reference = self.tr("designer.showReference");
        let d_reference_opacity = self.tr("designer.referenceOpacity");
        let d_layers = self.tr("designer.layers");
        let d_add_image = self.tr("designer.addImage");
        let d_add_text = self.tr("designer.addText");
        let d_add_rect = self.tr("designer.addRectangle");
        let d_add_round = self.tr("designer.addRoundedRectangle");
        let d_add_circle = self.tr("designer.addCircle");
        let d_properties = self.tr("designer.properties");
        let d_name = self.tr("designer.name");
        let d_visible = self.tr("designer.visible");
        let d_locked = self.tr("designer.locked");
        let d_size = self.tr("designer.size");
        let d_opacity = self.tr("designer.opacity");
        let d_rotation = self.tr("designer.rotation");
        let d_x = self.tr("designer.x");
        let d_y = self.tr("designer.y");
        let d_remove_bg = self.tr("designer.removeBg");
        let d_tolerance = self.tr("designer.tolerance");
        let d_outline = self.tr("designer.outline");
        let d_outline_width = self.tr("designer.outlineWidth");
        let d_mask = self.tr("designer.mask");
        let d_mask_rect = self.tr("designer.maskRectangle");
        let d_mask_round = self.tr("designer.maskRounded");
        let d_mask_circle = self.tr("designer.maskCircle");
        let d_corner_radius = self.tr("designer.cornerRadius");
        let d_text = self.tr("designer.text");
        let d_font_size = self.tr("designer.fontSize");
        let d_bold = self.tr("designer.bold");
        let d_color = self.tr("designer.color");
        let d_width = self.tr("designer.width");
        let d_height = self.tr("designer.height");
        let d_fill = self.tr("designer.fill");
        let d_stroke = self.tr("designer.stroke");
        let d_stroke_width = self.tr("designer.strokeWidth");
        let d_move_down = self.tr("designer.moveDown");
        let d_move_up = self.tr("designer.moveUp");
        let d_front = self.tr("designer.bringFront");
        let d_back = self.tr("designer.sendBack");
        let d_duplicate = self.tr("designer.duplicate");
        let d_delete = self.tr("designer.delete");
        let d_use_design = self.tr("designer.useDesign");
        let d_export = self.tr("designer.export");
        let d_save_draft = self.tr("designer.saveDraft");
        let d_preview = self.tr("designer.preview");
        let d_no_background = self.tr("designer.noBackground");
        let d_canvas_hint = self.tr("designer.canvasHint");
        let d_background_selected = self.tr("designer.backgroundSelected");

        egui::Window::new(d_title.clone())
            .open(&mut open)
            .default_size([1240.0, 780.0])
            .min_size([980.0, 650.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(d_subtitle.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(6.0);

                // Object creation toolbar.
                ui.horizontal_wrapped(|ui| {
                    if m3_button_filled(ui, &d_choose_background) {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                            .pick_file()
                        {
                            match self.card_designer.load_background(&path) {
                                Ok(()) => {
                                    self.source_path = Some(path.clone());
                                    self.skin = None;
                                    self.skin_texture = None;
                                    self.add_log(format!(
                                        "Designer background: {}",
                                        path.display()
                                    ));
                                    refresh_preview = true;
                                }
                                Err(err) => {
                                    self.add_log(format!("Designer background error: {err:#}"))
                                }
                            }
                        }
                    }
                    if m3_button_tonal(ui, &d_add_image) {
                        if let Some(paths) = rfd::FileDialog::new()
                            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                            .pick_files()
                        {
                            for path in paths {
                                match self.card_designer.add_image(&path) {
                                    Ok(()) => refresh_preview = true,
                                    Err(err) => self.add_log(format!("Image layer error: {err:#}")),
                                }
                            }
                        }
                    }
                    if m3_button_tonal(ui, &d_add_text) {
                        self.card_designer.add_text();
                        refresh_preview = true;
                    }
                    if m3_button_outlined(ui, &d_add_rect) {
                        self.card_designer.add_shape(ShapeKind::Rectangle);
                        refresh_preview = true;
                    }
                    if m3_button_outlined(ui, &d_add_round) {
                        self.card_designer.add_shape(ShapeKind::RoundedRectangle);
                        refresh_preview = true;
                    }
                    if m3_button_outlined(ui, &d_add_circle) {
                        self.card_designer.add_shape(ShapeKind::Circle);
                        refresh_preview = true;
                    }
                });
                ui.add_space(8.0);

                ui.columns(3, |cols| {
                    // ---------------- Layers ----------------
                    let left = &mut cols[0];
                    left.label(egui::RichText::new(d_layers.clone()).strong().size(13.0));
                    left.add_space(4.0);

                    let bg_selected = self.card_designer.selected_layer.is_none();
                    if left
                        .selectable_label(bg_selected, format!("▣ {}", d_background))
                        .clicked()
                    {
                        self.card_designer.selected_layer = None;
                    }

                    let mut select_layer: Option<usize> = None;
                    let mut toggle_visibility: Option<usize> = None;
                    let mut toggle_lock: Option<usize> = None;
                    egui::ScrollArea::vertical()
                        .max_height(330.0)
                        .show(left, |ui| {
                            for idx in (0..self.card_designer.layers.len()).rev() {
                                let layer = &self.card_designer.layers[idx];
                                let selected = self.card_designer.selected_layer == Some(idx);
                                let icon = match &layer.kind {
                                    DesignerLayerKind::Image(_) => "▧",
                                    DesignerLayerKind::Text(_) => "T",
                                    DesignerLayerKind::Shape(shape) => match shape.kind {
                                        ShapeKind::Circle => "○",
                                        ShapeKind::RoundedRectangle => "▢",
                                        ShapeKind::Rectangle => "□",
                                    },
                                };
                                let visible = layer.visible;
                                let locked = layer.locked;
                                let name = layer.name.clone();
                                ui.horizontal(|ui| {
                                    if ui.small_button(if visible { "👁" } else { "—" }).clicked()
                                    {
                                        toggle_visibility = Some(idx);
                                    }
                                    if ui
                                        .selectable_label(selected, format!("{}  {}", icon, name))
                                        .clicked()
                                    {
                                        select_layer = Some(idx);
                                    }
                                    if ui.small_button(if locked { "🔒" } else { "🔓" }).clicked()
                                    {
                                        toggle_lock = Some(idx);
                                    }
                                });
                            }
                        });
                    if let Some(idx) = select_layer {
                        self.card_designer.selected_layer = Some(idx);
                    }
                    if let Some(idx) = toggle_visibility {
                        if let Some(layer) = self.card_designer.layers.get_mut(idx) {
                            layer.visible = !layer.visible;
                            refresh_preview = true;
                        }
                    }
                    if let Some(idx) = toggle_lock {
                        if let Some(layer) = self.card_designer.layers.get_mut(idx) {
                            layer.locked = !layer.locked;
                        }
                    }

                    left.add_space(6.0);
                    left.horizontal_wrapped(|ui| {
                        if m3_button_outlined(ui, &d_move_up) {
                            move_layer = 1;
                        }
                        if m3_button_outlined(ui, &d_move_down) {
                            move_layer = -1;
                        }
                        if m3_button_outlined(ui, &d_front) {
                            bring_front = true;
                        }
                        if m3_button_outlined(ui, &d_back) {
                            send_back = true;
                        }
                    });
                    left.horizontal_wrapped(|ui| {
                        if m3_button_outlined(ui, &d_duplicate) {
                            duplicate_layer = true;
                        }
                        if m3_button_outlined(ui, &d_delete) {
                            delete_layer = true;
                        }
                    });

                    left.separator();
                    left.label(egui::RichText::new(d_reference.clone()).strong());
                    left.horizontal_wrapped(|ui| {
                        if m3_button_tonal(ui, &d_choose_reference) {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
                                .pick_file()
                            {
                                match self.card_designer.load_reference(&path) {
                                    Ok(()) => refresh_preview = true,
                                    Err(err) => {
                                        self.add_log(format!("Reference image error: {err:#}"))
                                    }
                                }
                            }
                        }
                        if self.card_designer.reference.is_some()
                            && m3_button_outlined(ui, &d_clear_reference)
                        {
                            self.card_designer.clear_reference();
                            refresh_preview = true;
                        }
                    });
                    if self.card_designer.reference.is_some() {
                        if left
                            .checkbox(
                                &mut self.card_designer.show_reference,
                                d_show_reference.clone(),
                            )
                            .changed()
                        {
                            refresh_preview = true;
                        }
                        if left
                            .add(
                                egui::Slider::new(
                                    &mut self.card_designer.reference_opacity,
                                    0.0..=1.0,
                                )
                                .text(d_reference_opacity.clone()),
                            )
                            .changed()
                        {
                            refresh_preview = true;
                        }
                    }

                    // ---------------- Canvas ----------------
                    let center = &mut cols[1];
                    center.label(egui::RichText::new(d_preview.clone()).strong().size(13.0));
                    center.add_space(5.0);
                    let width = (center.available_width() - 6.0).max(300.0);
                    let height = width * (969.0 / 1536.0);
                    let (rect, response) = center.allocate_exact_size(
                        egui::vec2(width, height),
                        egui::Sense::click_and_drag(),
                    );
                    let painter = center.painter();
                    if let Some(tex) = self.designer_preview_texture.as_ref() {
                        painter.image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    } else {
                        painter.rect_filled(rect, 8.0, md3::SURFACE_CONTAINER_HIGH);
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            d_no_background.clone(),
                            egui::FontId::proportional(14.0),
                            md3::ON_SURFACE_VARIANT,
                        );
                    }
                    painter.rect_stroke(
                        rect,
                        8.0,
                        egui::Stroke::new(1.0, md3::OUTLINE_VARIANT),
                        egui::StrokeKind::Inside,
                    );

                    // Select the topmost unlocked layer under the pointer. Empty canvas selects background.
                    if response.clicked() || response.drag_started() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let nx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                            let ny = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                            self.card_designer.selected_layer =
                                self.card_designer.hit_test_layer(nx, ny);
                        }
                    }
                    if response.dragged() {
                        let delta = response.drag_delta();
                        let dx = delta.x / rect.width();
                        let dy = delta.y / rect.height();
                        if self.card_designer.selected_layer.is_some() {
                            self.card_designer.drag_selected(dx, dy);
                        } else {
                            self.card_designer.drag_background(dx, dy);
                        }
                        refresh_preview = true;
                    }
                    if response.hovered() {
                        let scroll_y = ctx.input(|i| i.smooth_scroll_delta.y);
                        if scroll_y.abs() > 0.1 {
                            let factor = if scroll_y > 0.0 { 1.04 } else { 0.96 };
                            self.card_designer.scale_selected(factor);
                            refresh_preview = true;
                        }
                    }

                    // Selection chrome is UI-only and is never exported.
                    if let Some(index) = self.card_designer.selected_layer {
                        if let Some((x0, y0, x1, y1)) = self.card_designer.layer_bounds_norm(index)
                        {
                            let selection = egui::Rect::from_min_max(
                                egui::pos2(
                                    rect.left() + x0 * rect.width(),
                                    rect.top() + y0 * rect.height(),
                                ),
                                egui::pos2(
                                    rect.left() + x1 * rect.width(),
                                    rect.top() + y1 * rect.height(),
                                ),
                            );
                            painter.rect_stroke(
                                selection,
                                2.0,
                                egui::Stroke::new(2.0, md3::PRIMARY),
                                egui::StrokeKind::Inside,
                            );
                        }
                    } else {
                        painter.rect_stroke(
                            rect.shrink(2.0),
                            8.0,
                            egui::Stroke::new(2.0, md3::PRIMARY),
                            egui::StrokeKind::Inside,
                        );
                    }
                    center.add_space(6.0);
                    center.label(
                        egui::RichText::new(d_canvas_hint.clone())
                            .size(10.5)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    center.label(
                        egui::RichText::new("@3x 1536×969  |  @2x 1024×646  |  PDF")
                            .size(10.5)
                            .color(md3::SUCCESS),
                    );

                    // ---------------- Properties ----------------
                    let right = &mut cols[2];
                    right.label(
                        egui::RichText::new(d_properties.clone())
                            .strong()
                            .size(13.0),
                    );
                    right.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .max_height(500.0)
                        .show(right, |ui| {
                            if self.card_designer.selected_layer.is_none() {
                                ui.label(
                                    egui::RichText::new(d_background_selected.clone()).strong(),
                                );
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.card_designer.crop_zoom,
                                            1.0..=6.0,
                                        )
                                        .text(d_zoom.clone()),
                                    )
                                    .changed()
                                {
                                    refresh_preview = true;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.card_designer.crop_x,
                                            -1.0..=1.0,
                                        )
                                        .text(d_horizontal.clone()),
                                    )
                                    .changed()
                                {
                                    refresh_preview = true;
                                }
                                if ui
                                    .add(
                                        egui::Slider::new(
                                            &mut self.card_designer.crop_y,
                                            -1.0..=1.0,
                                        )
                                        .text(d_vertical.clone()),
                                    )
                                    .changed()
                                {
                                    refresh_preview = true;
                                }
                                if m3_button_outlined(ui, &d_reset) {
                                    self.card_designer.reset_crop();
                                    refresh_preview = true;
                                }
                                return;
                            }

                            let Some(layer) = self.card_designer.selected_layer_mut() else {
                                return;
                            };
                            ui.label(&d_name);
                            if ui.text_edit_singleline(&mut layer.name).changed() {
                                refresh_preview = true;
                            }
                            ui.horizontal(|ui| {
                                if ui.checkbox(&mut layer.visible, d_visible.clone()).changed() {
                                    refresh_preview = true;
                                }
                                ui.checkbox(&mut layer.locked, d_locked.clone());
                            });
                            if ui
                                .add(
                                    egui::Slider::new(&mut layer.opacity, 0.0..=1.0)
                                        .text(d_opacity.clone()),
                                )
                                .changed()
                            {
                                refresh_preview = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut layer.rotation_deg, -180.0..=180.0)
                                        .text(d_rotation.clone()),
                                )
                                .changed()
                            {
                                refresh_preview = true;
                            }
                            if ui
                                .add(egui::Slider::new(&mut layer.x, 0.0..=1.0).text(d_x.clone()))
                                .changed()
                            {
                                refresh_preview = true;
                            }
                            if ui
                                .add(egui::Slider::new(&mut layer.y, 0.0..=1.0).text(d_y.clone()))
                                .changed()
                            {
                                refresh_preview = true;
                            }
                            ui.separator();

                            match &mut layer.kind {
                                DesignerLayerKind::Image(image) => {
                                    if ui
                                        .add(
                                            egui::Slider::new(&mut image.scale, 0.05..=4.0)
                                                .text(d_size.clone()),
                                        )
                                        .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    let mask_label = match image.mask {
                                        ImageMask::Rectangle => d_mask_rect.clone(),
                                        ImageMask::RoundedRectangle => d_mask_round.clone(),
                                        ImageMask::Circle => d_mask_circle.clone(),
                                    };
                                    egui::ComboBox::from_label(d_mask.clone())
                                        .selected_text(mask_label)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut image.mask,
                                                ImageMask::Rectangle,
                                                d_mask_rect.clone(),
                                            );
                                            ui.selectable_value(
                                                &mut image.mask,
                                                ImageMask::RoundedRectangle,
                                                d_mask_round.clone(),
                                            );
                                            ui.selectable_value(
                                                &mut image.mask,
                                                ImageMask::Circle,
                                                d_mask_circle.clone(),
                                            );
                                        });
                                    if image.mask == ImageMask::RoundedRectangle
                                        && ui
                                            .add(
                                                egui::Slider::new(
                                                    &mut image.corner_radius,
                                                    0.0..=0.5,
                                                )
                                                .text(d_corner_radius.clone()),
                                            )
                                            .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    if ui
                                        .checkbox(&mut image.remove_bg, d_remove_bg.clone())
                                        .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    if image.remove_bg
                                        && ui
                                            .add(
                                                egui::Slider::new(
                                                    &mut image.tolerance,
                                                    1.0..=120.0,
                                                )
                                                .text(d_tolerance.clone()),
                                            )
                                            .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    if ui.checkbox(&mut image.outline, d_outline.clone()).changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    if image.outline
                                        && ui
                                            .add(
                                                egui::Slider::new(&mut image.outline_width, 0..=40)
                                                    .text(d_outline_width.clone()),
                                            )
                                            .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                }
                                DesignerLayerKind::Text(text) => {
                                    ui.label(d_text.clone());
                                    if ui
                                        .add(
                                            egui::TextEdit::multiline(&mut text.text)
                                                .desired_rows(3),
                                        )
                                        .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    if ui
                                        .add(
                                            egui::Slider::new(&mut text.font_size, 8.0..=360.0)
                                                .text(d_font_size.clone()),
                                        )
                                        .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    if ui.checkbox(&mut text.bold, d_bold.clone()).changed() {
                                        refresh_preview = true;
                                    }
                                    let mut color = egui::Color32::from_rgba_unmultiplied(
                                        text.color[0],
                                        text.color[1],
                                        text.color[2],
                                        text.color[3],
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label(d_color.clone());
                                        if ui.color_edit_button_srgba(&mut color).changed() {
                                            text.color =
                                                [color.r(), color.g(), color.b(), color.a()];
                                            refresh_preview = true;
                                        }
                                    });
                                }
                                DesignerLayerKind::Shape(shape) => {
                                    if shape.kind == ShapeKind::Circle {
                                        if ui
                                            .add(
                                                egui::Slider::new(&mut shape.width, 0.02..=0.95)
                                                    .text(d_size.clone()),
                                            )
                                            .changed()
                                        {
                                            shape.height =
                                                (shape.width * 1536.0 / 969.0).clamp(0.02, 1.5);
                                            refresh_preview = true;
                                        }
                                    } else {
                                        if ui
                                            .add(
                                                egui::Slider::new(&mut shape.width, 0.02..=1.5)
                                                    .text(d_width.clone()),
                                            )
                                            .changed()
                                        {
                                            refresh_preview = true;
                                        }
                                        if ui
                                            .add(
                                                egui::Slider::new(&mut shape.height, 0.02..=1.5)
                                                    .text(d_height.clone()),
                                            )
                                            .changed()
                                        {
                                            refresh_preview = true;
                                        }
                                    }
                                    if shape.kind == ShapeKind::RoundedRectangle
                                        && ui
                                            .add(
                                                egui::Slider::new(
                                                    &mut shape.corner_radius,
                                                    0.0..=0.5,
                                                )
                                                .text(d_corner_radius.clone()),
                                            )
                                            .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                    let mut fill = egui::Color32::from_rgba_unmultiplied(
                                        shape.fill[0],
                                        shape.fill[1],
                                        shape.fill[2],
                                        shape.fill[3],
                                    );
                                    let mut stroke = egui::Color32::from_rgba_unmultiplied(
                                        shape.stroke[0],
                                        shape.stroke[1],
                                        shape.stroke[2],
                                        shape.stroke[3],
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label(d_fill.clone());
                                        if ui.color_edit_button_srgba(&mut fill).changed() {
                                            shape.fill = [fill.r(), fill.g(), fill.b(), fill.a()];
                                            refresh_preview = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(d_stroke.clone());
                                        if ui.color_edit_button_srgba(&mut stroke).changed() {
                                            shape.stroke =
                                                [stroke.r(), stroke.g(), stroke.b(), stroke.a()];
                                            refresh_preview = true;
                                        }
                                    });
                                    if ui
                                        .add(
                                            egui::Slider::new(&mut shape.stroke_width, 0..=80)
                                                .text(d_stroke_width.clone()),
                                        )
                                        .changed()
                                    {
                                        refresh_preview = true;
                                    }
                                }
                            }
                        });
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    if m3_button_filled(ui, &d_use_design) {
                        apply_design = true;
                    }
                    if m3_button_tonal(ui, &d_export) {
                        export_png = true;
                    }
                    if m3_button_outlined(ui, &d_save_draft) {
                        save_draft = true;
                    }
                });
            });

        self.designer_open = open;
        if delete_layer {
            self.card_designer.remove_selected_layer();
            refresh_preview = true;
        }
        if duplicate_layer {
            self.card_designer.duplicate_selected_layer();
            refresh_preview = true;
        }
        if move_layer != 0 {
            self.card_designer.move_selected_layer(move_layer);
            refresh_preview = true;
        }
        if bring_front {
            self.card_designer.bring_selected_to_front();
            refresh_preview = true;
        }
        if send_back {
            self.card_designer.send_selected_to_back();
            refresh_preview = true;
        }
        if refresh_preview {
            self.card_designer.dirty = true;
            self.refresh_designer_preview(ctx);
            ctx.request_repaint();
        }
        if save_draft {
            self.save_settings_snapshot();
            self.status_msg = self.tr("designer.saved");
        }
        if export_png {
            self.export_designer_png();
        }
        if apply_design {
            self.apply_designer_to_skin(ctx);
        }
    }
    // #--- V9 LAYER CARD DESIGNER UI END ---

    fn show_wallet_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let w_scanning = self.tr("wallet.scanning");
        let w_scan_hint = self.tr("wallet.scanHint");
        let w_stop = self.tr("wallet.stop");
        let w_scan = self.tr("wallet.scan");
        let w_title = self.tr("wallet.title");
        let w_subtitle = self.tr("wallet.subtitle");
        let w_hash = self.tr("wallet.hash");
        let w_hash_hint = self.tr("wallet.hashHint");
        let w_saved = self.tr("wallet.saved");
        let w_select = self.tr("wallet.select");
        let w_static_title = self.tr("wallet.staticTitle");
        let w_static_help = self.tr("wallet.staticHelp");
        let w_choose_image = self.tr("wallet.chooseImage");
        let w_export_png = self.tr("wallet.exportPng");
        let w_designer = self.tr("wallet.designer");
        let w_write = self.tr("wallet.write");
        let w_apply_static = self.tr("wallet.applyStatic");
        let w_preview = self.tr("wallet.preview");
        let w_canvas = self.tr("wallet.canvas");
        let w_ratio = self.tr("wallet.ratio");
        let w_prepared = self.tr("wallet.prepared");
        let w_no_image = self.tr("wallet.noImage");
        let w_reopen = self.tr("wallet.reopenWallet");
        let w_restore_pdf = self.tr("wallet.restorePdf");
        let w_restore_pdf_help = self.tr("wallet.restorePdfHelp");
        let w_need_device_card = self.tr("wallet.needDeviceCard");

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
                                egui::RichText::new(w_scanning.clone())
                                    .strong()
                                    .size(13.0)
                                    .color(md3::ON_TERTIARY_CONTAINER),
                            );
                            ui.label(
                                egui::RichText::new(w_scan_hint.clone())
                                    .size(11.5)
                                    .color(md3::ON_TERTIARY_CONTAINER),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let btn = egui::Button::new(
                                egui::RichText::new(w_stop.clone())
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
                    egui::RichText::new(w_title.clone())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(w_subtitle.clone())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(16.0);

                // Target Card Hash
                ui.label(
                    egui::RichText::new(w_hash.clone())
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
                            .hint_text(w_hash_hint.clone())
                            .desired_width(text_w),
                    );

                    let scan_label = if self.scanning_syslog {
                        w_stop.as_str()
                    } else {
                        w_scan.as_str()
                    };
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
                        egui::RichText::new(scan_label).size(12.0).color(scan_fg),
                    )
                    .fill(scan_bg)
                    .corner_radius(20)
                    .stroke(egui::Stroke::NONE);
                    if ui.add(scan_btn).clicked() {
                        self.toggle_syslog_scan();
                    }
                });

                if !self.saved_cards.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(w_saved.clone())
                            .strong()
                            .size(11.5)
                            .color(md3::ON_SURFACE),
                    );
                    ui.add_space(2.0);
                    let combo_w = (ui.available_width() - 4.0).max(150.0);
                    let sel_label = self
                        .saved_cards
                        .iter()
                        .find(|c| c.hash == self.card_hash)
                        .map(|c| format!("{} ({})", c.name, &c.hash[..8.min(c.hash.len())]))
                        .unwrap_or_else(|| w_select.clone());

                    egui::ComboBox::from_id_salt("saved_cards_box")
                        .width(combo_w)
                        .selected_text(
                            egui::RichText::new(sel_label)
                                .strong()
                                .color(md3::ON_SURFACE),
                        )
                        .show_ui(ui, |ui| {
                            // The ComboBox popup can inherit a light Windows popup background.
                            // Use dark text inside the popup so saved-card names stay readable.
                            let popup_text = egui::Color32::from_rgb(24, 24, 28);
                            for card in &self.saved_cards {
                                let is_selected = self.card_hash == card.hash;
                                let label = format!(
                                    "{} ({}...)",
                                    card.name,
                                    &card.hash[..8.min(card.hash.len())]
                                );
                                let text = egui::RichText::new(label).color(popup_text).strong();
                                if ui.selectable_label(is_selected, text).clicked() {
                                    self.card_hash = card.hash.clone();
                                }
                            }
                        });
                }

                ui.add_space(16.0);

                // Card Skin
                ui.label(
                    egui::RichText::new(w_static_title.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.label(
                    egui::RichText::new(w_static_help.clone())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    if m3_button_filled(ui, w_choose_image.as_str()) {
                        self.select_skin(ctx);
                    }

                    if self.skin.is_some() {
                        if m3_button_tonal(ui, w_export_png.as_str()) {
                            self.save_prepared_png();
                        }
                    }
                });

                if self.card_designer.has_background() {
                    ui.add_space(4.0);
                    if m3_button_outlined(ui, &w_designer) {
                        self.designer_open = true;
                        self.refresh_designer_preview(ctx);
                    }
                }

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
                            "{} - @3x 1536x969 {:.0} KB | @2x 1024x646 {:.0} KB",
                            fname,
                            skin.png.len() as f32 / 1024.0,
                            skin.png_2x.len() as f32 / 1024.0
                        ))
                        .size(11.0)
                        .color(md3::PRIMARY),
                    );
                }

                ui.add_space(16.0);

                // Apply
                ui.label(
                    egui::RichText::new(w_write.clone())
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
                    egui::RichText::new(w_apply_static.clone())
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
                        r.push("連接 iPhone");
                    } else if !self.selected_transport_available() {
                        r.push("選擇可用連線");
                    }
                    if self.card_hash.trim().is_empty() {
                        r.push("輸入卡片 Hash");
                    }
                    if self.skin.is_none() {
                        r.push("選擇圖片");
                    }
                    if !r.is_empty() {
                        resp.on_disabled_hover_text(format!("需要：{}", r.join("、")));
                    }
                }

                // #--- ORIGINAL WALLET PDF RESTORE BUTTON START ---
                ui.add_space(8.0);
                let can_restore_pdf = !self.is_busy
                    && self.selected_transport_available()
                    && !self.card_hash.trim().is_empty();
                let restore_btn = egui::Button::new(
                    egui::RichText::new(&w_restore_pdf)
                        .strong()
                        .size(12.5)
                        .color(if can_restore_pdf {
                            md3::ON_TERTIARY_CONTAINER
                        } else {
                            md3::ON_SURFACE_VARIANT
                        }),
                )
                .fill(if can_restore_pdf {
                    md3::TERTIARY_CONTAINER
                } else {
                    md3::SURFACE_CONTAINER_HIGH
                })
                .corner_radius(18)
                .stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(ui.available_width(), 36.0));
                if ui.add_enabled(can_restore_pdf, restore_btn).clicked() {
                    self.restore_original_wallet_pdf();
                }
                ui.label(
                    egui::RichText::new(&w_restore_pdf_help)
                        .size(10.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                // #--- ORIGINAL WALLET PDF RESTORE BUTTON END ---

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
                        egui::RichText::new(&self.progress_msg)
                            .size(11.0)
                            .color(md3::PRIMARY),
                    );
                }
            });

            // Right: preview
            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(
                    egui::RichText::new(w_preview.clone())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(w_canvas.clone())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(12.0);

                let pass_w = (ui.available_width() - 8.0).clamp(250.0, 400.0);
                let pass_h = pass_w * (969.0 / 1536.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                    let painter = ui.painter();
                    if let Some(tex) = self.skin_texture.as_ref() {
                        painter.image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
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
                            w_no_image.clone(),
                            egui::FontId::proportional(14.0),
                            md3::ON_SURFACE_VARIANT,
                        );
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("1536x969")
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new("|")
                            .size(11.0)
                            .color(md3::OUTLINE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new(w_ratio.clone())
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new("|")
                            .size(11.0)
                            .color(md3::OUTLINE_VARIANT),
                    );
                    if self.skin.is_some() {
                        ui.label(
                            egui::RichText::new(w_prepared.clone())
                                .size(11.0)
                                .color(md3::SUCCESS),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(w_no_image.clone())
                                .size(11.0)
                                .color(md3::ON_SURFACE_VARIANT),
                        );
                    }
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(w_reopen.clone())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
            });
        });
    }

    fn show_passcode_tab(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let p_title = self.tr("passcode.title");
        let p_subtitle = self.tr("passcode.subtitle");
        let p_package = self.tr("passcode.package");
        let p_package_help = self.tr("passcode.packageHelp");
        let p_choose = self.tr("passcode.choose");
        let p_target = self.tr("passcode.target");
        let p_target_help = self.tr("passcode.targetHelp");
        let p_language = self.tr("passcode.language");
        let p_language_help = self.tr("passcode.languageHelp");
        let p_bold = self.tr("passcode.bold");
        let p_bold_help = self.tr("passcode.boldHelp");
        let p_write = self.tr("passcode.write");
        let p_apply = self.tr("passcode.apply");
        let p_preview = self.tr("passcode.preview");
        let p_preview_help = self.tr("passcode.previewHelp");
        let p_layout = self.tr("passcode.layout");
        let p_ready = self.tr("passcode.ready");
        let p_empty = self.tr("passcode.empty");
        let p_after = self.tr("passcode.afterApply");
        let p_need_device = self.tr("passcode.needDevice");
        let p_need_transport = self.tr("passcode.needTransport");
        let p_need_theme = self.tr("passcode.needTheme");

        ui.columns(2, |cols| {
            // Left: config
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(
                    egui::RichText::new(p_title.clone())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(p_subtitle.clone())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(16.0);

                // Theme file
                ui.label(
                    egui::RichText::new(p_package.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.label(
                    egui::RichText::new(p_package_help.clone())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(4.0);
                if m3_button_filled(ui, p_choose.as_str()) {
                    self.select_theme_file(ctx);
                }

                if let Some(theme) = &self.loaded_theme {
                    let fname = self
                        .theme_path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("theme.passthm");
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{} - {} assets", fname, theme.items.len()))
                            .size(11.0)
                            .color(md3::PRIMARY),
                    );
                }

                ui.add_space(16.0);

                // iOS version
                ui.label(
                    egui::RichText::new(p_target.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.label(
                    egui::RichText::new(p_target_help.clone())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(4.0);
                let combo_w = (ui.available_width() - 4.0).max(150.0);
                let mut ver_changed = false;
                egui::ComboBox::from_id_salt("telephony_combo")
                    .width(combo_w)
                    .selected_text(&self.forced_telephony_ver)
                    .show_ui(ui, |ui| {
                        ver_changed |= ui
                            .selectable_value(
                                &mut self.forced_telephony_ver,
                                "Auto (TelephonyUI-10)".into(),
                                "Auto (TelephonyUI-10)",
                            )
                            .clicked();
                        ver_changed |= ui
                            .selectable_value(
                                &mut self.forced_telephony_ver,
                                "TelephonyUI-10".into(),
                                "TelephonyUI-10 (iOS 18+)",
                            )
                            .clicked();
                        ver_changed |= ui
                            .selectable_value(
                                &mut self.forced_telephony_ver,
                                "TelephonyUI-9".into(),
                                "TelephonyUI-9 (iOS 16-17)",
                            )
                            .clicked();
                        ver_changed |= ui
                            .selectable_value(
                                &mut self.forced_telephony_ver,
                                "TelephonyUI-8".into(),
                                "TelephonyUI-8 (Legacy)",
                            )
                            .clicked();
                    });

                if ver_changed {
                    if let Some(path) = self.theme_path.clone() {
                        self.load_theme_from_path(ctx, &path);
                    }
                }

                ui.add_space(16.0);

                // Keypad Language
                ui.label(
                    egui::RichText::new(p_language.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.label(
                    egui::RichText::new(p_language_help.clone())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(4.0);
                let mut lang_changed = false;
                egui::ComboBox::from_id_salt("keypad_lang_combo")
                    .width(combo_w)
                    .selected_text(
                        egui::RichText::new(&self.keypad_language).color(md3::ON_SURFACE),
                    )
                    .show_ui(ui, |ui| {
                        lang_changed |= ui
                            .selectable_value(
                                &mut self.keypad_language,
                                "English".into(),
                                "English",
                            )
                            .clicked();
                        lang_changed |= ui
                            .selectable_value(
                                &mut self.keypad_language,
                                "Russian".into(),
                                "Russian",
                            )
                            .clicked();
                        lang_changed |= ui
                            .selectable_value(
                                &mut self.keypad_language,
                                "Ukrainian".into(),
                                "Ukrainian",
                            )
                            .clicked();
                        lang_changed |= ui
                            .selectable_value(
                                &mut self.keypad_language,
                                "Japanese".into(),
                                "Japanese",
                            )
                            .clicked();
                        lang_changed |= ui
                            .selectable_value(
                                &mut self.keypad_language,
                                "All Languages (Universal)".into(),
                                "Universal (all languages)",
                            )
                            .clicked();
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
                    if ui
                        .checkbox(
                            &mut self.passcode_bold,
                            egui::RichText::new(p_bold.clone())
                                .strong()
                                .size(12.0)
                                .color(md3::ON_SURFACE),
                        )
                        .changed()
                    {
                        bold_changed = true;
                    }
                });
                ui.label(
                    egui::RichText::new(p_bold_help.clone())
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
                ui.label(
                    egui::RichText::new(p_write.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);

                let can_flash = !self.is_busy
                    && self.selected_transport_available()
                    && self.loaded_theme.is_some();
                let flash_btn = egui::Button::new(
                    egui::RichText::new(p_apply.clone())
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
                    self.flash_theme();
                }
                if !can_flash {
                    let mut r = Vec::new();
                    if self.selected_udid.is_none() {
                        r.push(p_need_device.as_str());
                    } else if !self.selected_transport_available() {
                        r.push(p_need_transport.as_str());
                    }
                    if self.loaded_theme.is_none() {
                        r.push(p_need_theme.as_str());
                    }
                    if !r.is_empty() {
                        resp.on_disabled_hover_text(format!("需要：{}", r.join("、")));
                    }
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
                        egui::RichText::new(&self.progress_msg)
                            .size(11.0)
                            .color(md3::PRIMARY),
                    );
                }
            });

            // Right: preview
            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(
                    egui::RichText::new(p_preview.clone())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(p_preview_help.clone())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(12.0);

                let pass_w = (ui.available_width() - 8.0).clamp(240.0, 360.0);
                let pass_h = 265.0;

                ui.vertical_centered(|ui| {
                    if self.keypad_textures.is_empty() {
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 16.0, md3::SURFACE_CONTAINER_HIGH);
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            p_empty.clone(),
                            egui::FontId::proportional(14.0),
                            md3::ON_SURFACE_VARIANT,
                        );
                    } else {
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(pass_w, pass_h), egui::Sense::hover());
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
                                egui::Grid::new("keypad_grid").spacing([18.0, 6.0]).show(
                                    ui,
                                    |ui| {
                                        for row in DIALER_LAYOUT {
                                            for &d in *row {
                                                if d.is_empty() {
                                                    ui.allocate_exact_size(
                                                        egui::vec2(44.0, 50.0),
                                                        egui::Sense::hover(),
                                                    );
                                                } else if let Some((_, tex)) = self
                                                    .keypad_textures
                                                    .iter()
                                                    .find(|(k, _)| k == d)
                                                {
                                                    ui.vertical_centered(|ui| {
                                                        egui::Frame::new()
                                                            .fill(md3::SURFACE)
                                                            .corner_radius(12)
                                                            .inner_margin(3)
                                                            .show(ui, |ui| {
                                                                ui.image((
                                                                    tex.id(),
                                                                    egui::vec2(40.0, 40.0),
                                                                ));
                                                            });
                                                        ui.label(
                                                            egui::RichText::new(d)
                                                                .size(9.5)
                                                                .color(md3::ON_SURFACE_VARIANT),
                                                        );
                                                    });
                                                } else {
                                                    ui.vertical_centered(|ui| {
                                                        egui::Frame::new()
                                                            .fill(md3::SURFACE)
                                                            .corner_radius(12)
                                                            .inner_margin(3)
                                                            .show(ui, |ui| {
                                                                let (btn_rect, _) = ui
                                                                    .allocate_exact_size(
                                                                        egui::vec2(40.0, 40.0),
                                                                        egui::Sense::hover(),
                                                                    );
                                                                ui.painter().rect_filled(
                                                                    btn_rect,
                                                                    8.0,
                                                                    md3::SURFACE_CONTAINER,
                                                                );
                                                                ui.painter().text(
                                                                    btn_rect.center(),
                                                                    egui::Align2::CENTER_CENTER,
                                                                    d,
                                                                    egui::FontId::proportional(
                                                                        14.0,
                                                                    ),
                                                                    md3::ON_SURFACE_VARIANT,
                                                                );
                                                            });
                                                        ui.label(
                                                            egui::RichText::new(d)
                                                                .size(9.5)
                                                                .color(md3::ON_SURFACE_VARIANT),
                                                        );
                                                    });
                                                }
                                            }
                                            ui.end_row();
                                        }
                                    },
                                );
                            });
                        });
                    }
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(p_layout.clone())
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new("|")
                            .size(11.0)
                            .color(md3::OUTLINE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new("TelephonyUI")
                            .size(11.0)
                            .color(md3::ON_SURFACE_VARIANT),
                    );
                    ui.label(
                        egui::RichText::new("|")
                            .size(11.0)
                            .color(md3::OUTLINE_VARIANT),
                    );
                    if self.loaded_theme.is_some() {
                        ui.label(
                            egui::RichText::new(p_ready.clone())
                                .size(11.0)
                                .color(md3::SUCCESS),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(p_empty.clone())
                                .size(11.0)
                                .color(md3::ON_SURFACE_VARIANT),
                        );
                    }
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(p_after.clone())
                        .size(11.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
            });
        });
    }

    fn show_help_tab(&mut self, ui: &mut egui::Ui) {
        let h_title = self.tr("help.title");
        let h_subtitle = self.tr("help.subtitle");
        let h_prereq = self.tr("help.prerequisites");
        let h_prereq1 = self.tr("help.prereq1");
        let h_prereq2 = self.tr("help.prereq2");
        let h_prereq3 = self.tr("help.prereq3");
        let h_prereq4 = self.tr("help.prereq4");
        let h_hash = self.tr("help.hash");
        let h_hash1 = self.tr("help.hash1");
        let h_hash2 = self.tr("help.hash2");
        let h_hash3 = self.tr("help.hash3");
        let h_hash4 = self.tr("help.hash4");
        let h_hash5 = self.tr("help.hash5");
        let h_apply = self.tr("help.apply");
        let h_apply_subtitle = self.tr("help.applySubtitle");
        let h_wallet_title = self.tr("help.walletTitle");
        let h_wallet1 = self.tr("help.wallet1");
        let h_wallet2 = self.tr("help.wallet2");
        let h_wallet3 = self.tr("help.wallet3");
        let h_wallet4 = self.tr("help.wallet4");
        let h_theme_title = self.tr("help.themeTitle");
        let h_theme1 = self.tr("help.theme1");
        let h_theme2 = self.tr("help.theme2");
        let h_theme3 = self.tr("help.theme3");
        let h_theme4 = self.tr("help.theme4");

        ui.columns(2, |cols| {
            let left = &mut cols[0];
            m3_card(left, |ui| {
                ui.label(
                    egui::RichText::new(h_title.clone())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(h_subtitle.clone())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(16.0);

                ui.label(
                    egui::RichText::new(h_prereq.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(h_prereq1.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_prereq2.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_prereq3.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_prereq4.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );

                ui.add_space(18.0);

                ui.label(
                    egui::RichText::new(h_hash.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(h_hash1.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_hash2.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_hash3.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_hash4.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_hash5.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
            });

            let right = &mut cols[1];
            m3_card(right, |ui| {
                ui.label(
                    egui::RichText::new(h_apply.clone())
                        .strong()
                        .size(16.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(h_apply_subtitle.clone())
                        .size(12.0)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.add_space(16.0);

                ui.label(
                    egui::RichText::new(h_wallet_title.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(h_wallet1.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_wallet2.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_wallet3.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_wallet4.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );

                ui.add_space(18.0);

                ui.label(
                    egui::RichText::new(h_theme_title.clone())
                        .strong()
                        .size(12.0)
                        .color(md3::ON_SURFACE),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(h_theme1.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_theme2.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_theme3.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
                ui.label(
                    egui::RichText::new(h_theme4.clone())
                        .size(11.5)
                        .color(md3::ON_SURFACE_VARIANT),
                );
            });
        });
    }
}
