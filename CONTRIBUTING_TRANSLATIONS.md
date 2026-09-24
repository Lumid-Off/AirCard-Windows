# Translation contributions

Community v9 uses JSON catalogs under `locales/`.

## Add or improve a language

1. Copy `locales/en.json`.
2. Keep every JSON key exactly the same.
3. Translate string values only.
4. Preserve placeholders such as `{count}`.
5. Save UTF-8 JSON.
6. If adding a new locale, also add its code and display name to `LANGUAGES` in `src/i18n.rs` and add an `include_str!` entry there.

At runtime, AirCard falls back to English for any missing key, so partial translation PRs are safe. Complete catalogs are preferred for release builds.
