use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use regex::Regex;

use crate::device::ActiveDeviceSession;

#[cfg(windows)]
unsafe extern "system" {
    fn setsockopt(s: usize, level: i32, optname: i32, optval: *const i8, optlen: i32) -> i32;
}

#[cfg(windows)]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(windows)]
const SO_RCVTIMEO: i32 = 0x1006;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCard {
    pub hash: String,
    pub name: String,
}

pub fn get_cards_storage_path() -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    let dir = PathBuf::from(local_app_data).join("AirCard");
    let _ = fs::create_dir_all(&dir);
    dir.join("cards.json")
}

pub fn load_saved_cards() -> Vec<SavedCard> {
    let path = get_cards_storage_path();
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(cards) = serde_json::from_str::<Vec<SavedCard>>(&content) {
            return cards;
        }
    }
    Vec::new()
}

pub fn save_saved_cards(cards: &[SavedCard]) {
    let path = get_cards_storage_path();
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for c in cards {
        if seen.insert(c.hash.clone()) {
            unique.push(c.clone());
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&unique) {
        let _ = fs::write(path, json);
    }
}

pub fn add_or_update_card(hash: &str, name: &str) {
    let mut cards = load_saved_cards();
    if let Some(existing) = cards.iter_mut().find(|c| c.hash == hash) {
        if !name.is_empty() && (existing.name.is_empty() || existing.name.starts_with("Card ")) {
            existing.name = name.to_string();
        }
    } else {
        cards.push(SavedCard {
            hash: hash.to_string(),
            name: if name.is_empty() {
                format!("Card {}", cards.len() + 1)
            } else {
                name.to_string()
            },
        });
    }
    save_saved_cards(&cards);
}

const WALLET_KEYWORDS: &[&str] = &[
    "passd",
    "passbook",
    "passkit",
    "stockholm",
    "nanopassd",
    "wallet",
    "/cards/",
];

const CONTEXT_KEYWORDS: &[&str] = &[
    "card",
    "pass",
    "payment",
    "pkpass",
    "uniqueid",
    "identifier",
    "face",
    "cache",
    "stockholm",
    "/cards/",
];

const DUMMY_HASHES: &[&str] = &[
    "M6nDwZrkYbFlsodLgCbvyFZQ1cc=",
    "kJL-D0rr-SZhbj2c8nK-OQ9hCMY=",
    "hwAtAmHKYwsQrJbT5cTNDsaxVME=",
];

use std::sync::LazyLock;

static DESC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:description|localizedDescription|passName|title)\s*[:=]\s*['"]([^'"]+)['"]"#)
        .unwrap()
});

static CARD_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"/(?:Cards|Passes/Cards)/([-A-Za-z0-9_+=]{20,44})(?:\.pkpass|\.cache|\.pkcache|/|\s|\x22|'|\)|,|$)").unwrap(),
        Regex::new(r"/([-A-Za-z0-9_+=]{20,44})\.(?:pkpass|cache|pkcache)").unwrap(),
        Regex::new(r"(?:^|[^A-Za-z0-9+/_-])([A-Za-z0-9+/_-]{27}=)(?:$|[^A-Za-z0-9+/_-])").unwrap(),
        Regex::new(r"(?i)(?:card[_\s]?hash|uniqueid|identifier)\s*[:=]\s*['\x22]?([A-Za-z0-9+=_-]{20,44})").unwrap(),
    ]
});

pub fn extract_card_name_from_line(line: &str) -> Option<String> {
    if let Some(caps) = DESC_RE.captures(line) {
        if let Some(m) = caps.get(1) {
            let name = m.as_str().trim();
            if name.len() > 1 && !name.to_lowercase().contains("<private>") {
                return Some(name.to_string());
            }
        }
    }
    None
}

pub fn extract_card_hash_from_line(line: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let has_wallet = WALLET_KEYWORDS.iter().any(|k| lower.contains(k));
    let has_context = CONTEXT_KEYWORDS.iter().any(|k| lower.contains(k));
    if !has_wallet && !has_context {
        return None;
    }

    for r in CARD_REGEXES.iter() {
        if let Some(caps) = r.captures(line) {
            if let Some(m) = caps.get(1) {
                let h = m.as_str().trim().trim_matches(['\'', '"']).trim_end_matches(['.', ',']);
                // Discard UUIDs (8-4-4-4-12 format)
                if h.len() == 36 && h.chars().filter(|&c| c == '-').count() == 4 {
                    continue;
                }
                // Must be a plausible hash length (22..=44) and not a system identifier
                if h.len() < 22 || h.contains("Keyboard") || h.contains("Scene") || h.contains("Input") {
                    continue;
                }
                if DUMMY_HASHES.contains(&h) || DUMMY_HASHES.iter().any(|d| d.trim_end_matches('=') == h) {
                    continue;
                }
                return Some(h.to_string());
            }
        }
    }

    None
}

