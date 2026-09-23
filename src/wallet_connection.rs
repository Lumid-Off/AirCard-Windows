use crate::pairing::{self, PairRecord, PairStore};
use crate::usbmux::{self, UsbDevice, UsbMux, check_reply, dict, lockdown_request, string};
use anyhow::{Context, Result, bail, ensure};
use plist::{Dictionary, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub enum DeviceStream {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
    Native(crate::native_stream::NativeStream),
}
impl Read for DeviceStream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.read(b),
            Self::Tls(s) => s.read(b),
            Self::Native(s) => s.read(b),
        }
    }
}
impl Write for DeviceStream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(s) => s.write(b),
            Self::Tls(s) => s.write(b),
            Self::Native(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(s) => s.flush(),
            Self::Tls(s) => s.flush(),
            Self::Native(s) => s.flush(),
        }
    }
}
fn secure(socket: TcpStream, record: &PairRecord, stage: &str) -> Result<DeviceStream> {
    // Lockdown is a local paired endpoint. TLS 1.2 also supports the RSA/CBC
    // suites used by Apple's service, which rustls deliberately does not offer.
    let connection = record
        .native_tls_connector()?
        .connect("iphone.local", socket)
        .with_context(|| format!("{stage}: Windows TLS 1.2 handshake failed"))?;
    // Do not expose the stream or send any application request until its
    // certificate is pinned to the device certificate from this pairing.
    let peer = connection
        .peer_certificate()?
        .context("iPhone sent no TLS certificate")?;
    record
        .verify_peer_certificate(&peer.to_der()?)
        .with_context(|| format!("{stage}: iPhone certificate verification failed"))?;
    Ok(DeviceStream::Tls(Box::new(connection)))
}
pub struct UsbSession {
    control: DeviceStream,
    mux: UsbMux,
    device: UsbDevice,
    record: PairRecord,
}
pub struct WalletConnection {
    _control: DeviceSession,
    pub stream: DeviceStream,
}
impl UsbSession {
    pub fn udid(&self) -> &str {
        &self.device.udid
    }

