use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use regex::Regex;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

pub const KEYPAD_SUBTEXTS_EN: &[(&str, &str)] = &[
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

pub const KEYPAD_SUBTEXTS_RU: &[(&str, &str)] = &[
    ("0", "+"),
    ("1", ""),
    ("2", "А Б В Г"),
    ("3", "Д Е Ж З"),
    ("4", "И Й К Л"),
    ("5", "М Н О П"),
    ("6", "Р С Т У"),
    ("7", "Ф Х Ц Ч"),
    ("8", "Ш Щ Ъ Ы"),
    ("9", "Ь Э Ю Я"),
];

pub const KEYPAD_SUBTEXTS_UK: &[(&str, &str)] = &[
    ("0", "+"),
    ("1", ""),
    ("2", "А Б В Г Ґ"),
    ("3", "Д Е Є Ж З"),
    ("4", "И І Ї Й"),
    ("5", "К Л М Н"),
    ("6", "О П Р С"),
    ("7", "Т У Ф Х"),
    ("8", "Ц Ч Ш Щ"),
    ("9", "Ь Ю Я"),
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
    language: &str,
    bold: bool,
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
    let white_strip_re = Regex::new(r"(?i)--?white(?:-bold)?$").unwrap();

    let ru_map: HashMap<&str, &str> = KEYPAD_SUBTEXTS_RU.iter().copied().collect();
    let en_map: HashMap<&str, &str> = KEYPAD_SUBTEXTS_EN.iter().copied().collect();
    let uk_map: HashMap<&str, &str> = KEYPAD_SUBTEXTS_UK.iter().copied().collect();

    let is_ru = language.contains("Russian") || language.contains("ru");
    let is_uk = language.contains("Ukrainian") || language.contains("uk");
    let is_ja = language.contains("Japanese") || language.contains("ja");
    let is_en = language.contains("English") || language.contains("en");
    let is_all = language.contains("All") || language.contains("Universal");

    let bold_suffix = if bold { "-bold" } else { "" };

    for entry_name in image_entries {
        let leaf = Path::new(&entry_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&entry_name)
            .to_string();

        if leaf.starts_with('.')
            || leaf.starts_with('_')
            || (!leaf.ends_with(".png") && !leaf.ends_with(".jpg") && !leaf.ends_with(".jpeg"))
        {
            continue;
        }

        let mut entry = zip.by_name(&entry_name)?;
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;

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

            let ru_sub = ru_map.get(d.as_str()).copied().unwrap_or("");
            let en_sub = en_map.get(d.as_str()).copied().unwrap_or("");
            let uk_sub = uk_map.get(d.as_str()).copied().unwrap_or("");

            let mut add_variant = |prefix: &str, sub: &str| {
                if sub.is_empty() {
                    items_dict.insert(
                        format!("{}-{}---white{}.png", prefix, d, bold_suffix),
                        data.clone(),
                    );
                } else {
                    items_dict.insert(
                        format!("{}-{}-{}--white{}.png", prefix, d, sub, bold_suffix),
                        data.clone(),
                    );
                    let nospace = sub.replace(' ', "");
                    if nospace != sub {
                        items_dict.insert(
                            format!("{}-{}-{}--white{}.png", prefix, d, nospace, bold_suffix),
                            data.clone(),
                        );
                    }
                }
            };

            if is_ru {
                for p in &["ru", "other", "en"] {
                    add_variant(p, "");
                    if !ru_sub.is_empty() {
                        add_variant(p, ru_sub);
                    }
                    if !en_sub.is_empty() {
                        add_variant(p, en_sub);
                    }
                }
            } else if is_uk {
                for p in &["uk", "other", "en"] {
                    add_variant(p, "");
                    if !uk_sub.is_empty() {
                        add_variant(p, uk_sub);
                    }
                    if !en_sub.is_empty() {
                        add_variant(p, en_sub);
                    }
                }
            } else if is_ja {
                for p in &["ja", "other", "en"] {
                    add_variant(p, "");
                    if !en_sub.is_empty() {
                        add_variant(p, en_sub);
                    }
                }
            } else if is_en && !is_all {
                for p in &["en", "other"] {
                    add_variant(p, "");
                    if !en_sub.is_empty() {
                        add_variant(p, en_sub);
                    }
                }
            } else if is_all {
                for p in &[
                    "en", "other", "ru", "uk", "ja", "es", "fr", "de", "it", "pt", "tr", "pl",
                    "ko", "zh",
                ] {
                    add_variant(p, "");
                    if *p == "ru" && !ru_sub.is_empty() {
                        add_variant(p, ru_sub);
                    } else if *p == "uk" && !uk_sub.is_empty() {
                        add_variant(p, uk_sub);
                    }
                    if !en_sub.is_empty() {
                        add_variant(p, en_sub);
                    }
                }
            }

            if let Some(ref s) = subtext {
                if !s.is_empty() {
                    for p in &["en", "other", "ru", "uk", "ja"] {
                        add_variant(p, s);
                    }
                }
            }
        }
    }

    if items_dict.is_empty() {
        bail!("No valid keypad image assets found in passcode theme archive");
    }

    let target_dirs = vec![format!(
        "/var/mobile/Library/Caches/{}",
        primary_target_version
    )];

    let mut items = Vec::new();
    for tdir in &target_dirs {
        // High-DPI cache marker for modern iOS (16, 17, 18+)
        items.push((tdir.clone(), "_big".to_string(), Vec::new()));
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

        let theme = parse_passthm_file(&test_path, None, "Russian (Русский)", false)
            .expect("failed to parse synthetic theme");
        assert_eq!(theme.name, "test_synthetic");
        assert_eq!(theme.detected_version, "TelephonyUI-10");
        assert!(theme.key_previews.contains_key("0"));
        assert!(theme.key_previews.contains_key("2"));

        let leaf_names: Vec<&str> = theme
            .items
            .iter()
            .map(|(_, leaf, _)| leaf.as_str())
            .collect();
        assert!(leaf_names.contains(&"_big"));
        assert!(leaf_names.contains(&"ru-0---white.png"));
        assert!(leaf_names.contains(&"ru-2-А Б В Г--white.png"));
        assert!(leaf_names.contains(&"en-2-A B C--white.png"));

        // Test bold mode
        let theme_bold = parse_passthm_file(&test_path, None, "Japanese (日本語)", true)
            .expect("failed to parse synthetic bold theme");
        let bold_leaves: Vec<&str> = theme_bold
            .items
            .iter()
            .map(|(_, leaf, _)| leaf.as_str())
            .collect();
        assert!(bold_leaves.contains(&"_big"));
        assert!(bold_leaves.contains(&"ja-0---white-bold.png"));
        assert!(bold_leaves.contains(&"ja-2-A B C--white-bold.png"));

        let _ = std::fs::remove_file(test_path);
    }
}
