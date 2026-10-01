use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::afc::AfcClient;
use crate::airlift::{
    LINK_PREFIX, RECOVERED_PREFIX, SOURCE_PREFIX, build_books_plist,
    build_streaming_zip_archive, build_streaming_zip_archive_multi, restore_books, snapshot_books,
    stage_streaming_zip,
};
use crate::airtraffic::sync_assets_via_airtraffic;
use crate::device::{ActiveDeviceSession, ConnectionMode};
use crate::wallet_backup::{capture_original_card, load_original_assets};

#[allow(dead_code)]
pub const TARGET_WALLET_ASSETS: &[&str] = &[
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

const TARGET_WALLET_PREVIEW_ASSETS: &[&str] = &[
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

fn restore_system_file_in_session<L>(
    udid: &str,
    session: &ActiveDeviceSession,
    afc: &AfcClient,
    target_dir: &str,
    leaf: &str,
    payload: &[u8],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let payload_ident = format!("../../{}/payload", source);
    let target_dest = format!("{}/{}", link_dest, leaf);
    let archive = build_streaming_zip_archive(target_dir, payload)
        .context("Failed to build card restoration archive")?;
    let books = build_books_plist(&[link_ident.clone(), payload_ident.clone()])
        .context("Failed to build Books.plist for card restoration")?;

    let restore_res = (|| -> Result<()> {
        stage_streaming_zip(session, &source, &archive)
            .context("Failed to stage card restoration archive")?;
        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload", source);
        if !afc.exists(&link_obj) || !afc.exists(&payload_obj) {
            bail!("Card restoration archive is missing its link or payload");
        }
        afc.write_file("Books/Sync/Books.plist", &books)?;
        sync_assets_via_airtraffic(
            udid,
            session.transport,
            &[
                (link_ident.as_str(), link_dest.as_str()),
                (payload_ident.as_str(), target_dest.as_str()),
            ],
            &mut log,
        )
        .context("AirTraffic failed to restore the original card artwork")?;
        if afc.exists(&payload_obj) {
            bail!("AirTraffic did not consume the card restoration payload");
        }
        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_tree(&source);
    restore_res
}

pub fn read_system_files<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    leaves: &[&str],
    mut log: L,
) -> Result<Vec<(String, Vec<u8>)>>
where
    L: FnMut(&str),
{
    if leaves.is_empty() {
        bail!("No system files were requested");
    }
    for leaf in leaves {
        if leaf.is_empty() || leaf.contains('/') || leaf.contains('\\') {
            bail!("Invalid system file name: {}", leaf);
        }
    }

    let token = generate_token();

    log(&format!(
        "Connecting AFC to read {} current asset(s)...",
        leaves.len()
    ));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)
        .context("Failed to open device session for reading")?;
    log(&format!("Connected over {}.", session.transport.label()));
    let afc = AfcClient::new(&session).context("Failed to open AFC connection")?;
    let snapshot = snapshot_books(&afc).context("Failed to snapshot Books state before reading")?;

    let read_res = (|| -> Result<Vec<(String, Vec<u8>)>> {
        afc.make_directory_recursive("Books/Sync")?;
        let mut files = Vec::new();
        let mut restore_failures = Vec::new();
        for (index, leaf) in leaves.iter().enumerate() {
            let target_path = format!("{}/{}", target_dir.trim_end_matches('/'), leaf);
            let target_tail = target_path
                .strip_prefix("/var/mobile/")
                .context("Card artwork path is outside /var/mobile")?;
            // AirTraffic resolves identifiers below /var/mobile/Media/Airlock/Book.
            let target_ident = format!("../../../{}", target_tail);
            let recovered = format!("{}{}-{}", RECOVERED_PREFIX, token, index);
            let target_books = build_books_plist(std::slice::from_ref(&target_ident))
                .context("Failed to build Books.plist for card export")?;
            afc.write_file("Books/Sync/Books.plist", &target_books)?;

            log(&format!(
                "Exporting current {} into the AFC media area...",
                leaf
            ));
            let export_result = sync_assets_via_airtraffic(
                udid,
                session.transport,
                &[(target_ident.as_str(), recovered.as_str())],
                &mut log,
            );

            let mut data = None;
            for _ in 0..20 {
                if let Ok(bytes) = afc.read_file(&recovered) {
                    data = Some(bytes);
                    break;
                }
                sleep(Duration::from_millis(250));
            }

            if !afc.exists(&recovered) {
                match export_result {
                    Ok(()) => log(&format!("Current {} is not present on this card.", leaf)),
                    Err(err) => log(&format!("Current {} could not be exported: {}", leaf, err)),
                }
                continue;
            }

            if let Some(bytes) = data.as_ref() {
                log(&format!("Read current {} ({} bytes).", leaf, bytes.len()));
            } else {
                log(&format!(
                    "Current {} was exported but AFC could not read it.",
                    leaf
                ));
            }

            // Reading is indirect: move the known file into Media and read it
            // with AFC. Restore it with a fresh link and payload in one sync;
            // AirTraffic may discard links left over from an earlier session.
            let Some(bytes) = data.as_ref() else {
                restore_failures.push(format!("{} retained at AFC path {}", leaf, recovered));
                continue;
            };
            let mut restored = false;
            for attempt in 1..=2 {
                let restore_result = restore_system_file_in_session(
                    udid, &session, &afc, target_dir, leaf, bytes, &mut log,
                );
                if restore_result.is_ok() {
                    restored = true;
                    break;
                }
                log(&format!(
                    "Restore attempt {} for {} did not complete.",
                    attempt, leaf
                ));
            }

            if !restored {
                restore_failures.push(format!("{} retained at AFC path {}", leaf, recovered));
                continue;
            }
            afc.remove_path(&recovered).context(
                "Card artwork was restored, but its temporary Media copy could not be removed",
            )?;
            log(&format!(
                "Restored current {} to its original card path.",
                leaf
            ));
            if !bytes.is_empty() {
                files.push(((*leaf).to_string(), bytes.clone()));
            }
        }

        if !restore_failures.is_empty() {
            bail!(
                "Card artwork restoration failed; original files were preserved in Media: {}",
                restore_failures.join("; ")
            );
        }
        if files.is_empty() {
            bail!("No current card artwork could be read from {}", target_dir);
        }
        Ok(files)
    })();

    sleep(Duration::from_millis(800));
    let restore_res = restore_books(&afc, &snapshot);

    match (read_res, restore_res) {
        (Ok(files), Ok(())) => Ok(files),
        (Err(read_err), Ok(())) => Err(read_err),
        (Ok(_), Err(restore_err)) => {
            Err(restore_err).context("Failed to restore Books state after reading card artwork")
        }
        (Err(read_err), Err(restore_err)) => bail!(
            "{}; restoring Books state after the failed read also failed: {}",
            read_err,
            restore_err
        ),
    }
}

pub fn read_wallet_artwork<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    mut log: L,
) -> Result<(String, Vec<u8>)>
where
    L: FnMut(&str),
{
    anyhow::ensure!(
        crate::scanner::is_valid_card_hash(card_hash),
        "Invalid Wallet card path identifier"
    );

    let trimmed = card_hash.trim_end_matches('=');
    let mut candidates = Vec::new();
    for candidate in [
        card_hash.to_string(),
        trimmed.to_string(),
        format!("{trimmed}="),
        format!("{trimmed}=="),
    ] {
        if !candidate.is_empty() && !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }

    let mut last_error = None;
    for candidate in candidates {
        let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", candidate);
        match read_system_files(
            udid,
            connection_mode,
            &pkpass_dir,
            TARGET_WALLET_PREVIEW_ASSETS,
            &mut log,
        ) {
            Ok(assets) => {
                if candidate != card_hash {
                    log(&format!(
                        "Using device Wallet card hash variant {} for artwork export.",
                        candidate
                    ));
                }
                return assets
                    .into_iter()
                    .next()
                    .context("The current card has no PNG artwork");
            }
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("No card hash candidates were available")))
}

#[allow(dead_code)]
pub const CACHE_FILES: &[&str] = &[
    "FrontFace",
    "PlaceHolder",
    "Preview",
];

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(
        hAlgorithm: *mut std::ffi::c_void,
        pbBuffer: *mut u8,
        cbBuffer: u32,
        dwFlags: u32,
    ) -> i32;
}