pub fn scan_syslog_for_cards<F>(
    udid: Option<&str>,
    stop_flag: Arc<AtomicBool>,
    mut on_card_found: F,
) -> Result<()>
where
    F: FnMut(String, String),
{
    let session = ActiveDeviceSession::open(udid)
        .context("Failed to connect to device for syslog scanning")?;
    let libs = &session.libs;
    let service_conn = session.start_service("com.apple.syslog_relay")
        .context("Failed to start com.apple.syslog_relay service")?;

    let raw_socket = unsafe { (libs.amd_service_connection_get_socket)(service_conn) };
    if raw_socket <= 0 {
        unsafe { (libs.amd_service_connection_invalidate)(service_conn) };
        anyhow::bail!("Invalid syslog socket");
    }

    // Set socket receive timeout
    #[cfg(windows)]
    unsafe {
        let timeout_ms: u32 = 500;
        setsockopt(
            raw_socket as usize,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &timeout_ms as *const u32 as *const i8,
            std::mem::size_of::<u32>() as i32,
        );
    }

    let mut buffer = [0u8; 8192];
    let mut line_acc = Vec::with_capacity(1024);

    while !stop_flag.load(Ordering::Relaxed) {
        let bytes_read = unsafe {
            (libs.amd_service_connection_receive)(
                service_conn,
                buffer.as_mut_ptr(),
                buffer.len(),
            )
        };

        if bytes_read > 0 {
            let slice = &buffer[..bytes_read as usize];
            for &b in slice {
                if b == b'\n' || b == b'\0' {
                    if !line_acc.is_empty() {
                        let line = String::from_utf8_lossy(&line_acc);
                        if let Some(hash) = extract_card_hash_from_line(&line) {
                            let name = extract_card_name_from_line(&line).unwrap_or_default();
                            add_or_update_card(&hash, &name);
                            on_card_found(hash, name);
                        }
                        line_acc.clear();
                    }
                } else if b != b'\r' {
                    line_acc.push(b);
                }
            }
        } else if bytes_read == 0 {
            break; // Socket closed
        } else {
            // Timeout or transient: sleep briefly to avoid pegging CPU
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    unsafe {
        (libs.amd_service_connection_invalidate)(service_conn);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_card_hash() {
        let line1 = "passd[123]: Card hash: 'OM6NYhwXMZrAw0sRUjR62wmF4ZQ=' loaded";
        assert_eq!(
            extract_card_hash_from_line(line1),
            Some("OM6NYhwXMZrAw0sRUjR62wmF4ZQ=".to_string())
        );

        let line2 = "nanopassd: Accessing /var/mobile/Library/Passes/Cards/d64fKk0kyHWP11IWV2GRLud4XQk.pkpass";
        assert_eq!(
            extract_card_hash_from_line(line2),
            Some("d64fKk0kyHWP11IWV2GRLud4XQk".to_string())
        );

        // Dummy/unrelated lines should be ignored
        let dummy = "passd: Using dummy hash hwAtAmHKYwsQrJbT5cTNDsaxVME=";
        assert_eq!(extract_card_hash_from_line(dummy), None);
    }

    #[test]
    fn test_extract_card_name() {
        let line = "passd[456]: Pass with localizedDescription = 'Apple Card' updated";
        assert_eq!(
            extract_card_name_from_line(line),
            Some("Apple Card".to_string())
        );
    }

    #[test]
    fn test_syslog_service_receive() {
        let session = match ActiveDeviceSession::open(None) {
            Ok(s) => s,
            Err(e) => {
                println!("No device connected: {:?}", e);
                return;
            }
        };
        let libs = &session.libs;
        let conn = session.start_service("com.apple.syslog_relay").expect("start syslog_relay");
        let raw_socket = unsafe { (libs.amd_service_connection_get_socket)(conn) };
        unsafe {
            let timeout_ms: u32 = 500;
            setsockopt(
                raw_socket as usize,
                SOL_SOCKET,
                SO_RCVTIMEO,
                &timeout_ms as *const u32 as *const i8,
                std::mem::size_of::<u32>() as i32,
            );
        }
        let mut buf = [0u8; 4096];
        let start = std::time::Instant::now();
        let n = unsafe { (libs.amd_service_connection_receive)(conn, buf.as_mut_ptr(), buf.len()) };
        println!("AMDServiceConnectionReceive returned: {} in {:?}", n, start.elapsed());
        unsafe { (libs.amd_service_connection_invalidate)(conn) };
    }
}
