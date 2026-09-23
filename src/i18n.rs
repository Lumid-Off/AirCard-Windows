//! Fluent-based, platform-neutral UI localisation.
//!
//! Translation resources live outside Rust source in `locales/*.ftl`. New
//! languages need a catalogue file and a language option; UI code never
//! contains translated display text.

use fluent_bundle::{FluentBundle, FluentResource};
use serde::{Deserialize, Serialize};
use unic_langid::LanguageIdentifier;

const EN_US: &str = include_str!("../locales/en-US.ftl");
const JA_JP: &str = include_str!("../locales/ja-JP.ftl");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AppLanguage {
    #[default]
    System,
    English,
    Japanese,
}

impl AppLanguage {
    pub const ALL: [Self; 3] = [Self::System, Self::English, Self::Japanese];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::English => "English",
            Self::Japanese => "日本語",
        }
    }

    fn locale_id(self) -> &'static str {
        match self {
            Self::Japanese => "ja-JP",
            Self::English => "en-US",
            Self::System if system_locale().as_deref().is_some_and(|name| name.to_ascii_lowercase().starts_with("ja")) => "ja-JP",
            Self::System => "en-US",
        }
    }
}

pub struct Localizer {
    bundle: FluentBundle<FluentResource>,
}

impl Localizer {
    pub fn new(language: AppLanguage) -> Self {
        let locale_id = language.locale_id();
        let langid: LanguageIdentifier = locale_id.parse().expect("supported locale identifier");
        let mut bundle = FluentBundle::new(vec![langid]);
        let source = if locale_id == "ja-JP" { JA_JP } else { EN_US };
        let resource = FluentResource::try_new(source.to_owned()).expect("valid bundled Fluent resource");
        bundle.add_resource(resource).expect("add bundled Fluent resource");
        Self { bundle }
    }

    pub fn text(&self, key: &str) -> String {
        let Some(message) = self.bundle.get_message(key) else {
            return format!("⟪{key}⟫");
        };
        let Some(pattern) = message.value() else {
            return format!("⟪{key}⟫");
        };
        let mut errors = Vec::new();
        self.bundle.format_pattern(pattern, None, &mut errors).into_owned()
    }
}

fn system_locale() -> Option<String> {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetUserDefaultLocaleName(locale_name: *mut u16, cch_locale_name: i32) -> i32;
        }
        let mut buffer = [0_u16; 85];
        let length = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
        return (length > 1)
            .then(|| String::from_utf16(&buffer[..length as usize - 1]).ok())
            .flatten();
    }

    #[cfg(not(windows))]
    {
        std::env::var("LANG").ok().or_else(|| std::env::var("LANGUAGE").ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_catalogue_loads() {
        assert_eq!(Localizer::new(AppLanguage::Japanese).text("tab-settings"), "設定");
    }

    #[test]
    fn missing_key_is_visible_to_developers() {
        assert_eq!(Localizer::new(AppLanguage::English).text("missing-key"), "⟪missing-key⟫");
    }
}
