use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::afc::AfcClient;
use crate::airlift::{
    LINK_PREFIX, RECOVERED_PREFIX, SOURCE_PREFIX, build_books_plist, build_streaming_zip_archive,
    build_streaming_zip_archive_multi, restore_books, snapshot_books, stage_streaming_zip,
};
use crate::airtraffic::sync_assets_via_airtraffic;
use crate::device::{ActiveDeviceSession, ConnectionMode};

#[allow(dead_code)]
pub const TARGET_WALLET_ASSETS: &[&str] = &[
    "cardBackgroundCombined@3x.png",
    "cardBackgroundCombined@2x.png",
    "cardBackgroundCombined.pdf",
];

#[allow(dead_code)]
pub const CACHE_FILES: &[&str] = &["FrontFace", "PlaceHolder", "Preview"];

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

    let books_plist =
        build_books_plist(&books_identifiers).context("Failed to build Books.plist")?;

    let write_res = (|| -> Result<()> {
        log(&format!(
            "Staging payload archive ({} bytes) via MobileInstallation...",
            archive_data.len()
        ));
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

        log(&format!(
            "Synchronizing {} with AirTraffic host daemon...",
            leaf_name
        ));
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

    log(&format!(
        "Packaging atomic batch of {} file(s) for {}...",
        items.len(),
        target_dir
    ));

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

    log(&format!(
        "Connecting AFC for batch of {} assets...",
        items.len()
    ));
    let session = ActiveDeviceSession::open(Some(udid), connection_mode)
        .context("Failed to open device session for writing")?;
    log(&format!("Connected over {}.", session.transport.label()));
    let afc = AfcClient::new(&session).context("Failed to open AFC connection")?;

    let snapshot = snapshot_books(&afc).context("Failed to snapshot Books state before staging")?;

    let archive_data = build_streaming_zip_archive_multi(target_dir, items)
        .context("Failed to build multi-payload streaming zip archive")?;

    let books_plist =
        build_books_plist(&books_identifiers).context("Failed to build Books.plist for batch")?;

    let write_res = (|| -> Result<()> {
        log(&format!(
            "Staging multi-payload archive ({} bytes, {} files) via MobileInstallation...",
            archive_data.len(),
            items.len()
        ));
        stage_streaming_zip(&session, &source, &archive_data)
            .context("Failed to stage streaming zip conduit")?;

        let link_obj = format!("{}/p0/p1/p2/link", source);
        let payload_obj = format!("{}/payload_0", source);
        let fallback_obj = format!("{}/payload", source);
        if !afc.exists(&source)
            || !afc.exists(&link_obj)
            || (!afc.exists(&payload_obj) && !afc.exists(&fallback_obj))
        {
            bail!("StreamingZip completed but staging link/payload object missing on AFC");
        }

        afc.make_directory_recursive("Books/Sync")?;
        afc.write_file("Books/Sync/Books.plist", &books_plist)?;
        if !afc.exists("Books/Sync/Books.plist") {
            bail!("Failed to stage Books/Sync/Books.plist");
        }

        log(&format!(
            "Synchronizing batch ({} items) with AirTraffic host daemon in single session...",
            items.len()
        ));
        let assets_refs: Vec<(&str, &str)> = assets_to_sync
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
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
    log(&format!(
        "Batch injection of {} file(s) completed successfully!",
        items.len()
    ));

    Ok(())
}

pub fn flash_wallet_skin<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    skin_png_3x: &[u8],
    skin_png_2x: &[u8],
    skin_pdf: &[u8],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", card_hash);

    log(&format!("Target Card Hash: {}", card_hash));
    log(&format!(
        "Skin payload size: @3x {} bytes, @2x {} bytes, PDF {} bytes",
        skin_png_3x.len(),
        skin_png_2x.len(),
        skin_pdf.len()
    ));

    let total_steps = 3;
    progress(
        1,
        total_steps,
        "Writing card artwork assets (@3x, @2x, .pdf)...",
    );
    log("[1/3] Writing card artwork assets (@3x.png, @2x.png, cardBackgroundCombined.pdf)...");

    // #--- CORRECT @2X / @3X WALLET WRITE START ---
    let card_assets: [(&str, &[u8]); 3] = [
        ("cardBackgroundCombined@3x.png", skin_png_3x),
        ("cardBackgroundCombined@2x.png", skin_png_2x),
        ("cardBackgroundCombined.pdf", skin_pdf),
    ];
    // #--- CORRECT @2X / @3X WALLET WRITE END ---

    if let Err(err) =
        write_system_files_batch(udid, connection_mode, &pkpass_dir, &card_assets, &mut log)
    {
        log(&format!(
            "Notice: Batch write failed ({}), trying individual asset writes...",
            err
        ));
        for (asset, data) in &card_assets {
            write_system_file(udid, connection_mode, &pkpass_dir, asset, data, &mut log)
                .context(format!("Failed to write card asset {}", asset))?;
        }
    }

    let cache_leaves: [(&str, &[u8]); 3] = [
        ("FrontFace", b"corrupted"),
        ("PlaceHolder", b"corrupted"),
        ("Preview", b"corrupted"),
    ];

    for (c_idx, ext) in [".cache", ".pkcache"].iter().enumerate() {
        let step = 2 + c_idx;
        let cache_dir = format!("/var/mobile/Library/Passes/Cards/{}{}", card_hash, ext);
        progress(step, total_steps, &format!("Clearing {} cache...", ext));
        log(&format!(
            "[{}/{}] Invalidating cache leaves in {}...",
            step, total_steps, cache_dir
        ));

        if write_system_files_batch(udid, connection_mode, &cache_dir, &cache_leaves, &mut log)
            .is_err()
        {
            for (leaf, data) in &cache_leaves {
                let _ = write_system_file(udid, connection_mode, &cache_dir, leaf, data, &mut log);
            }
        }
    }

    progress(total_steps, total_steps, "Card skin updated successfully!");
    log("Card skin write finished! Close and reopen Wallet on iPhone to view.");
    Ok(())
}

// #--- ORIGINAL WALLET PDF RESTORE START ---
pub fn restore_wallet_pdf<F, L>(
    udid: &str,
    connection_mode: ConnectionMode,
    card_hash: &str,
    pdf_bytes: &[u8],
    mut progress: F,
    mut log: L,
) -> Result<()>
where
    F: FnMut(usize, usize, &str),
    L: FnMut(&str),
{
    if !pdf_bytes.starts_with(b"%PDF-") {
        bail!("Selected file is not a valid PDF");
    }
    let pkpass_dir = format!("/var/mobile/Library/Passes/Cards/{}.pkpass", card_hash);
    let total_steps = 3;

    progress(
        1,
        total_steps,
        "Restoring original cardBackgroundCombined.pdf...",
    );
    log(&format!("Target Card Hash: {}", card_hash));
    log(&format!(
        "Original PDF payload size: {} bytes",
        pdf_bytes.len()
    ));
    log("[1/3] Restoring original cardBackgroundCombined.pdf only...");
    write_system_file(
        udid,
        connection_mode,
        &pkpass_dir,
        "cardBackgroundCombined.pdf",
        pdf_bytes,
        &mut log,
    )
    .context("Failed to restore original cardBackgroundCombined.pdf")?;

    let cache_leaves: [(&str, &[u8]); 3] = [
        ("FrontFace", b"corrupted"),
        ("PlaceHolder", b"corrupted"),
        ("Preview", b"corrupted"),
    ];

    for (idx, ext) in [".cache", ".pkcache"].iter().enumerate() {
        let step = idx + 2;
        let cache_dir = format!("/var/mobile/Library/Passes/Cards/{}{}", card_hash, ext);
        progress(step, total_steps, &format!("Invalidating {} cache...", ext));
        log(&format!(
            "[{}/{}] Invalidating {}...",
            step, total_steps, cache_dir
        ));
        if let Err(err) =
            write_system_files_batch(udid, connection_mode, &cache_dir, &cache_leaves, &mut log)
        {
            log(&format!(
                "Cache batch invalidation failed: {}. Trying individually...",
                err
            ));
            for (leaf, data) in &cache_leaves {
                let _ = write_system_file(udid, connection_mode, &cache_dir, leaf, data, &mut log);
            }
        }
    }
    progress(
        total_steps,
        total_steps,
        "Original Wallet PDF restored successfully!",
    );
    log("Original Wallet PDF restore finished. Force-close Wallet and reopen it.");
    Ok(())
}
// #--- ORIGINAL WALLET PDF RESTORE END ---

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

    let mut dirs_map: std::collections::BTreeMap<String, Vec<(&str, &[u8])>> =
        std::collections::BTreeMap::new();
    for (tdir, leaf, payload) in items {
        dirs_map
            .entry(tdir.clone())
            .or_default()
            .push((leaf.as_str(), payload.as_slice()));
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
            &format!(
                "Flashing {} ({} assets in atomic batch)...",
                tdir_name,
                dir_items.len()
            ),
        );
        log(&format!(
            "Flashing batch of {} assets into {}...",
            dir_items.len(),
            tdir_name
        ));

        let batch_res =
            write_system_files_batch(udid, connection_mode, target_dir, dir_items, &mut log);
        if let Err(err) = batch_res {
            log(&format!(
                "Warning: Batch write failed ({}), falling back to file-by-file write...",
                err
            ));
            for (f_idx, (leaf, payload)) in dir_items.iter().enumerate() {
                progress(
                    f_idx + 1,
                    dir_items.len(),
                    &format!(
                        "Fallback [{}/{}]: writing {}...",
                        f_idx + 1,
                        dir_items.len(),
                        leaf
                    ),
                );
                write_system_file(udid, connection_mode, target_dir, leaf, payload, &mut log)
                    .context(format!("Failed to write button asset {}", leaf))?;
            }
        }
    }

    progress(
        total_dirs,
        total_dirs,
        "Passcode theme applied successfully!",
    );
    log("Passcode theme successfully written! Lock or reboot iPhone to see new keypad.");
    Ok(())
}