pub fn generate_token() -> String {
    let mut bytes = [0u8; 10];
    unsafe {
        let _ = BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            2, // BCRYPT_USE_SYSTEM_PREFERRED_RNG
        );
    }
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn write_system_file<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    leaf_name: &str,
    payload: &[u8],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let payload_ident = format!("../../{}/payload", source);
    let target_dest = format!("{}/{}", link_dest, leaf_name);

    let books_identifiers = vec![link_ident.clone(), payload_ident.clone()];
    let assets_to_sync = [
        (link_ident.as_str(), link_dest.as_str()),
        (payload_ident.as_str(), target_dest.as_str()),
    ];

    log(&format!("Connecting AFC for {}...", leaf_name));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)
        .context("Failed to open device session for writing")?;
    log(&format!("Connected over {}.", session.transport.label()));
    let afc = AfcClient::new(&session).context("Failed to open AFC connection")?;

    let snapshot = snapshot_books(&afc).context("Failed to snapshot Books state before staging")?;

    let archive_data = build_streaming_zip_archive(target_dir, payload)
        .context("Failed to build streaming zip archive")?;

    let books_plist = build_books_plist(&books_identifiers)
        .context("Failed to build Books.plist")?;

    let write_res = (|| -> Result<()> {
        log(&format!("Staging payload archive ({} bytes) via MobileInstallation...", archive_data.len()));
        stage_streaming_zip(&session, &source, &archive_data)
            .context("Failed to stage streaming zip conduit")?;

        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload", source);
        if !afc.exists(&source) || !afc.exists(&link_obj) || !afc.exists(&payload_obj) {
            bail!("StreamingZip completed but staging link/payload object missing on AFC");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;
        if !afc.exists("Books/Sync/Books.plist") {
            bail!("Failed to stage Books/Sync/Books.plist");
        }

        log(&format!("Synchronizing {} with AirTraffic host daemon...", leaf_name));
        sync_assets_via_airtraffic(udid, session.transport, &assets_to_sync, &mut log)
            .context("AirTraffic sync failed")?;

        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(800));

    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("Failed to restore Books state during cleanup")?;
    log(&format!("Successfully written: {}", leaf_name));

    Ok(())
}

pub fn write_system_files_batch<L>(
    udid: &str,
    connection_mode: ConnectionMode,
    target_dir: &str,
    items: &[(&str, &[u8])],
    mut log: L,
) -> Result<()>
where
    L: FnMut(&str),
{
    if items.is_empty() {
        return Ok(());
    }
    if items.len() == 1 {
        return write_system_file(
            udid,
            connection_mode,
            target_dir,
            items[0].0,
            items[0].1,
            log,
        );
    }

    log(&format!("Packaging atomic batch of {} file(s) for {}...", items.len(), target_dir));

    let token = generate_token();
    let source = format!("{}{}", SOURCE_PREFIX, token);
    let link_dest = format!("{}{}", LINK_PREFIX, token);
    let recovered = format!("{}{}", RECOVERED_PREFIX, token);

    let link_ident = format!("../../{}/p0/p1/p2/link", source);
    let mut books_identifiers = Vec::with_capacity(items.len() + 1);
    books_identifiers.push(link_ident.clone());

    let mut assets_to_sync: Vec<(String, String)> = Vec::with_capacity(items.len() + 1);
    assets_to_sync.push((link_ident, link_dest.clone()));

    for (idx, (leaf, _)) in items.iter().enumerate() {
        let payload_ident = format!("../../{}/payload_{}", source, idx);
        let target_dest = format!("{}/{}", link_dest, leaf);
        books_identifiers.push(payload_ident.clone());
        assets_to_sync.push((payload_ident, target_dest));
    }

    log(&format!("Connecting AFC for batch of {} assets...", items.len()));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)
        .context("Failed to open device session for writing")?;
    log(&format!("Connected over {}.", session.transport.label()));
    let afc = AfcClient::new(&session).context("Failed to open AFC connection")?;

    let snapshot = snapshot_books(&afc).context("Failed to snapshot Books state before staging")?;

    let archive_data = build_streaming_zip_archive_multi(target_dir, items)
        .context("Failed to build multi-payload streaming zip archive")?;

    let books_plist = build_books_plist(&books_identifiers)
        .context("Failed to build Books.plist for batch")?;

    let write_res = (|| -> Result<()> {
        log(&format!("Staging multi-payload archive ({} bytes, {} files) via MobileInstallation...", archive_data.len(), items.len()));
        stage_streaming_zip(&session, &source, &archive_data)
            .context("Failed to stage streaming zip conduit")?;

        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload_0", source);
        let fallback_obj = format!("{}/payload", source);
        if !afc.exists(&source) || !afc.exists(&link_obj) || (!afc.exists(&payload_obj) && !afc.exists(&fallback_obj)) {
            bail!("StreamingZip completed but staging link/payload object missing on AFC");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;
        if !afc.exists("Books/Sync/Books.plist") {
            bail!("Failed to stage Books/Sync/Books.plist");
        }

        log(&format!("Synchronizing batch ({} items) with AirTraffic host daemon in single session...", items.len()));
        let assets_refs: Vec<(&str, &str)> = assets_to_sync.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        sync_assets_via_airtraffic(udid, session.transport, &assets_refs, &mut log)
            .context("AirTraffic batch sync failed")?;

        Ok(())
    })();

    let _ = afc.remove_path(&link_dest);
    let _ = afc.remove_path(&recovered);
    let _ = afc.remove_tree(&source);
    sleep(Duration::from_millis(800));

    let restore_res = restore_books(&afc, &snapshot);

    write_res?;
    restore_res.context("Failed to restore Books state during cleanup")?;
    log(&format!("Batch injection of {} file(s) completed successfully!", items.len()));

    Ok(())
}

pub fn flash_wallet_skin<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    skin_png: &[u8],
    skin_pdf: &[u8],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    anyhow::ensure!(
        crate::scanner::is_valid_card_hash(card_hash),
        "Invalid Wallet card path identifier"
    );

    log(&format!("Target Card Hash: {}", card_hash));
    log(&format!(
        "Skin payload size: {} bytes PNG, {} bytes PDF",
        skin_png.len(),
        skin_pdf.len()
    ));
    let resolved_hash = capture_original_card(udid, connection_mode, card_hash, &mut log)
        .context("Failed to inspect the original Wallet card face")?
        .unwrap_or_else(|| card_hash.to_string());
    let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", resolved_hash);

    let total_steps = 3;
    progress(1, total_steps, "Writing card artwork assets (@3x, @2x, .pdf)...");
    log("[1/3] Writing card artwork assets (@3x.png, @2x.png, cardBackgroundCombined.pdf)...");

    let card_assets: [(&str, &[u8]); 3] = [
        ("cardBackgroundCombined@3x.png", skin_png),
        ("cardBackgroundCombined@2x.png", skin_png),
        ("cardBackgroundCombined.pdf", skin_pdf),
    ];

    if let Err(err) = write_system_files_batch(
        udid,
        connection_mode,
        &pkpass_dir,
        &card_assets,
        &mut log,
    ) {
        log(&format!("Notice: Batch write failed ({}), trying individual asset writes...", err));
        for (asset, data) in &card_assets {
            write_system_file(udid, connection_mode, &pkpass_dir, asset, data, &mut log)
                .context(format!("Failed to write card asset {}", asset))?;
        }
    }

    invalidate_wallet_caches(
        udid,
        connection_mode,
        resolved_hash.as_str(),
        &mut progress,
        &mut log,
    );

    progress(total_steps, total_steps, "Card skin updated successfully!");
    log("Card skin write finished! Close and reopen Wallet on iPhone to view.");
    Ok(())
}

