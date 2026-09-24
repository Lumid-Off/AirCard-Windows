use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::card_designer::DesignerDraft;

// #--- PERSISTENT SETTINGS START ---
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub language: String,
    pub last_card_hash: String,
    pub designer_draft: Option<DesignerDraft>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: "zh-TW".to_string(),
            last_card_hash: String::new(),
            designer_draft: None,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = settings_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(path, data)
    }
}

pub fn settings_path() -> Option<PathBuf> {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(PathBuf::from(appdata).join("AirCard").join("settings.json"));
    }
    std::env::current_dir()
        .ok()
        .map(|p| p.join("aircard-settings.json"))
}
// #--- PERSISTENT SETTINGS END ---
