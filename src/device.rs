use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::ptr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::apple::{
    AMDServiceConnectionRef, AMDeviceRef, AppleLibraries, get_apple_libraries,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub udid: String,
    pub name: String,
    pub product_type: String,
    pub ios_version: String,
    pub build_version: String,
}

impl std::fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}, iOS {} [{}])",
            self.name, self.product_type, self.ios_version, self.build_version
        )
    }
}

pub struct UsbmuxDeviceEntry {
    pub udid: String,
    pub connection_type: String,
    pub properties_plist: Vec<u8>,
}

pub fn query_usbmux_devices() -> Result<Vec<UsbmuxDeviceEntry>> {
    let addr: SocketAddr = "127.0.0.1:27015".parse().unwrap();
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .context("Could not connect to Apple Mobile Device Service (usbmuxd) at 127.0.0.1:27015. Please ensure iTunes or Apple Mobile Device Support is installed and the service is running.")?;

    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let mut req_dict = HashMap::new();
    req_dict.insert("MessageType".to_string(), plist::Value::String("ListDevices".to_string()));
    req_dict.insert("ClientVersionString".to_string(), plist::Value::String("aircard".to_string()));
    req_dict.insert("ProgName".to_string(), plist::Value::String("aircard".to_string()));

    let mut plist_bytes = Vec::new();
    plist::to_writer_xml(&mut plist_bytes, &plist::Value::Dictionary(req_dict.into_iter().collect()))
        .context("Failed to serialize ListDevices request")?;

    let length = (plist_bytes.len() + 16) as u32;
    let version = 1u32;
    let msg_type = 8u32; // PLIST
    let tag = 1u32;

    let mut header = Vec::with_capacity(16);
    header.extend_from_slice(&length.to_le_bytes());
    header.extend_from_slice(&version.to_le_bytes());
    header.extend_from_slice(&msg_type.to_le_bytes());
    header.extend_from_slice(&tag.to_le_bytes());

    stream.write_all(&header)?;
    stream.write_all(&plist_bytes)?;
    stream.flush()?;

    // Read 16-byte response header
    let mut resp_header = [0u8; 16];
    stream.read_exact(&mut resp_header)?;

    let resp_len = u32::from_le_bytes([resp_header[0], resp_header[1], resp_header[2], resp_header[3]]) as usize;
    if resp_len < 16 {
        bail!("Invalid usbmux response length: {}", resp_len);
    }

    let mut payload = vec![0u8; resp_len - 16];
    stream.read_exact(&mut payload)?;

    let val = plist::Value::from_reader(std::io::Cursor::new(payload))
        .context("Failed to parse usbmux ListDevices response plist")?;

    let root_dict = val.as_dictionary().context("Expected dictionary in usbmux response")?;
    let device_list = root_dict.get("DeviceList").and_then(|v| v.as_array()).context("Expected DeviceList array in usbmux response")?;

    let mut result = Vec::new();
    for entry in device_list {
        if let Some(d) = entry.as_dictionary() {
            if let Some(props_val) = d.get("Properties") {
                if let Some(props_dict) = props_val.as_dictionary() {
                    let serial = props_dict
                        .get("SerialNumber")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string();
                    let conn_type = props_dict
                        .get("ConnectionType")
                        .and_then(|v| v.as_string())
                        .unwrap_or("USB")
                        .to_string();

                    let mut props_binary = Vec::new();
                    plist::to_writer_binary(&mut props_binary, props_val)
                        .context("Failed to serialize device properties to binary plist")?;

                    result.push(UsbmuxDeviceEntry {
                        udid: serial,
                        connection_type: conn_type,
                        properties_plist: props_binary,
                    });
                }
            }
        }
    }

    Ok(result)
}

pub fn list_connected_devices() -> Result<Vec<DeviceInfo>> {
    let libs = get_apple_libraries()?;
    let entries = query_usbmux_devices()?;

    let mut result = Vec::new();

    for entry in entries {
        let cf_props = libs.create_cf_plist_from_bytes(&entry.properties_plist)?;
        let dev = unsafe { (libs.am_device_create_from_properties)(cf_props.raw) };
        if dev.is_null() {
            continue;
        }

        let mut name = "iPhone".to_string();
        let mut product_type = "iPhone".to_string();
        let mut ios_version = "Unknown".to_string();
        let mut build_version = "Unknown".to_string();

        unsafe {
            let connected = (libs.am_device_connect)(dev) == 0;
            if connected {
                let _ = (libs.am_device_validate_pairing)(dev);

                if let Ok(k) = libs.create_cf_string("DeviceName") {
                    let v = (libs.am_device_copy_value)(dev, ptr::null(), k.raw);
                    if !v.is_null() {
                        name = libs.to_rust_string(v);
                        (libs.cf_release)(v);
                    }
                }
                if let Ok(k) = libs.create_cf_string("ProductType") {
                    let v = (libs.am_device_copy_value)(dev, ptr::null(), k.raw);
                    if !v.is_null() {
                        product_type = libs.to_rust_string(v);
                        (libs.cf_release)(v);
                    }
                }
                if let Ok(k) = libs.create_cf_string("ProductVersion") {
                    let v = (libs.am_device_copy_value)(dev, ptr::null(), k.raw);
                    if !v.is_null() {
                        ios_version = libs.to_rust_string(v);
                        (libs.cf_release)(v);
                    }
                }
                if let Ok(k) = libs.create_cf_string("BuildVersion") {
                    let v = (libs.am_device_copy_value)(dev, ptr::null(), k.raw);
                    if !v.is_null() {
                        build_version = libs.to_rust_string(v);
                        (libs.cf_release)(v);
                    }
                }
                (libs.am_device_disconnect)(dev);
            }
            (libs.cf_release)(dev);
        }

        result.push(DeviceInfo {
            udid: entry.udid,
            name,
            product_type,
            ios_version,
            build_version,
        });
    }

    Ok(result)
}