pub fn restore_wallet_original<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let original_assets = load_original_assets(udid, card_hash)
        .context("Could not load the original Wallet card face backup")?;
    let asset_refs: Vec<(&str, &[u8])> = original_assets
        .iter()
        .map(|(asset, data)| (asset.as_str(), data.as_slice()))
        .collect();

    log(&format!(
        "Restoring {} original card artwork asset(s) for hash {}...",
        asset_refs.len(),
        card_hash
    ));
    progress(1, 3, "Restoring original card artwork...");

    let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", card_hash);
    if let Err(err) = write_system_files_batch(
        udid,
        connection_mode,
        &pkpass_dir,
        &asset_refs,
        &mut log,
    ) {
        log(&format!(
            "Notice: Restore batch write failed ({}), trying individual asset writes...",
            err
        ));
        for (asset, data) in &original_assets {
            write_system_file(
                udid,
                connection_mode,
                &pkpass_dir,
                asset,
                data,
                &mut log,
            )
            .context(format!("Failed to restore original card asset {}", asset))?;
        }
    }

    invalidate_wallet_caches(
        udid,
        connection_mode,
        card_hash,
        &mut progress,
        &mut log,
    );

    progress(3, 3, "Original card face restored successfully!");
    log("Original card face restored. Close and reopen Wallet on iPhone to view it.");
    Ok(())
}

