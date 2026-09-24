# AirCard Windows Community changelog

## v9

### Card Designer
- Reworked the designer around a real layer model: Background + Image/Logo + Text + Shape.
- Only the selected layer is draggable; overlapping objects can be selected precisely from the Layers panel.
- Added explicit z-order controls: bring forward, send backward, bring to front, send to back.
- Added visibility, lock, duplicate and delete controls per layer.
- Background crop supports drag-to-pan, mouse-wheel zoom and numeric sliders.
- Image/Logo layers support independent position, scale, rotation, opacity, flat-background keying and auto-contrast outline.
- Added Rectangle / Rounded Rectangle / Circle image masks, including adjustable corner radius.
- Added editable text layers with size, bold, color, position, rotation and opacity.
- Added Rectangle / Rounded Rectangle / Circle shape layers with fill, stroke, dimensions and corner radius.
- Reference image remains preview-only and is never exported.
- Designer drafts now persist layer types, order and properties.

### Wallet output
- Keeps correct artwork assets: @3x = 1536×969 and @2x = 1024×646.
- PDF is generated from the same final composed card design.
- Keeps Original Wallet PDF restore for transit-card recovery.

### Localization
- Expanded JSON locale architecture to 18 language choices:
  en, zh-TW, zh-CN, ja, ko, es, pt-BR, fr, de, ru, id, vi, th, tr, it, pl, hi, ar.
- Missing translated keys fall back to English.
- Locale JSON files can be overridden next to the executable without recompiling.

### Public GitHub cleanup
- Removed GIF/APNG animated-card experiments.
- Removed first-frame animation diagnostics.
- Removed full .pkpass inspector/export research UI.
- Preserved Saved Cards, Passcode themes, Logs, USB/Wi-Fi transport and PDF recovery.

## v8
- Added the first native crop/layout designer.
- Added correct @2x / @3x generation.
- Added zh-TW / English JSON i18n and persistent settings.