    pub fn open<L: FnMut(String)>(
        udid: Option<&str>,
        cancel: &AtomicBool,
        log: &mut L,
    ) -> Result<Self> {
        cancelled(cancel)?;
        Self::open_with(
            UsbMux::default(),
            PairStore::current_user()?,
            udid,
            cancel,
            log,
        )
    }
    fn open_with<L: FnMut(String)>(
        mux: UsbMux,
        store: PairStore,
        udid: Option<&str>,
        cancel: &AtomicBool,
        log: &mut L,
    ) -> Result<Self> {
        cancelled(cancel)?;
        let device = mux.select(udid)?;
        log("USB device selected. Opening paired device connection...".into());
        let mut socket = mux
            .connect(&device, 62078)
            .context("Connect to iPhone lockdown service")?;
        let public = usbmux::get_value(&mut socket, "DevicePublicKey")?
            .into_data()
            .context("DevicePublicKey is not data")?;
        let mut record = match store.load(&device.udid) {
            Ok(Some(record)) => match record.matches_device(&public) {
                Ok(true) => Some(record),
                _ => {
                    log("App pairing certificate needs renewal (older format or different device); creating a new app pairing.".into());
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                log(format!(
                    "Cannot use the saved app pairing: {e:#}. Creating a new app pairing."
                ));
                None
            }
        };
        cancelled(cancel)?;
        if record.is_none() {
            record = Some(pair_new(
                &mut socket,
                &public,
                &store,
                &device,
                cancel,
                log,
            )?);
        }
        let mut record = record.unwrap();
        let mut response = start_session(&mut socket, &record)?;
        if needs_new_pairing(&response) {
            log("iPhone rejected the saved app identity. Renewing app pairing once...".into());
            drop(socket);
            socket = mux.connect(&device, 62078)?;
            // Fetch the key again after reconnect; never sign a stale device key.
            let public = usbmux::get_value(&mut socket, "DevicePublicKey")?
                .into_data()
                .context("Missing device public key")?;
            record = pair_new(&mut socket, &public, &store, &device, cancel, log)?;
            response = start_session(&mut socket, &record)?;
        }
        check_reply(&response, "Start paired device session")?;
        ensure!(
            response
                .get("SessionID")
                .and_then(Value::as_string)
                .is_some(),
            "iPhone returned no session ID"
        );
        cancelled(cancel)?;
        let control = if response.get("EnableSessionSSL").and_then(Value::as_boolean) == Some(true)
        {
            log("Starting lockdown TLS 1.2 handshake (Windows)...".into());
            secure(socket, &record, "Lockdown session")?
        } else {
            DeviceStream::Plain(socket)
        };
        log("Device session authenticated.".into());
        Ok(Self {
            control,
            mux,
            device,
            record,
        })
    }
    pub fn start_service(&mut self, name: &str) -> Result<DeviceStream> {
        let response = lockdown_request(
            &mut self.control,
            dict([
                ("Request", string("StartService")),
                ("Label", string("AirCard")),
                ("Service", string(name)),
            ]),
        )?;
        check_reply(&response, &format!("Start {name}"))?;
        let port = response
            .get("Port")
            .and_then(Value::as_unsigned_integer)
            .context("Service returned no port")?;
        let socket = self.mux.connect(&self.device, port.try_into()?)?;
        if response.get("EnableServiceSSL").and_then(Value::as_boolean) == Some(true) {
            secure(socket, &self.record, name)
        } else {
            Ok(DeviceStream::Plain(socket))
        }
    }
}
impl DeviceStream {
    pub fn set_timeout(&self, timeout: Duration) -> Result<()> {
        let socket = match self {
            Self::Plain(s) => s,
            Self::Tls(s) => s.get_ref(),
            Self::Native(s) => return s.set_timeout(timeout),
        };
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;
        Ok(())
    }
}
impl WalletConnection {
    pub fn open<L: FnMut(String)>(
        udid: Option<&str>,
        mode: crate::device::ConnectionMode,
        cancel: &AtomicBool,
        log: &mut L,
    ) -> Result<Self> {
        let mut session = DeviceSession::open(udid, mode, cancel, log)?;
        let stream = session.start_service("com.apple.syslog_relay")?;
        stream.set_timeout(Duration::from_millis(500))?;
        cancelled(cancel)?;
        log("Wallet log service connected. Open Wallet and select a card.".into());
        Ok(Self {
            _control: session,
            stream,
        })
    }
    #[cfg(test)]
    fn open_with<L: FnMut(String)>(
        mux: UsbMux,
        store: PairStore,
        udid: Option<&str>,
        cancel: &AtomicBool,
        log: &mut L,
    ) -> Result<Self> {
        let mut session = DeviceSession::Usb(UsbSession::open_with(mux, store, udid, cancel, log)?);
        let stream = session.start_service("com.apple.syslog_relay")?;
        stream.set_timeout(Duration::from_millis(500))?;
        cancelled(cancel)?;
        log("Wallet log service connected. Double-click the side button, authenticate, then tap your card.".into());
        Ok(Self {
            _control: session,
            stream,
        })
    }
}

fn cancelled(flag: &AtomicBool) -> Result<()> {
    ensure!(!flag.load(Ordering::Relaxed), "Scan cancelled");
    Ok(())
}
fn start_session(s: &mut TcpStream, record: &PairRecord) -> Result<Dictionary> {
    lockdown_request(
        s,
        dict([
            ("Request", string("StartSession")),
            ("Label", string("AirCard")),
            ("HostID", string(record.text("HostID")?)),
            ("SystemBUID", string(record.text("SystemBUID")?)),
        ]),
    )
}
fn needs_new_pairing(d: &Dictionary) -> bool {
    matches!(
        d.get("Error").and_then(Value::as_string),
        Some("InvalidHostID" | "MissingPairRecord" | "InvalidPairRecord")
    )
}
fn pair_new<L: FnMut(String)>(
    s: &mut TcpStream,
    public: &[u8],
    store: &PairStore,
    device: &UsbDevice,
    cancel: &AtomicBool,
    log: &mut L,
) -> Result<PairRecord> {
    log("Preparing app pairing. Keep iPhone unlocked; tap Trust if it asks.".into());
    let mut record =
        pairing::generate(public, &store.buid()?).context("Generate app pairing certificates")?;
    cancelled(cancel)?;
    let request = dict([
        ("Request", string("Pair")),
        ("Label", string("AirCard")),
        ("ProtocolVersion", string("2")),
        (
            "PairingOptions",
            dict([("ExtendedPairingErrors", Value::Boolean(true))]),
        ),
        ("PairRecord", record.public_record()?),
    ]);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut waiting = false;
    loop {
        cancelled(cancel)?;
        let reply = lockdown_request(s, request.clone()).context("Send app pairing request")?;
        match reply.get("Error").and_then(Value::as_string) {
            None => {
                if let Some(bag) = reply.get("EscrowBag") {
                    record.0.insert("EscrowBag".into(), bag.clone());
                }
                store
                    .save(&device.udid, &record)
                    .context("Save encrypted app pairing record")?;
                log("App pairing saved securely for this Windows account.".into());
                return Ok(record);
            }
            Some("PairingDialogResponsePending") => {
                if !waiting {
                    log("Waiting for Trust confirmation on the unlocked iPhone (up to 60 seconds)...".into());
                    waiting = true;
                }
                ensure!(
                    Instant::now() < deadline,
                    "Trust confirmation timed out. Unlock iPhone, tap Trust, then scan again"
                );
                for _ in 0..5 {
                    cancelled(cancel)?;
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            Some("UserDeniedPairing") => {
                bail!("Trust was declined on iPhone. Unlock it and approve Trust before scanning")
            }
            Some("PasswordProtected") => {
                bail!("iPhone is locked. Unlock with the passcode before scanning")
            }
            Some(e) => bail!("App pairing failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_identity_errors_trigger_one_repair() {
        for e in ["InvalidHostID", "MissingPairRecord", "InvalidPairRecord"] {
            assert!(needs_new_pairing(
                dict([("Error", string(e))]).as_dictionary().unwrap()
            ));
        }
        for e in [
            "PasswordProtected",
            "UserDeniedPairing",
            "ServiceProhibited",
        ] {
            assert!(!needs_new_pairing(
                dict([("Error", string(e))]).as_dictionary().unwrap()
            ));
        }
    }
    #[test]
    fn cancel_before_open_does_not_contact_device() {
        let cancel = AtomicBool::new(true);
        let mut logs = Vec::new();
        let result = WalletConnection::open(
            None,
            crate::device::ConnectionMode::Usb,
            &cancel,
            &mut |s| logs.push(s),
        );
        assert!(result.is_err());
        assert!(logs.is_empty());
    }
}

#[cfg(test)]
#[path = "wallet_connection_tests.rs"]
mod protocol_tests;

pub enum DeviceSession {
    Usb(UsbSession),
    Native(std::sync::Arc<crate::device::ActiveDeviceSession>),
}
impl DeviceSession {
    pub fn open<L: FnMut(String)>(
        udid: Option<&str>,
        mode: crate::device::ConnectionMode,
        cancel: &AtomicBool,
        log: &mut L,
    ) -> Result<Self> {
        use crate::device::{ActiveDeviceSession, ConnectionMode, DeviceTransport};
        cancelled(cancel)?;
        let entries = crate::device::query_usbmux_devices()?;
        if udid.is_none() {
            let distinct: std::collections::HashSet<_> = entries
                .iter()
                .map(|e| e.udid.to_ascii_lowercase())
                .collect();
            ensure!(
                distinct.len() == 1,
                "Select one iPhone before opening a device session"
            );
        }
        let selected = entries.iter().filter(|e| {
            udid.map(|u| usbmux::same_device(u, &e.udid))
                .unwrap_or(true)
        });
        let available: Vec<_> = selected.map(|e| e.transport).collect();
        let route = preferred_route(mode, &available)?;
        if route == DeviceTransport::Usb {
            match UsbSession::open(udid, cancel, log) {
                Ok(s) => return Ok(Self::Usb(s)),
                Err(e)
                    if mode == ConnectionMode::Auto
                        && available.contains(&DeviceTransport::Wifi)
                        && !cancel.load(Ordering::Relaxed) =>
                {
                    log(format!(
                        "USB session failed: {e:#}; trying the available WiFi connection."
                    ))
                }
                Err(e) => return Err(e),
            }
        }
        cancelled(cancel)?;
        let session = ActiveDeviceSession::open(udid, ConnectionMode::Wifi)?;
        log("Connected over WiFi using Apple native pairing.".into());
        Ok(Self::Native(std::sync::Arc::new(session)))
    }
    pub fn udid(&self) -> &str {
        match self {
            Self::Usb(s) => s.udid(),
            Self::Native(s) => &s.udid,
        }
    }
    pub fn transport(&self) -> crate::device::DeviceTransport {
        match self {
            Self::Usb(_) => crate::device::DeviceTransport::Usb,
            Self::Native(s) => s.transport,
        }
    }
    pub fn start_service(&mut self, name: &str) -> Result<DeviceStream> {
        match self {
            Self::Usb(s) => s.start_service(name),
            Self::Native(s) => Ok(DeviceStream::Native(
                crate::native_stream::NativeStream::open(std::sync::Arc::clone(s), name)?,
            )),
        }
    }
}
fn preferred_route(
    mode: crate::device::ConnectionMode,
    available: &[crate::device::DeviceTransport],
) -> Result<crate::device::DeviceTransport> {
    use crate::device::{ConnectionMode as M, DeviceTransport as T};
    if mode != M::Wifi && available.contains(&T::Usb) {
        return Ok(T::Usb);
    }
    if mode != M::Usb && available.contains(&T::Wifi) {
        return Ok(T::Wifi);
    }
    bail!(
        "No device available for {}. Refresh the device list.",
        mode.label()
    )
}
#[cfg(test)]
mod transport_tests {
    use super::*;
    use crate::device::{ConnectionMode as M, DeviceTransport as T};
    #[test]
    fn explicit_modes_do_not_fall_back_and_auto_prefers_usb() {
        assert_eq!(
            preferred_route(M::Auto, &[T::Wifi, T::Usb]).unwrap(),
            T::Usb
        );
        assert_eq!(preferred_route(M::Auto, &[T::Wifi]).unwrap(), T::Wifi);
        assert_eq!(
            preferred_route(M::Wifi, &[T::Wifi, T::Usb]).unwrap(),
            T::Wifi
        );
        assert!(preferred_route(M::Usb, &[T::Wifi]).is_err());
        assert!(preferred_route(M::Wifi, &[T::Usb]).is_err());
        assert!(preferred_route(M::Auto, &[]).is_err());
    }
}