fn invalidate_wallet_caches<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    progress: &mut F,
    log: &mut L,
) where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let cache_leaves: [(&str, &[u8]); 3] = [
        ("FrontFace", b"corrupted"),
        ("PlaceHolder", b"corrupted"),
        ("Preview", b"corrupted"),
    ];

    for (c_idx, ext) in [".cache", ".pkcache"].iter().enumerate() {
        let step = 2 + c_idx;
        let cache_dir = format!("/var/mobile/Library/Passes/Cards/{}{}", card_hash, ext);
        progress(step, 3, &format!("Clearing {} cache...", ext));
        log(&format!(
            "[{}/3] Invalidating cache leaves in {}...",
            step, cache_dir
        ));

        if write_system_files_batch(
            udid,
            connection_mode,
            &cache_dir,
            &cache_leaves,
            &mut *log,
        )
        .is_err()
        {
            for (leaf, data) in &cache_leaves {
                let _ = write_system_file(
                    udid,
                    connection_mode,
                    &cache_dir,
                    leaf,
                    data,
                    &mut *log,
                );
            }
        }
    }
}

pub fn flash_passcode_theme<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    items: &[(String, String, Vec<u8>)],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let total = items.len();
    log(&format!("Flashing passcode theme ({} assets)...", total));

    let mut dirs_map: std::collections::BTreeMap<String, Vec<(&str, &[u8])>> = std::collections::BTreeMap::new();
    for (tdir, leaf, payload) in items {
        dirs_map.entry(tdir.clone()).or_default().push((leaf.as_str(), payload.as_slice()));
    }

    let total_dirs = dirs_map.len();
    let mut dir_idx = 0;

    for (target_dir, dir_items) in &dirs_map {
        dir_idx += 1;
        let tdir_name = std::path::Path::new(target_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(target_dir);

        progress(
            dir_idx,
            total_dirs,
            &format!("Flashing {} ({} assets in atomic batch)...", tdir_name, dir_items.len()),
        );
        log(&format!("Flashing batch of {} assets into {}...", dir_items.len(), tdir_name));

        let batch_res = write_system_files_batch(
            udid,
            connection_mode,
            target_dir,
            dir_items,
            &mut log,
        );
        if let Err(err) = batch_res {
            log(&format!("Warning: Batch write failed ({}), falling back to file-by-file write...", err));
            for (f_idx, (leaf, payload)) in dir_items.iter().enumerate() {
                progress(
                    f_idx + 1,
                    dir_items.len(),
                    &format!("Fallback [{}/{}]: writing {}...", f_idx + 1, dir_items.len(), leaf),
                );
                write_system_file(
                    udid,
                    connection_mode,
                    target_dir,
                    leaf,
                    payload,
                    &mut log,
                )
                    .context(format!("Failed to write button asset {}", leaf))?;
            }
        }
    }

    progress(total_dirs, total_dirs, "Passcode theme applied successfully!");
    log("Passcode theme successfully written! Lock or reboot iPhone to see new keypad.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_reader_rejects_non_leaf_paths_before_connecting() {
        let result = read_system_files(
            "unused",
            ConnectionMode::Auto,
            "/unused",
            &["../secret"],
            |_| {},
        );
        assert!(result.unwrap_err().to_string().contains("Invalid system file name"));
    }
}
