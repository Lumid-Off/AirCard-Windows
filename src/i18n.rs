use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

// #--- V9 JSON I18N START ---
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("zh-TW", "繁體中文"),
    ("zh-CN", "简体中文"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("es", "Español"),
    ("pt-BR", "Português (Brasil)"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("ru", "Русский"),
    ("id", "Bahasa Indonesia"),
    ("vi", "Tiếng Việt"),
    ("th", "ไทย"),
    ("tr", "Türkçe"),
    ("it", "Italiano"),
    ("pl", "Polski"),
    ("hi", "हिन्दी"),
    ("ar", "العربية"),
];

pub struct I18n {
    language: String,
    builtins: HashMap<String, HashMap<String, String>>,
    overrides: HashMap<String, HashMap<String, String>>,
}

impl I18n {
    pub fn new(language: &str) -> Self {
        let mut builtins = HashMap::new();
        builtins.insert(
            "en".into(),
            parse_catalog(include_str!("../locales/en.json")),
        );
        builtins.insert(
            "zh-TW".into(),
            parse_catalog(include_str!("../locales/zh-TW.json")),
        );
        builtins.insert(
            "zh-CN".into(),
            parse_catalog(include_str!("../locales/zh-CN.json")),
        );
        builtins.insert(
            "ja".into(),
            parse_catalog(include_str!("../locales/ja.json")),
        );
        builtins.insert(
            "ko".into(),
            parse_catalog(include_str!("../locales/ko.json")),
        );
        builtins.insert(
            "es".into(),
            parse_catalog(include_str!("../locales/es.json")),
        );
        builtins.insert(
            "pt-BR".into(),
            parse_catalog(include_str!("../locales/pt-BR.json")),
        );
        builtins.insert(
            "fr".into(),
            parse_catalog(include_str!("../locales/fr.json")),
        );
        builtins.insert(
            "de".into(),
            parse_catalog(include_str!("../locales/de.json")),
        );
        builtins.insert(
            "ru".into(),
            parse_catalog(include_str!("../locales/ru.json")),
        );
        builtins.insert(
            "id".into(),
            parse_catalog(include_str!("../locales/id.json")),
        );
        builtins.insert(
            "vi".into(),
            parse_catalog(include_str!("../locales/vi.json")),
        );
        builtins.insert(
            "th".into(),
            parse_catalog(include_str!("../locales/th.json")),
        );
        builtins.insert(
            "tr".into(),
            parse_catalog(include_str!("../locales/tr.json")),
        );
        builtins.insert(
            "it".into(),
            parse_catalog(include_str!("../locales/it.json")),
        );
        builtins.insert(
            "pl".into(),
            parse_catalog(include_str!("../locales/pl.json")),
        );
        builtins.insert(
            "hi".into(),
            parse_catalog(include_str!("../locales/hi.json")),
        );
        builtins.insert(
            "ar".into(),
            parse_catalog(include_str!("../locales/ar.json")),
        );
        let mut this = Self {
            language: normalize_language(language),
            builtins,
            overrides: HashMap::new(),
        };
        this.load_runtime_overrides();
        this
    }

    pub fn language(&self) -> &str {
        &self.language
    }
    pub fn languages() -> &'static [(&'static str, &'static str)] {
        LANGUAGES
    }

    pub fn language_label(&self) -> &'static str {
        LANGUAGES
            .iter()
            .find(|(code, _)| *code == self.language.as_str())
            .map(|(_, label)| *label)
            .unwrap_or("English")
    }

    pub fn set_language(&mut self, language: &str) {
        self.language = normalize_language(language);
    }

    pub fn t(&self, key: &str) -> String {
        if let Some(value) = self.lookup(&self.language, key) {
            return value.to_string();
        }
        if let Some(value) = self.lookup("en", key) {
            return value.to_string();
        }
        key.to_string()
    }

    fn lookup<'a>(&'a self, lang: &str, key: &str) -> Option<Cow<'a, str>> {
        if let Some(value) = self.overrides.get(lang).and_then(|c| c.get(key)) {
            return Some(Cow::Borrowed(value));
        }
        self.builtins
            .get(lang)
            .and_then(|c| c.get(key))
            .map(|v| Cow::Borrowed(v.as_str()))
    }

    fn load_runtime_overrides(&mut self) {
        let Some(dir) = runtime_locale_dir() else {
            return;
        };
        for (lang, _) in LANGUAGES {
            let path = dir.join(format!("{lang}.json"));
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let catalog = parse_catalog(&text);
            if !catalog.is_empty() {
                self.overrides.insert((*lang).to_string(), catalog);
            }
        }
    }
}

fn normalize_language(language: &str) -> String {
    let raw = language.trim();
    let lower = raw.to_ascii_lowercase().replace('_', "-");
    match lower.as_str() {
        "zh" | "zh-tw" | "zh-hant" | "traditional chinese" => "zh-TW".into(),
        "zh-cn" | "zh-hans" | "simplified chinese" => "zh-CN".into(),
        "pt" | "pt-br" => "pt-BR".into(),
        _ => LANGUAGES
            .iter()
            .find(|(code, _)| code.eq_ignore_ascii_case(raw) || code.to_ascii_lowercase() == lower)
            .map(|(code, _)| (*code).to_string())
            .unwrap_or_else(|| "en".into()),
    }
}

fn runtime_locale_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("locales"))
}

fn parse_catalog(text: &str) -> HashMap<String, String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    flatten_json("", &value, &mut out);
    out
}

fn flatten_json(prefix: &str, value: &Value, out: &mut HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_json(&next, child, out);
            }
        }
        Value::String(text) => {
            out.insert(prefix.to_string(), text.clone());
        }
        _ => {}
    }
}

pub fn write_default_locales_to(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let files = [
        ("en", include_str!("../locales/en.json")),
        ("zh-TW", include_str!("../locales/zh-TW.json")),
        ("zh-CN", include_str!("../locales/zh-CN.json")),
        ("ja", include_str!("../locales/ja.json")),
        ("ko", include_str!("../locales/ko.json")),
        ("es", include_str!("../locales/es.json")),
        ("pt-BR", include_str!("../locales/pt-BR.json")),
        ("fr", include_str!("../locales/fr.json")),
        ("de", include_str!("../locales/de.json")),
        ("ru", include_str!("../locales/ru.json")),
        ("id", include_str!("../locales/id.json")),
        ("vi", include_str!("../locales/vi.json")),
        ("th", include_str!("../locales/th.json")),
        ("tr", include_str!("../locales/tr.json")),
        ("it", include_str!("../locales/it.json")),
        ("pl", include_str!("../locales/pl.json")),
        ("hi", include_str!("../locales/hi.json")),
        ("ar", include_str!("../locales/ar.json")),
    ];
    for (code, data) in files {
        let path = dir.join(format!("{code}.json"));
        if !path.exists() {
            fs::write(path, data)?;
        }
    }
    Ok(())
}

pub fn ensure_runtime_locales() -> std::io::Result<()> {
    let Some(dir) = runtime_locale_dir() else {
        return Ok(());
    };
    write_default_locales_to(&dir)
}
// #--- V9 JSON I18N END ---
