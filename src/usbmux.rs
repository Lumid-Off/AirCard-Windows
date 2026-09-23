//! Framed USB transport. Pair-record RPCs are deliberately not used: some Apple
//! services support ListDevices/Connect but reject ReadPairRecord/SavePairRecord.
use anyhow::{Context, Result, bail, ensure};
use plist::{Dictionary, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub fn dict(items: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Dictionary(items.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}
pub fn string(s: impl Into<String>) -> Value {
    Value::String(s.into())
}
pub fn canonical_udid(s: &str) -> Result<String> {
    let compact = s.replace('-', "");
    ensure!(
        (compact.len() == 24 || compact.len() == 40)
            && compact.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid device identifier format"
    );
    Ok(if compact.len() == 24 {
        format!("{}-{}", &compact[..8], &compact[8..]).to_uppercase()
    } else {
        compact.to_lowercase()
    })
}
pub fn same_device(a: &str, b: &str) -> bool {
    match (canonical_udid(a), canonical_udid(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct UsbDevice {
    pub id: u32,
    pub udid: String,
}
#[derive(Clone, Debug)]
pub struct UsbMux {
    address: SocketAddr,
}
impl Default for UsbMux {
    fn default() -> Self {
        Self {
            address: "127.0.0.1:27015".parse().unwrap(),
        }
    }
}
impl UsbMux {
    #[cfg(test)]
    pub fn at(address: SocketAddr) -> Self {
        Self { address }
    }
    fn socket(&self) -> Result<TcpStream> {
        let s = TcpStream::connect_timeout(&self.address, IO_TIMEOUT)
            .context("Cannot reach Apple USB service. Start Apple Mobile Device Service")?;
        s.set_read_timeout(Some(IO_TIMEOUT))?;
        s.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(s)
    }
    pub fn devices(&self) -> Result<Vec<UsbDevice>> {
        let reply = mux_request(
            &mut self.socket()?,
            dict([("MessageType", string("ListDevices"))]),
        )?;
        let list = reply
            .get("DeviceList")
            .and_then(Value::as_array)
            .context("Apple USB service returned no device list")?;
        let mut result = Vec::new();
        for item in list {
            let Some(p) = item
                .as_dictionary()
                .and_then(|d| d.get("Properties"))
                .and_then(Value::as_dictionary)
            else {
                continue;
            };
            if p.get("ConnectionType").and_then(Value::as_string) != Some("USB") {
                continue;
            }
            let id = p
                .get("DeviceID")
                .and_then(Value::as_unsigned_integer)
                .context("Missing USB device ID")?;
            let serial = p
                .get("SerialNumber")
                .and_then(Value::as_string)
                .context("Missing device identifier")?;
            result.push(UsbDevice {
                id: u32::try_from(id)?,
                udid: canonical_udid(serial)?,
            });
        }
        Ok(result)
    }
    pub fn select(&self, udid: Option<&str>) -> Result<UsbDevice> {
        let devices = self.devices()?;
        match udid {
            Some(u) => devices
                .into_iter()
                .find(|d| same_device(&d.udid, u))
                .context(
                    "Selected iPhone is no longer connected by USB. Reconnect and click Refresh",
                ),
            None => {
                ensure!(
                    devices.len() == 1,
                    "Connect exactly one iPhone by USB, or select a device first"
                );
                Ok(devices[0].clone())
            }
        }
    }
    pub fn connect(&self, device: &UsbDevice, port: u16) -> Result<TcpStream> {
        let mut s = self.socket()?;
        let r = mux_request(
            &mut s,
            dict([
                ("MessageType", string("Connect")),
                ("DeviceID", Value::Integer(device.id.into())),
                ("PortNumber", Value::Integer(u64::from(port.to_be()).into())),
            ]),
        )?;
        let status = r
            .get("Number")
            .and_then(Value::as_unsigned_integer)
            .context("USB Connect reply has no status")?;
        ensure!(
            status == 0,
            "Apple USB Connect failed (status {status}, port {port}). The device may have disconnected"
        );
        Ok(s)
    }
}
fn mux_request<S: Read + Write>(s: &mut S, mut request: Value) -> Result<Dictionary> {
    let d = request
        .as_dictionary_mut()
        .context("Request must be a dictionary")?;
    d.insert("ClientVersionString".into(), string("AirCard"));
    d.insert("ProgName".into(), string("AirCard"));
    d.insert("kLibUSBMuxVersion".into(), Value::Integer(3.into()));
    let mut payload = Vec::new();
    request.to_writer_xml(&mut payload)?;
    let length = u32::try_from(payload.len() + 16)?;
    for v in [length, 1, 8, 1] {
        s.write_all(&v.to_le_bytes())?;
    }
    s.write_all(&payload)?;
    s.flush()?;
    let mut header = [0u8; 16];
    s.read_exact(&mut header)
        .context("Reading USB reply header")?;
    let words: Vec<_> = header
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    ensure!(
        words[0] >= 16 && words[0] as usize <= MAX_FRAME,
        "Invalid USB reply size"
    );
    ensure!(
        words[1] == 1 && words[2] == 8 && words[3] == 1,
        "Unexpected USB reply version/type/tag"
    );
    let mut payload = vec![0; words[0] as usize - 16];
    s.read_exact(&mut payload)?;
    Value::from_reader(std::io::Cursor::new(payload))?
        .into_dictionary()
        .context("USB reply is not a dictionary")
}
pub fn lockdown_request<S: Read + Write>(s: &mut S, request: Value) -> Result<Dictionary> {
    let mut b = Vec::new();
    request.to_writer_xml(&mut b)?;
    ensure!(b.len() <= MAX_FRAME, "Lockdown request too large");
    s.write_all(&(b.len() as u32).to_be_bytes())?;
    s.write_all(&b)?;
    s.flush()?;
    let mut header = [0; 4];
    s.read_exact(&mut header)
        .context("Reading iPhone reply header")?;
    let n = u32::from_be_bytes(header) as usize;
    ensure!(n > 0 && n <= MAX_FRAME, "Invalid iPhone reply size {n}");
    let mut b = vec![0; n];
    s.read_exact(&mut b).context("Reading iPhone reply body")?;
    Value::from_reader(std::io::Cursor::new(b))?
        .into_dictionary()
        .context("iPhone reply is not a dictionary")
}
pub fn check_reply(reply: &Dictionary, step: &str) -> Result<()> {
    if let Some(error) = reply.get("Error").and_then(Value::as_string) {
        bail!("{step}: {error}");
    }
    Ok(())
}
pub fn get_value(s: &mut TcpStream, key: &str) -> Result<Value> {
    let reply = lockdown_request(
        s,
        dict([
            ("Request", string("GetValue")),
            ("Label", string("AirCard")),
            ("Key", string(key)),
        ]),
    )?;
    check_reply(&reply, "Read device information")?;
    reply
        .get("Value")
        .cloned()
        .context(format!("Device did not supply {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    struct Fake {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Fake {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            let n = b.len().min(3);
            self.input.read(&mut b[..n])
        }
    }
    impl Write for Fake {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let n = b.len().min(2);
            self.output.extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn identifier_normalization() {
        assert!(same_device(
            "00008130000404D021D8001C",
            "00008130-000404d021d8001c"
        ));
        assert!(!same_device("../record", "../record"));
        assert!(canonical_udid("../record").is_err());
    }
    #[test]
    fn fragmented_frames_and_network_port() {
        let mut xml = Vec::new();
        dict([("Number", Value::Integer(0.into()))])
            .to_writer_xml(&mut xml)
            .unwrap();
        let mut wire = Vec::new();
        for n in [(xml.len() + 16) as u32, 1, 8, 1] {
            wire.extend_from_slice(&n.to_le_bytes());
        }
        wire.extend(xml);
        let mut fake = Fake {
            input: Cursor::new(wire),
            output: Vec::new(),
        };
        mux_request(
            &mut fake,
            dict([
                ("MessageType", string("Connect")),
                (
                    "PortNumber",
                    Value::Integer(u64::from(62078u16.to_be()).into()),
                ),
            ]),
        )
        .unwrap();
        let sent = Value::from_reader(Cursor::new(&fake.output[16..])).unwrap();
        assert_eq!(
            sent.as_dictionary().unwrap()["PortNumber"].as_unsigned_integer(),
            Some(32498)
        );
    }
    #[test]
    fn oversized_reply_rejected_before_allocation() {
        let mut f = Fake {
            input: Cursor::new(u32::MAX.to_be_bytes().to_vec()),
            output: vec![],
        };
        assert!(
            format!("{:#}", lockdown_request(&mut f, dict([])).unwrap_err())
                .contains("Invalid iPhone reply size")
        );
    }
    #[test]
    fn truncated_reply_is_an_error() {
        let mut f = Fake {
            input: Cursor::new(vec![0, 0, 0, 20, b'<']),
            output: vec![],
        };
        assert!(lockdown_request(&mut f, dict([])).is_err());
    }
}
