use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::apple::{ATHostConnectionRef, get_apple_libraries};

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut std::ffi::c_void,
        pbBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> i32;
}

fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    unsafe {
        let _ = BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, 2);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

pub fn sync_assets_via_airtraffic(udid: &str, assets: &[(&str, &str)]) -> Result<()> {
    let libs = get_apple_libraries()?;
    let cf_udid = libs.create_cf_string(udid)?;

    let conn: ATHostConnectionRef = unsafe { (libs.at_host_connection_create)(cf_udid.raw) };
    if conn.is_null() {
        bail!("ATHostConnectionCreate failed for UDID: {}", udid);
    }

    let run_sync = || -> Result<()> {
        // 1. Wait for SyncAllowed message
        let mut sync_allowed = false;
        for _ in 0..15 {
            let msg = unsafe { (libs.at_host_connection_read_message)(conn) };
            if msg.is_null() {
                sleep(Duration::from_millis(150));
                continue;
            }
            let name_ref = unsafe { (libs.at_cf_message_get_name)(msg) };
            let name = libs.to_rust_string(name_ref);
            unsafe { (libs.cf_release)(msg) };
            if name == "SyncAllowed" {
                sync_allowed = true;
                break;
            }
        }
        if !sync_allowed {
            bail!("AirTraffic: SyncAllowed message not received from device");
        }

        // 2. Send HostInfo
        let mut host_info_dict = HashMap::new();
        host_info_dict.insert("Type".to_string(), plist::Value::String("iTunes".to_string()));
        host_info_dict.insert("Version".to_string(), plist::Value::String("13.7.0.161".to_string()));
        host_info_dict.insert("MacOSVersion".to_string(), plist::Value::String("Windows NT 10.0".to_string()));
        host_info_dict.insert("SyncHostName".to_string(), plist::Value::String("airlift".to_string()));
        host_info_dict.insert("LibraryID".to_string(), plist::Value::String(generate_uuid_v4()));
        host_info_dict.insert("SyncedDataclasses".to_string(), plist::Value::Array(vec![plist::Value::String("Book".to_string())]));
        host_info_dict.insert("SyncedAssetTypes".to_string(), plist::Value::Array(vec![plist::Value::String("Book".to_string())]));
        host_info_dict.insert("Wakeable".to_string(), plist::Value::Boolean(false));

        let mut host_info_bytes = Vec::new();
        plist::to_writer_binary(&mut host_info_bytes, &plist::Value::Dictionary(host_info_dict.into_iter().collect()))?;
        let cf_host_info = libs.create_cf_plist_from_bytes(&host_info_bytes)?;

        unsafe {
            (libs.at_host_connection_send_host_info)(conn, cf_host_info.raw);
        }
        sleep(Duration::from_millis(200));

        // 3. Send SyncRequest
        let mut dataclasses_bytes = Vec::new();
        plist::to_writer_binary(&mut dataclasses_bytes, &plist::Value::Array(vec![plist::Value::String("Book".to_string())]))?;
        let cf_dataclasses = libs.create_cf_plist_from_bytes(&dataclasses_bytes)?;

        let mut anchors_bytes = Vec::new();
        plist::to_writer_binary(&mut anchors_bytes, &plist::Value::Dictionary(HashMap::<String, plist::Value>::new().into_iter().collect()))?;
        let cf_anchors = libs.create_cf_plist_from_bytes(&anchors_bytes)?;

        unsafe {
            (libs.at_host_connection_send_sync_request)(
                conn,
                cf_dataclasses.raw,
                cf_anchors.raw,
                cf_host_info.raw,
            );
        }

        // 4. Wait for ReadyForSync
        let mut ready_for_sync = false;
        for _ in 0..20 {
            let msg = unsafe { (libs.at_host_connection_read_message)(conn) };
            if msg.is_null() {
                sleep(Duration::from_millis(150));
                continue;
            }
            let name_ref = unsafe { (libs.at_cf_message_get_name)(msg) };
            let name = libs.to_rust_string(name_ref);
            unsafe { (libs.cf_release)(msg) };
            if name == "ReadyForSync" {
                ready_for_sync = true;
                break;
            }
        }
        if !ready_for_sync {
            bail!("AirTraffic: ReadyForSync message not received from device");
        }

        // 5. Send MetadataSyncFinished
        let mut sync_types_dict = HashMap::new();
        sync_types_dict.insert("Book".to_string(), plist::Value::Integer(1.into()));
        let mut sync_types_bytes = Vec::new();
        plist::to_writer_binary(&mut sync_types_bytes, &plist::Value::Dictionary(sync_types_dict.into_iter().collect()))?;
        let cf_sync_types = libs.create_cf_plist_from_bytes(&sync_types_bytes)?;

        unsafe {
            (libs.at_host_connection_send_metadata_sync_finished)(conn, cf_sync_types.raw, cf_anchors.raw);
        }

        // 6. Read AssetManifest
        let cf_key_manifest = libs.create_cf_string("AssetManifest")?;
        let mut manifest_val: Option<plist::Value> = None;

        for _ in 0..30 {
            let msg = unsafe { (libs.at_host_connection_read_message)(conn) };
            if msg.is_null() {
                sleep(Duration::from_millis(150));
                continue;
            }
            let name_ref = unsafe { (libs.at_cf_message_get_name)(msg) };
            let name = libs.to_rust_string(name_ref);
            if name == "AssetManifest" {
                let param = unsafe { (libs.at_cf_message_get_param)(msg, cf_key_manifest.raw) };
                if !param.is_null() {
                    if let Ok(bytes) = libs.cf_plist_to_bytes(param) {
                        manifest_val = plist::Value::from_reader(std::io::Cursor::new(bytes)).ok();
                    }
                }
                unsafe { (libs.cf_release)(msg) };
                break;
            } else if name == "SyncFailed" || name == "SyncFinished" {
                unsafe { (libs.cf_release)(msg) };
                bail!("AirTraffic returned unexpected terminating message: {}", name);
            }
            unsafe { (libs.cf_release)(msg) };
        }

        let Some(manifest) = manifest_val else {
            bail!("AirTraffic: AssetManifest was not received or failed to parse");
        };

        // Validate Book manifest contains downloads
        let book_entries = manifest
            .as_dictionary()
            .and_then(|d| d.get("Book"))
            .and_then(|v| v.as_array())
            .context("AssetManifest does not contain Book list")?;

        let mut available_downloads = Vec::new();
        for entry in book_entries {
            if let Some(dict) = entry.as_dictionary() {
                let is_dl = dict.get("IsDownload").and_then(|b| b.as_boolean()).unwrap_or(false);
                if is_dl {
                    if let Some(asset_id) = dict.get("AssetID").and_then(|s| s.as_string()) {
                        available_downloads.push(asset_id.to_string());
                    }
                }
            }
        }

        for (ident, _) in assets {
            if !available_downloads.iter().any(|d| d == ident) {
                bail!(
                    "Asset '{}' missing from device download manifest (available: {:?})",
                    ident,
                    available_downloads
                );
            }
        }

        // 7. Dispatch AssetCompleted for each asset
        let cf_dataclass = libs.create_cf_string("Book")?;
        for (idx, (ident, dest)) in assets.iter().enumerate() {
            let cf_ident = libs.create_cf_string(ident)?;
            let cf_dest = libs.create_cf_string(dest)?;

            unsafe {
                (libs.at_host_connection_send_asset_completed)(
                    conn,
                    cf_ident.raw,
                    cf_dataclass.raw,
                    cf_dest.raw,
                );
            }

            if idx + 1 < assets.len() {
                sleep(Duration::from_millis(900));
            }
        }

        sleep(Duration::from_millis(2000));
        Ok(())
    };

    let result = run_sync();
    unsafe {
        (libs.at_host_connection_release)(conn);
    }
    result
}
