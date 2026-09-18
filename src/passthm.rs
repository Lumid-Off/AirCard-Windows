use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub const KEYPAD_SUBTEXTS: &[(&str, &str)] = &[
    ("0", "+"),
    ("1", ""),
    ("2", "A B C"),
    ("3", "D E F"),
    ("4", "G H I"),
    ("5", "J K L"),
    ("6", "M N O"),
    ("7", "P Q R S"),
    ("8", "T U V"),
    ("9", "W X Y Z"),
];

#[derive(Debug, Clone)]
pub struct PasscodeTheme {
    pub name: String,
    pub detected_version: String,
    pub items: Vec<(String, String, Vec<u8>)>, // (target_dir, leaf_name, data)
    pub key_previews: HashMap<String, Vec<u8>>, // digit -> image bytes
}

pub fn parse_passthm_file(
    file_path: &Path,
    forced_version: Option<&str>,
) -> Result<PasscodeTheme> {
    let file = File::open(file_path).context("Failed to open passcode theme file")?;
    let mut zip = ZipArchive::new(file).context("Failed to read theme file as zip archive")?;

    let name = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("CustomTheme")
        .to_string();

    let mut detected_version = "TelephonyUI-10".to_string();
    let mut image_entries = Vec::new();

    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let entry_name = entry.name().to_string();

        if entry.is_dir()
            || entry_name.starts_with("__MACOSX")
            || Path::new(&entry_name)
                .file_name()
                .map(|f| f.to_string_lossy().starts_with('.'))
                .unwrap_or(false)
        {
            continue;
        }

        let low = entry_name.to_lowercase();
        if low.contains("telephonyui-8") || low.contains("telephony-8") {
            detected_version = "TelephonyUI-8".to_string();
        } else if low.contains("telephonyui-9") || low.contains("telephony-9") {
            detected_version = "TelephonyUI-9".to_string();
        }

        if low.ends_with(".png") || low.ends_with(".jpg") || low.ends_with(".jpeg") {
            image_entries.push(entry_name);
        }
    }

    // Default target version is TelephonyUI-10 for modern iOS (iOS 16, 17, 18+)
    let primary_target_version = forced_version.unwrap_or("TelephonyUI-10");

    let mut items_dict: HashMap<String, Vec<u8>> = HashMap::new();
    let mut key_previews = HashMap::new();

    let digit_re = Regex::new(r"(?:^[a-zA-Z]+-)?([0-9*#])(?:-([^-\n]+))?").unwrap();
    let simple_digit_re = Regex::new(r"([0-9*#])").unwrap();
    let white_strip_re = Regex::new(r"(?i)--?white$").unwrap();

    let subtext_map: HashMap<&str, &str> = KEYPAD_SUBTEXTS.iter().copied().collect();

    for entry_name in image_entries {
        let leaf = Path::new(&entry_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&entry_name)
            .to_string();

        if leaf.starts_with('.') || leaf.starts_with('_') || (!leaf.ends_with(".png") && !leaf.ends_with(".jpg") && !leaf.ends_with(".jpeg")) {
            continue;
        }

        let mut entry = zip.by_name(&entry_name)?;
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

        items_dict.insert(leaf.clone(), data.clone());

        let stem = Path::new(&leaf)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&leaf);
        let stem_clean = white_strip_re.replace(stem, "").to_string();

        let mut digit: Option<String> = None;
        let mut subtext: Option<String> = None;

        if let Some(caps) = digit_re.captures(&stem_clean) {
            if let Some(d) = caps.get(1) {
                digit = Some(d.as_str().to_string());
            }
            if let Some(s) = caps.get(2) {
                subtext = Some(s.as_str().trim().to_string());
            }
        }

        if digit.is_none() {
            if let Some(caps) = simple_digit_re.captures(&leaf) {
                if let Some(d) = caps.get(1) {
                    digit = Some(d.as_str().to_string());
                }
            }
        }

        if let Some(d) = digit {
            if !key_previews.contains_key(&d) {
                key_previews.insert(d.clone(), data.clone());
            }

            if let Some(ref s) = subtext {
                if !s.is_empty() {
                    items_dict.insert(format!("en-{}-{}--white.png", d, s), data.clone());
                    items_dict.insert(format!("other-{}-{}--white.png", d, s), data.clone());
                }
            }
            items_dict.insert(format!("en-{}---white.png", d), data.clone());
            items_dict.insert(format!("other-{}---white.png", d), data.clone());

            if let Some(&std_sub) = subtext_map.get(d.as_str()) {
                if !std_sub.is_empty() {
                    items_dict.insert(format!("en-{}-{}--white.png", d, std_sub), data.clone());
                    items_dict.insert(format!("other-{}-{}--white.png", d, std_sub), data.clone());
                }
            }

            if leaf.starts_with("en-") {
                items_dict.insert(format!("other{}", &leaf[2..]), data.clone());
            } else if leaf.starts_with("other-") {
                items_dict.insert(format!("en{}", &leaf[5..]), data.clone());
            }
        }
    }

    if items_dict.is_empty() {
        bail!("No valid keypad image assets found in passcode theme archive");
    }

    let mut target_dirs = vec![format!("/var/mobile/Library/Caches/{}", primary_target_version)];
    if forced_version.is_none() && detected_version != primary_target_version {
        target_dirs.push(format!("/var/mobile/Library/Caches/{}", detected_version));
    }

    let mut items = Vec::new();
    for tdir in &target_dirs {
        for (leaf, data) in &items_dict {
            items.push((tdir.clone(), leaf.clone(), data.clone()));
        }
    }

    Ok(PasscodeTheme {
        name,
        detected_version,
        items,
        key_previews,
    })
}

#[allow(dead_code)]
pub fn export_passthm_archive(
    output_path: &Path,
    images: &[(String, Vec<u8>)], // (filename, png_bytes)
) -> Result<()> {
    let file = File::create(output_path).context("Failed to create .passthm file")?;
    let mut zip = ZipWriter::new(file);

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for (name, data) in images {
        zip.start_file(name, options)?;
        zip.write_all(data)?;
    }

    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_synthetic_passthm() {
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join("test_synthetic.passthm");

        // Create a synthetic .passthm zip
        {
            let file = File::create(&test_path).expect("failed to create temp test file");
            let mut zip = ZipWriter::new(file);
            let options = SimpleFileOptions::default();

            // Add dummy digit 0 and 2
            zip.start_file("en-0---white.png", options).unwrap();
            zip.write_all(b"\x89PNG\r\n\x1a\nfake0").unwrap();

            zip.start_file("en-2-A B C--white.png", options).unwrap();
            zip.write_all(b"\x89PNG\r\n\x1a\nfake2").unwrap();

            zip.finish().unwrap();
        }

        let theme = parse_passthm_file(&test_path, None).expect("failed to parse synthetic theme");
        assert_eq!(theme.name, "test_synthetic");
        assert_eq!(theme.detected_version, "TelephonyUI-10");
        assert!(theme.key_previews.contains_key("0"));
        assert!(theme.key_previews.contains_key("2"));

        let leaf_names: Vec<&str> = theme.items.iter().map(|(_, leaf, _)| leaf.as_str()).collect();
        assert!(leaf_names.contains(&"en-0---white.png"));
        assert!(leaf_names.contains(&"en-2-A B C--white.png"));

        let _ = std::fs::remove_file(test_path);
    }
}
