use crate::wallet_connection::WalletConnection;
use anyhow::{Context, Result};
#[cfg(test)]
use base64::Engine;
use regex::Regex;
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::PathBuf,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedCard {
    pub hash: String,
    pub name: String,
}
pub fn get_cards_storage_path() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
        .join("AirCard")
        .join("cards.json")
}
fn normalized_hash(h: &str) -> Option<String> {
    let h = h.trim_matches(['\'', '"']).trim_end_matches(['.', ',']);
    // Preserve opaque Wallet path identifiers, including original padding.
    if !(20..=44).contains(&h.len())
        || h.len() == 36 && h.contains('-')
        || !h
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+-_=".contains(&b))
        || h.trim_end_matches('=').contains('=')
        || h.ends_with("===")
        || h.trim_end_matches('=')
            .bytes()
            .all(|b| b == h.as_bytes()[0])
    {
        return None;
    }
    if DUMMY_HASHES
        .iter()
        .any(|dummy| dummy.trim_end_matches('=') == h.trim_end_matches('='))
    {
        return None;
    }
    Some(h.to_owned())
}
pub fn is_valid_card_hash(h: &str) -> bool {
    normalized_hash(h).is_some()
}
pub fn load_saved_cards() -> Vec<SavedCard> {
    fs::read(get_cards_storage_path())
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<SavedCard>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|c| is_valid_card_hash(&c.hash))
        .collect()
}
fn save_cards(cards: &[SavedCard]) -> Result<()> {
    let path = get_cards_storage_path();
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, serde_json::to_vec_pretty(cards)?)?;
    Ok(())
}
const DUMMY_HASHES: &[&str] = &[
    "M6nDwZrkYbFlsodLgCbvyFZQ1cc=",
    "kJL-D0rr-SZhbj2c8nK-OQ9hCMY=",
    "hwAtAmHKYwsQrJbT5cTNDsaxVME=",
];
const WALLET_KEYWORDS: &[&str] = &[
    "passd",
    "passbook",
    "passkit",
    "stockholm",
    "nanopassd",
    "npkcompanion",
    "wallet",
    "/cards/",
    "/passes/",
    ".pkpass",
];
static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?:localizedDescription|description|passName|title)\s*[:=]\s*['"]([^'"]+)['"]"#,
    )
    .unwrap()
});
static HIDDEN_IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:pass\s+uniqueID|passUniqueIdentifier|passIdentifier|cardIdentifier|card[_\s]?hash|pass[_\s]?hash)\s*[:=]?\s*<private>").unwrap()
});
static HASH_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
    Regex::new(r#"(?i)/(?:Cards|Passes/Cards)/([A-Za-z0-9+_=-]{20,44})(?:\.pkpass|\.pkcache|\.cache|/|\s|["'),]|$)"#).unwrap(),
    Regex::new(r#"/([A-Za-z0-9+_=-]{20,44})\.(?:pkpass|pkcache|cache)"#).unwrap(),
    Regex::new(r#"(?i)(?:card[_\s]?(?:hash|id)|pass[_\s]?(?:hash|id)|unique[_\s]?(?:id|identifier)|passUniqueIdentifier|passIdentifier|cardIdentifier)\s*(?:[:=]\s*|\s+)['"]?([A-Za-z0-9+_=-]{20,44})(?:[^A-Za-z0-9+/_=-]|$)"#).unwrap(),
    Regex::new(r#"(?:^|[^A-Za-z0-9+/_=-])([A-Za-z0-9+/_-]{27}=|[A-Za-z0-9+/_-]{43}=)(?:[^A-Za-z0-9+/_=-]|$)"#).unwrap(),
]
});
pub fn extract_card_name_from_line(line: &str) -> Option<String> {
    NAME.captures(line)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_owned())
        .filter(|s| s.len() > 1 && !s.to_lowercase().contains("<private>"))
}
pub fn extract_card_hashes(line: &str) -> Vec<String> {
    let lower = line.to_lowercase();
    if !WALLET_KEYWORDS.iter().any(|k| lower.contains(k)) {
        return vec![];
    }
    let mut hashes = Vec::new();
    let mut seen = HashSet::new();
    for pattern in HASH_PATTERNS.iter() {
        for c in pattern.captures_iter(line) {
            if let Some(hash) = c.get(1).and_then(|m| normalized_hash(m.as_str())) {
                if seen.insert(hash.clone()) {
                    hashes.push(hash)
                }
            }
        }
    }
    hashes
}
/// Bound unterminated messages and preserve UTF-8 across USB reads.
#[derive(Default)]
struct LineDecoder {
    pending: Vec<u8>,
    discarding: bool,
}
impl LineDecoder {
    fn feed(&mut self, bytes: &[u8], mut line: impl FnMut(&str)) {
        for b in bytes {
            if *b == b'\n' || *b == 0 {
                if !self.discarding && !self.pending.is_empty() {
                    line(&String::from_utf8_lossy(&self.pending));
                }
                self.pending.clear();
                self.discarding = false;
            } else if *b != b'\r' && !self.discarding {
                if self.pending.len() < 256 * 1024 {
                    self.pending.push(*b)
                } else {
                    self.pending.clear();
                    self.discarding = true;
                }
            }
        }
    }
}
pub fn scan_syslog_for_cards<F: FnMut(String, String), L: FnMut(String)>(
    udid: Option<&str>,
    connection_mode: crate::device::ConnectionMode,
    stop: Arc<AtomicBool>,
    mut on_card_found: F,
    mut log: L,
) -> Result<()> {
    let opened = WalletConnection::open(udid, connection_mode, &stop, &mut log);
    if stop.load(Ordering::Relaxed) {
        log("Scan stopped.".into());
        return Ok(());
    }
    let mut connection = opened.context("Wallet scan connection failed")?;
    let mut decoder = LineDecoder::default();
    let mut buf = [0; 16384];
    let mut seen = HashSet::new();
    let mut saved = load_saved_cards();
    let mut lines = 0usize;
    let mut wallet_lines = 0usize;
    let mut hidden_identifier_lines = 0usize;
    let mut report = Instant::now();
    let mut privacy_reported = false;
    while !stop.load(Ordering::Relaxed) {
        match connection.stream.read(&mut buf) {
            Ok(0) => {
                anyhow::bail!("iPhone closed the wallet log connection. Reconnect and scan again")
            }
            Ok(n) => decoder.feed(&buf[..n], |line| {
                lines += 1;
                let lower = line.to_lowercase();
                if WALLET_KEYWORDS.iter().any(|k| lower.contains(k)) {
                    wallet_lines += 1;
                    if HIDDEN_IDENTIFIER.is_match(line) {
                        hidden_identifier_lines += 1;
                    }
                }
                let hashes = extract_card_hashes(line);
                let name = if hashes.len() == 1 {
                    extract_card_name_from_line(line).unwrap_or_default()
                } else {
                    String::new()
                };
                for hash in hashes {
                    if !seen.insert(hash.clone()) {
                        continue;
                    }
                    let card_name = if name.is_empty() {
                        saved
                            .iter()
                            .find(|c| c.hash == hash)
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| format!("Card {}", saved.len() + 1))
                    } else {
                        name.clone()
                    };
                    if !saved.iter().any(|c| c.hash == hash) {
                        saved.push(SavedCard {
                            hash: hash.clone(),
                            name: card_name.clone(),
                        });
                    }
                    if let Err(e) = save_cards(&saved) {
                        log(format!("Card detected, but saving it failed: {e:#}"));
                    }
                    log(format!(
                        "Wallet card detected: {card_name}. Hash is ready in the card field."
                    ));
                    on_card_found(hash, card_name);
                }
            }),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e).context("Wallet log stream was interrupted"),
        }
        if report.elapsed() >= Duration::from_secs(10) {
            log(format!(
                "Scan active: {lines} log lines, {wallet_lines} Wallet lines, {} cards detected.",
                seen.len()
            ));
            if hidden_identifier_lines > 0 && !privacy_reported {
                privacy_reported = true;
                log("Wallet received the card selection, but iOS hid its identifier as <private>. For a supported transit card, keep scanning and open Wallet > card > (...) > Card Details > Turn On Service Mode. Other card types may not offer this. Previously detected cards do not mean all cards were found.".into());
            } else if seen.is_empty() {
                log(if wallet_lines == 0 {
                    "No Wallet activity yet. Open Wallet on iPhone and tap a card."
                } else {
                    "Wallet activity received, but no usable card identifier yet. Open card details while scanning."
                }.into());
            }
            report = Instant::now();
        }
    }
    log(format!("Scan stopped. {} cards detected.", seen.len()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "OM6NYhwXMZrAw0sRUjR62wmF4ZQ=";
    const B: &str = "d64fKk0kyHWP11IWV2GRLud4XQk=";
    #[test]
    fn selection_without_colon_and_specific_redaction() {
        let hash = "aB12cD34eF56gH78iJ90kL";
        assert_eq!(extract_card_hashes(&format!("Passbook: pass uniqueID {hash}, accountID (null)")), vec![hash]);
        assert!(HIDDEN_IDENTIFIER.is_match("Passbook: pass uniqueID <private>, accountID (null)"));
        assert!(HIDDEN_IDENTIFIER.is_match("PassKitCore: selected pass uniqueID: <private>"));
        assert!(!HIDDEN_IDENTIFIER.is_match("passd: description=<private>"));
        assert!(!HIDDEN_IDENTIFIER.is_match(&format!("passd: pass uniqueID {hash}, title=<private>")));
    }
    #[test]
    fn opaque_paths_identifiers_and_redaction_are_independent() {
        let opaque = "aB12cD34eF56gH78iJ90kL";
        assert_eq!(
            extract_card_hashes(&format!("passd: title=<private> /Cards/{opaque}.pkpass")),
            vec![opaque]
        );
        assert_eq!(
            extract_card_hashes(&format!("passd: passUniqueIdentifier='{A}'")),
            vec![A]
        );
        assert_eq!(
            extract_card_hashes(&format!("passd: uniqueIdentifier='{B}'")),
            vec![B]
        );
        assert!(!is_valid_card_hash("9qzwRxHhdY6jmi/stAk+Sd8X08g="));
    }
    #[test]
    fn padded_paths_and_multiple_cards() {
        assert_eq!(
            extract_card_hashes(&format!(
                "passd: /var/mobile/Library/Passes/Cards/{A}.pkpass /Cards/{B}.pkcache"
            )),
            vec![A, B]
        );
    }
    #[test]
    fn sha256_urlsafe_with_many_separators() {
        let bytes: [u8; 32] = std::array::from_fn(|i| if i % 2 == 0 { 255 } else { (i * 7) as u8 });
        let h = base64::engine::general_purpose::URL_SAFE.encode(bytes);
        assert!(h.matches('_').count() > 1);
        assert!(is_valid_card_hash(&h));
        assert_eq!(
            extract_card_hashes(&format!("passd: /Cards/{}.pkpass", h.trim_end_matches('='))),
            vec![h.trim_end_matches('=').to_owned()]
        );
    }
    #[test]
    fn invalid_candidate_does_not_hide_valid_one() {
        assert_eq!(
            extract_card_hashes(&format!(
                "passd: Card hash: AAAAAAAAAAAAAAAAAAAAAAAAAAA=; Card hash: '{A}'"
            )),
            vec![A]
        );
    }
    #[test]
    fn unrelated_private_dummy_and_malformed() {
        for line in [
            format!("networkd: token {A}"),
            "passd: card hash: <private>".into(),
            format!("passd: dummy {}", DUMMY_HASHES[0]),
            "passd: PresentationBinderIndirectAccessHosting-".into(),
        ] {
            assert!(extract_card_hashes(&line).is_empty(), "{line}");
        }
        assert!(!is_valid_card_hash("AAAAAAAAAAAAAAAAAAAAAAAAAAA="));
        assert!(!is_valid_card_hash("../../etc/passwd"));
    }
    #[test]
    fn fragmented_utf8_nul_crlf_and_oversized_lines() {
        let text = format!("passd: title='交通卡' card hash: '{A}'\r\npassd: /Cards/{B}.cache\0");
        let mut d = LineDecoder::default();
        let mut lines = vec![];
        for b in text.as_bytes().chunks(1) {
            d.feed(b, |s| lines.push(s.to_owned()));
        }
        assert_eq!(lines.len(), 2);
        assert_eq!(
            extract_card_name_from_line(&lines[0]).as_deref(),
            Some("交通卡")
        );
        assert_eq!(extract_card_hashes(&lines[1]), vec![B]);
        d.feed(&vec![b'x'; 300_000], |_| panic!("unterminated line"));
        assert!(d.pending.len() <= 256 * 1024);
        d.feed(b"\npassd: ok\n", |s| assert_eq!(s, "passd: ok"));
    }
    #[test]
    fn repeated_patterns_are_deduplicated() {
        assert_eq!(
            extract_card_hashes(&format!("passd: card hash: '{A}', path /Cards/{A}.pkpass")),
            vec![A]
        );
    }
}