#[allow(dead_code)]
pub struct ActiveDeviceSession {
    pub libs: Arc<AppleLibraries>,
    pub device: AMDeviceRef,
    pub udid: String,
    connected: bool,
    session_started: bool,
}

impl Drop for ActiveDeviceSession {
    fn drop(&mut self) {
        unsafe {
            if self.session_started {
                (self.libs.am_device_stop_session)(self.device);
            }
            if self.connected {
                (self.libs.am_device_disconnect)(self.device);
            }
            if !self.device.is_null() {
                (self.libs.cf_release)(self.device);
            }
        }
    }
}

impl ActiveDeviceSession {
    pub fn open(target_udid: Option<&str>) -> Result<Self> {
        let libs = get_apple_libraries()?;
        let entries = query_usbmux_devices()?;

        let matched_entry = if let Some(target) = target_udid {
            entries
                .into_iter()
                .find(|e| e.udid.eq_ignore_ascii_case(target))
                .context(format!("iPhone with UDID {} not found", target))?
        } else {
            entries
                .into_iter()
                .find(|e| e.connection_type.eq_ignore_ascii_case("USB"))
                .context("No connected iPhone found via USB")?
        };

        let udid = matched_entry.udid.clone();
        let cf_props = libs.create_cf_plist_from_bytes(&matched_entry.properties_plist)?;

        unsafe {
            let device = (libs.am_device_create_from_properties)(cf_props.raw);
            if device.is_null() {
                bail!("AMDeviceCreateFromProperties failed");
            }

            let connect_status = (libs.am_device_connect)(device);
            if connect_status != 0 {
                (libs.cf_release)(device);
                bail!("AMDeviceConnect failed with code {}", connect_status);
            }

            if (libs.am_device_is_paired)(device) == 0 {
                (libs.am_device_pair)(device);
            }

            let mut validate_status = (libs.am_device_validate_pairing)(device);
            if validate_status != 0 {
                (libs.am_device_pair)(device);
                validate_status = (libs.am_device_validate_pairing)(device);
            }
            if validate_status != 0 {
                (libs.am_device_disconnect)(device);
                (libs.cf_release)(device);
                bail!("AMDeviceValidatePairing failed with code {}. Ensure iPhone is unlocked and computer is trusted.", validate_status);
            }

            let session_status = (libs.am_device_start_session)(device);
            if session_status != 0 {
                (libs.am_device_disconnect)(device);
                (libs.cf_release)(device);
                bail!("AMDeviceStartSession failed with code {}", session_status);
            }

            Ok(Self {
                libs,
                device,
                udid,
                connected: true,
                session_started: true,
            })
        }
    }

    pub fn start_service(&self, service_name: &str) -> Result<AMDServiceConnectionRef> {
        let cf_name = self.libs.create_cf_string(service_name)?;
        let mut service_conn: AMDServiceConnectionRef = ptr::null_mut();
        let status = unsafe {
            (self.libs.am_device_secure_start_service)(
                self.device,
                cf_name.raw,
                ptr::null(),
                &mut service_conn,
            )
        };
        if status != 0 || service_conn.is_null() {
            bail!("AMDeviceSecureStartService('{}') failed with code {}", service_name, status);
        }
        Ok(service_conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usbmux_query() {
        match query_usbmux_devices() {
            Ok(devs) => {
                println!("Detected {} usbmux device(s)", devs.len());
                if !devs.is_empty() {
                    println!("Detected usbmux device: {}", devs[0].udid);
                }
            }
            Err(e) => {
                println!("usbmuxd not running on this host (expected in CI): {e}");
            }
        }
    }

    #[test]
    fn test_list_connected_devices() {
        match list_connected_devices() {
            Ok(devs) => {
                for d in &devs {
                    println!("Connected iPhone: {}", d);
                }
            }
            Err(e) => {
                println!("Apple Mobile Device Support not installed on this host (expected in CI): {e}");
            }
        }
    }
}
