use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

use crate::apple;
use crate::device::{ConnectionMode, DeviceTransport, UsbmuxDeviceEntry, query_usbmux_devices};

pub const APPLE_RUNTIME: &str = "Apple device support";
pub const APPLE_HELP_URL: &str = "https://support.apple.com/itunes";
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "Passed",
            Self::Warning => "Needs attention",
            Self::Fail => "Failed",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EnvironmentCheck {
    pub title: String,
    pub status: CheckStatus,
    pub detail: String,
    pub action: String,
}

impl EnvironmentCheck {
    pub fn new(title: &str, status: CheckStatus, detail: impl Into<String>, action: &str) -> Self {
        Self {
            title: title.into(),
            status,
            detail: detail.into(),
            action: action.into(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ScanRequest {
    pub udid: Option<String>,
    pub mode: ConnectionMode,
}

pub enum ScanEvent {
    Check(EnvironmentCheck),
    Finished(Result<(), String>),
}

// Apple native calls can block. A separate, bounded process keeps detection
// responsive and stops native activity before another device operation starts.
pub fn scan_in_worker(
    request: ScanRequest,
    mut on_check: impl FnMut(EnvironmentCheck),
) -> Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--environment-worker")
        .creation_flags(0x08000000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start environment checks")?;
    let result = (|| -> Result<()> {
        let mut input = child.stdin.take().context("Diagnostic input unavailable")?;
        serde_json::to_writer(&mut input, &request)?;
        drop(input);
        let output = child
            .stdout
            .take()
            .context("Diagnostic output unavailable")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let check = line.map_err(anyhow::Error::from).and_then(|line| {
                    serde_json::from_str::<EnvironmentCheck>(&line).map_err(Into::into)
                });
                if tx.send(check).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + CHECK_TIMEOUT;
        loop {
            while let Ok(check) = rx.try_recv() {
                on_check(check?);
            }
            if let Some(status) = child.try_wait()? {
                for check in rx {
                    on_check(check?);
                }
                ensure!(
                    status.success(),
                    "Environment worker failed; see the check results above"
                );
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "Environment check timed out after 20 seconds. The diagnostic worker was stopped. Reconnect the unlocked iPhone and check again."
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    child
        .wait()
        .context("Could not stop the diagnostic worker")?;
    result
}

pub fn run_worker() -> i32 {
    let result = (|| -> Result<()> {
        let mut input = Vec::new();
        std::io::stdin()
            .take(16 * 1024 + 1)
            .read_to_end(&mut input)?;
        ensure!(input.len() <= 16 * 1024, "Diagnostic request is too large");
        let request: ScanRequest = serde_json::from_slice(&input)?;
        scan(&request, emit_check)
    })();
    if let Err(error) = result {
        let _ = emit_check(EnvironmentCheck::new(
            "Environment worker",
            CheckStatus::Fail,
            format!("{error:#}"),
            "Run the checks again. Copy the report if the problem continues.",
        ));
        return 1;
    }
    0
}

fn emit_check(check: EnvironmentCheck) -> Result<()> {
    let mut output = std::io::stdout().lock();
    serde_json::to_writer(&mut output, &check)?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn scan(request: &ScanRequest, mut emit: impl FnMut(EnvironmentCheck) -> Result<()>) -> Result<()> {
    let runtime = apple::get_apple_libraries();
    emit(match &runtime {
        Ok(_) => EnvironmentCheck::new(
            APPLE_RUNTIME,
            CheckStatus::Pass,
            format!(
                "Apple DLLs and required exports loaded from {}.",
                apple::locate_support_dir().unwrap_or_default().display()
            ),
            "",
        ),
        Err(error) => EnvironmentCheck::new(
            APPLE_RUNTIME,
            CheckStatus::Fail,
            format!("{error:#}"),
            "Install or repair 64-bit desktop iTunes from Apple, then restart AirCard.",
        ),
    })?;
    emit(service_check())?;

    let devices = query_usbmux_devices();
    emit(match &devices {
        Ok(entries) => EnvironmentCheck::new(
            "Apple USB device channel",
            CheckStatus::Pass,
            format!(
                "usbmuxd responded on 127.0.0.1:27015; {} device connection(s) reported.",
                entries.len()
            ),
            "",
        ),
        Err(error) => EnvironmentCheck::new(
            "Apple USB device channel",
            CheckStatus::Fail,
            format!("{error:#}"),
            "Open Services, start Apple Mobile Device Service, and reconnect the iPhone.",
        ),
    })?;
    // Check through the service itself: a readable directory for the desktop
    // user does not prove that the Apple service account can read Lockdown.
    let buid = read_system_buid();
    emit(match &buid {
        Ok(_) => EnvironmentCheck::new(
            "Lockdown service access (BUID)",
            CheckStatus::Pass,
            "The Apple service can read its system identity and pairing storage.",
            "",
        ),
        Err(error) => EnvironmentCheck::new(
            "Lockdown service access (BUID)",
            CheckStatus::Fail,
            format!("The Apple service could not read its system identity: {error:#}"),
            "Check the service Log On account and its access to %ProgramData%\\Apple\\Lockdown. Repair Apple Mobile Device Support if needed. This can cause 0xE8000003 and AirTraffic timeouts.",
        ),
    })?;

    let selected = devices
        .as_ref()
        .ok()
        .and_then(|entries| selected_entry(entries, request));
    emit(match selected {
        Some(entry) => EnvironmentCheck::new(
            "Selected iPhone / transport",
            CheckStatus::Pass,
            format!(
                "{} over {}. Mode: {}.",
                entry.udid,
                entry.transport.label(),
                request.mode.label()
            ),
            "Keep the iPhone unlocked during device operations.",
        ),
        None => EnvironmentCheck::new(
            "Selected iPhone / transport",
            CheckStatus::Warning,
            format!(
                "No usable device for {}{}.",
                request.mode.label(),
                request
                    .udid
                    .as_ref()
                    .map(|id| format!(" (selected: {id})"))
                    .unwrap_or_default()
            ),
            "Connect USB and open iTunes. Unlock iPhone and tap Trust when prompted. For WiFi, enable WiFi sync on the same network, unplug USB, then refresh.",
        ),
    })?;

    emit(match (runtime.as_ref(), selected, buid.is_ok()) {
        (Ok(libs), Some(entry), true) => match probe_native_session(libs, entry) {
            Ok(()) => EnvironmentCheck::new(
                "Apple pairing / AirTraffic service",
                CheckStatus::Pass,
                "Native pairing and session authentication passed; com.apple.atc opened successfully.",
                "This checks prerequisites. A completed card or theme transfer still requires verification on the iPhone.",
            ),
            Err(error) => EnvironmentCheck::new(
                "Apple pairing / AirTraffic service",
                CheckStatus::Fail,
                format!("{error:#}"),
                "Keep the iPhone unlocked. Open iTunes and complete Trust this Computer, then close iTunes and run these checks again.",
            ),
        },
        _ => EnvironmentCheck::new(
            "Apple pairing / AirTraffic service",
            CheckStatus::Warning,
            "Not checked because Apple support, BUID access, or the selected connection is unavailable.",
            "Resolve the checks above, then run the checks again.",
        ),
    })?;
    Ok(())
}

fn selected_entry<'a>(
    entries: &'a [UsbmuxDeviceEntry],
    request: &ScanRequest,
) -> Option<&'a UsbmuxDeviceEntry> {
    let candidates: Vec<_> = entries
        .iter()
        .filter(|entry| {
            request
                .udid
                .as_ref()
                .is_none_or(|id| entry.udid.eq_ignore_ascii_case(id))
        })
        .collect();
    // Match the UI rule: Apple's sync API chooses by UDID, so WiFi only must
    // not claim readiness while the same device also has a USB connection.
    candidates
        .iter()
        .copied()
        .find(|entry| match request.mode {
            ConnectionMode::Usb => entry.transport == DeviceTransport::Usb,
            ConnectionMode::Wifi => {
                entry.transport == DeviceTransport::Wifi
                    && !entries.iter().any(|other| {
                        other.udid.eq_ignore_ascii_case(&entry.udid)
                            && other.transport == DeviceTransport::Usb
                    })
            }
            ConnectionMode::Auto => entry.transport == DeviceTransport::Usb,
        })
        .or_else(|| {
            (request.mode == ConnectionMode::Auto)
                .then(|| {
                    candidates
                        .into_iter()
                        .find(|entry| entry.transport == DeviceTransport::Wifi)
                })
                .flatten()
        })
}

fn read_system_buid() -> Result<String> {
    let mut stream =
        TcpStream::connect_timeout(&"127.0.0.1:27015".parse()?, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    query_buid(&mut stream)
}

fn query_buid(stream: &mut (impl Read + Write)) -> Result<String> {
    let request = plist::Value::Dictionary(
        [
            (
                "MessageType".to_string(),
                plist::Value::String("ReadBUID".into()),
            ),
            (
                "ClientVersionString".to_string(),
                plist::Value::String("aircard".into()),
            ),
            (
                "ProgName".to_string(),
                plist::Value::String("aircard".into()),
            ),
            (
                "kLibUSBMuxVersion".to_string(),
                plist::Value::Integer(3.into()),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let mut data = Vec::new();
    request.to_writer_xml(&mut data)?;
    for field in [data.len() as u32 + 16, 1, 8, 1] {
        stream.write_all(&field.to_le_bytes())?;
    }
    stream.write_all(&data)?;
    stream.flush()?;
    let mut header = [0u8; 16];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header[..4].try_into()?) as usize;
    ensure!(
        (16..=64 * 1024).contains(&length),
        "Invalid ReadBUID response length"
    );
    ensure!(
        u32::from_le_bytes(header[4..8].try_into()?) == 1
            && u32::from_le_bytes(header[8..12].try_into()?) == 8,
        "Unexpected ReadBUID response type"
    );
    let mut body = vec![0; length - 16];
    stream.read_exact(&mut body)?;
    let response = plist::Value::from_reader(std::io::Cursor::new(body))?;
    response.as_dictionary().and_then(|dict| dict.get("BUID")).and_then(|value| value.as_string())
        .filter(|value| !value.trim().is_empty()).map(str::to_string)
        .context("ReadBUID returned no system identity; check the Apple service account's Lockdown permissions")
}

fn service_check() -> EnvironmentCheck {
    // Query the Windows SCM directly; sc.exe's display text is localized.
    #[repr(C)]
    #[derive(Default)]
    struct ServiceStatus {
        kind: u32,
        state: u32,
        controls: u32,
        exit: u32,
        specific_exit: u32,
        checkpoint: u32,
        wait_hint: u32,
    }
    type Handle = *mut std::ffi::c_void;
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenSCManagerW(machine: *const u16, database: *const u16, access: u32) -> Handle;
        fn OpenServiceW(manager: Handle, name: *const u16, access: u32) -> Handle;
        fn QueryServiceStatus(service: Handle, status: *mut ServiceStatus) -> i32;
        fn CloseServiceHandle(handle: Handle) -> i32;
    }
    let result = (|| -> Result<u32> {
        unsafe {
            let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), 1);
            ensure!(
                !manager.is_null(),
                "Could not open service manager: {}",
                std::io::Error::last_os_error()
            );
            let name: Vec<u16> = "Apple Mobile Device Service"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let service = OpenServiceW(manager, name.as_ptr(), 4);
            let error = std::io::Error::last_os_error();
            CloseServiceHandle(manager);
            if service.is_null() {
                return Err(error.into());
            }
            let mut status = ServiceStatus::default();
            let ok = QueryServiceStatus(service, &mut status);
            let error = std::io::Error::last_os_error();
            CloseServiceHandle(service);
            if ok == 0 {
                return Err(error.into());
            }
            Ok(status.state)
        }
    })();
    match result {
        Ok(4) => EnvironmentCheck::new(
            "Apple Mobile Device Service",
            CheckStatus::Pass,
            "The Windows service is running.",
            "",
        ),
        Ok(state) => EnvironmentCheck::new(
            "Apple Mobile Device Service",
            CheckStatus::Fail,
            format!("The service is installed but is not running (state {state})."),
            "Open Services and start or restart Apple Mobile Device Service.",
        ),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.raw_os_error() == Some(1060)) =>
        {
            EnvironmentCheck::new(
                "Apple Mobile Device Service",
                CheckStatus::Fail,
                "The Windows service is not installed.",
                "Install or repair 64-bit desktop iTunes from Apple.",
            )
        }
        Err(error) => EnvironmentCheck::new(
            "Apple Mobile Device Service",
            CheckStatus::Warning,
            format!("Could not query the service: {error:#}"),
            "Open Services and verify Apple Mobile Device Service manually.",
        ),
    }
}

fn probe_native_session(libs: &apple::AppleLibraries, entry: &UsbmuxDeviceEntry) -> Result<()> {
    let properties = libs.create_cf_plist_from_bytes(&entry.properties_plist)?;
    unsafe {
        let device = (libs.am_device_create_from_properties)(properties.raw);
        ensure!(
            !device.is_null(),
            "AMDeviceCreateFromProperties returned no device"
        );
        let mut connected = false;
        let mut started = false;
        let result = (|| -> Result<()> {
            let status = (libs.am_device_connect)(device);
            ensure!(
                status == 0,
                "AMDeviceConnect failed: 0x{:08X}",
                status as u32
            );
            connected = true;
            ensure!(
                (libs.am_device_is_paired)(device) != 0,
                "This iPhone has no usable Apple native pairing"
            );
            let status = (libs.am_device_validate_pairing)(device);
            ensure!(
                status == 0,
                "AMDeviceValidatePairing failed: 0x{:08X}",
                status as u32
            );
            let status = (libs.am_device_start_session)(device);
            ensure!(
                status == 0,
                "AMDeviceStartSession failed: 0x{:08X}",
                status as u32
            );
            started = true;
            let service_name = libs.create_cf_string("com.apple.atc")?;
            let mut service = std::ptr::null_mut();
            let status = (libs.am_device_secure_start_service)(
                device,
                service_name.raw,
                std::ptr::null(),
                &mut service,
            );
            if !service.is_null() {
                (libs.amd_service_connection_invalidate)(service);
            }
            ensure!(
                status == 0 && !service.is_null(),
                "StartService(com.apple.atc) failed: 0x{:08X}",
                status as u32
            );
            Ok(())
        })();
        if started {
            (libs.am_device_stop_session)(device);
        }
        if connected {
            (libs.am_device_disconnect)(device);
        }
        (libs.cf_release)(device);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeMux {
        read: std::io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }
    impl Read for FakeMux {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.read.read(bytes)
        }
    }
    impl Write for FakeMux {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn response(value: plist::Value) -> FakeMux {
        let mut data = Vec::new();
        value.to_writer_binary(&mut data).unwrap();
        let mut frame = Vec::new();
        for field in [data.len() as u32 + 16, 1, 8, 1] {
            frame.extend_from_slice(&field.to_le_bytes());
        }
        frame.extend(data);
        FakeMux {
            read: std::io::Cursor::new(frame),
            written: Vec::new(),
        }
    }
    #[test]
    fn missing_buid_is_a_failure_even_when_the_service_replies() {
        let mut peer = response(plist::Value::Dictionary(plist::Dictionary::new()));
        assert!(
            query_buid(&mut peer)
                .unwrap_err()
                .to_string()
                .contains("Lockdown permissions")
        );
        let request = plist::Value::from_reader(std::io::Cursor::new(&peer.written[16..])).unwrap();
        assert_eq!(
            request.as_dictionary().unwrap()["MessageType"].as_string(),
            Some("ReadBUID")
        );
        let mut peer = response(plist::Value::Dictionary(
            [("BUID".to_string(), plist::Value::String("identity".into()))]
                .into_iter()
                .collect(),
        ));
        assert_eq!(query_buid(&mut peer).unwrap(), "identity");
    }
    #[test]
    fn wifi_requires_the_selected_device_to_be_detached_from_usb() {
        let entry = |id: &str, transport| UsbmuxDeviceEntry {
            udid: id.into(),
            transport,
            properties_plist: Vec::new(),
        };
        let entries = vec![
            entry("phone", DeviceTransport::Wifi),
            entry("PHONE", DeviceTransport::Usb),
            entry("other", DeviceTransport::Wifi),
        ];
        let mut request = ScanRequest {
            udid: Some("phone".into()),
            mode: ConnectionMode::Wifi,
        };
        assert!(selected_entry(&entries, &request).is_none());
        request.mode = ConnectionMode::Auto;
        assert_eq!(
            selected_entry(&entries, &request).unwrap().transport,
            DeviceTransport::Usb
        );
        request.udid = Some("missing".into());
        assert!(selected_entry(&entries, &request).is_none());
        request.udid = Some("other".into());
        request.mode = ConnectionMode::Wifi;
        assert_eq!(
            selected_entry(&entries, &request).unwrap().transport,
            DeviceTransport::Wifi
        );
    }
}
