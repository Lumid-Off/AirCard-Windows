# AirCard (Windows) 🎴

> **Apple Wallet Card Skinner & Lockscreen Passcode Themer for iOS 18+ (No Jailbreak Required)**  
> Native Windows client written in Rust. Powered by the `airlift` AirTraffic sync exploit.

---

## Features
- 🎨 **Custom Card Skins:** Assign custom artwork, textures, or bank logos to Apple Pay and Apple Cash cards.
- 🔢 **Lock Screen Passcode Themes (.passthm):** Apply custom keypad button artwork from popular Cowabunga & Nugget `.passthm` themes directly to iOS lockscreen.
- ⚡ **100% Native & Lightweight:** Single standalone `aircard.exe` (~7.5 MB). No Python, no Flet, no webview, no bloated runtimes.
- 🪟 **Material Design 3 Interface:** Clean, modern dark theme built with `egui` and `eframe`.
- 📱 **Zero-Hassle Card Detection:** Tap any card in your iPhone's Wallet app while connected to detect its hash in real-time via `syslog_relay`.
- 📶 **USB & WiFi Transport:** Scan card events and apply Wallet or passcode assets through USB or a paired local WiFi connection.
- 🌐 **English / Simplified Chinese UI:** Switch the interface language from the top bar; the selection is saved locally for future launches.
- 🔄 **Safe & Reversible:** Complete Books state snapshot and automatic restore engine — preserves original device state and backs up the original Wallet card face before replacing it.
- 🚀 **Zero Jailbreak:** Utilizes Apple's built-in AirTraffic sync conduit without modifying system partitions or disabling security.

---

## Requirements
- **Windows 10 / 11 (64-bit)**
- **Apple Mobile Device Support / 64-bit iTunes** (required for Apple device communication).
- A Lightning or USB-C cable for the initial trust/pairing setup.
- For WiFi mode, enable WiFi sync and keep the PC and iPhone on the same local network.

---

## ⚠️ Troubleshooting & Driver Repair (If Nothing Works)

> [!TIP]
> **iPhone not detected, AirTraffic sync hangs, or operation fails?**  
> Corrupted or conflicting Apple USB drivers on Windows are the #1 root cause.
> 1. Download and install **[3uTools](https://www.3u.com/)**.
> 2. **Disconnect your iPhone** from your PC.
> 3. In 3uTools, go to **Toolbox ➔ Repair Driver**.
> 4. Click **Repair Now** and wait for the Apple driver reinstallation to finish.
> 5. Reconnect your unlocked iPhone, tap **Trust**, and launch **AirCard**.

---

## Installation

### Pre-built Executable
1. Download **`aircard.exe`** from [Releases](https://github.com/Lumid-Off/AirCard-Windows/releases).
2. Connect your iPhone via USB, unlock it, and tap **"Trust this Computer"** if prompted.
3. Run **`aircard.exe`**. After WiFi sync is enabled, later sessions can work without the cable.

---

## WiFi Connection Setup
1. Connect the iPhone by USB for the initial pairing.
2. In Apple Devices or iTunes, enable **Show this iPhone when on Wi-Fi** / **Sync with this iPhone over Wi-Fi**.
3. Apply the setting, then keep the iPhone and PC on the same local network.
4. In AirCard, click **Refresh** and confirm the device shows a **WiFi** transport.
5. Disconnect the cable, click **Refresh** again, and select **WiFi only**. Use **Auto (USB preferred)** when automatic fallback is desired.

If both transports are available, **Auto** uses USB first and falls back to WiFi. For a guaranteed end-to-end WiFi route, disconnect the USB cable, click **Refresh**, and then choose **WiFi only**. This is required because Apple's AirTraffic API selects its route by UDID rather than accepting a transport parameter.

---

## How to Customize Apple Wallet Cards
1. Connect your iPhone through USB or paired WiFi and ensure it is unlocked.
2. In AirCard, stay on the **Wallet** tab and click **Scan**.
3. On your iPhone:
   - Open **Apple Wallet** (or double-click the Side/Power button).
   - Tap the card you want to customize.
   - AirCard intercepts and saves the card hash, stops scanning, and loads the current artwork into **Wallet Preview**.
4. Click **Save Current Card PNG...** to save the artwork read from the iPhone as a `.png` file. PDF-only cards use their embedded preview image and are converted to `1536 × 969` PNG.
5. Click **Choose Image...** to pick replacement artwork (PNG, JPG, or WebP), then drag inside the preview to position its crop.
6. Use **Reset to Original** to discard the selected replacement and show the current on-device artwork again.
7. Click **Apply Card Skin**. The first apply also attempts to store a local safety backup for **Restore Original**.
8. Force-close the **Wallet** app on your iPhone from the App Switcher (swipe up from bottom, then swipe Wallet away) and reopen Wallet to see your new card!

---

## How to Apply Lockscreen Passcode Themes (.passthm)
1. Switch to the **Passcode** tab in AirCard.
2. Click **Choose .passthm...** and select any `.passthm` package (Cowabunga or Nugget).
3. Select your target iOS version cache:
   - **Auto (TelephonyUI-10)** — iOS 18+ (Default)
   - **TelephonyUI-9** — iOS 16 - 17
   - **TelephonyUI-8** — Legacy iOS
4. Click **Apply Passcode Theme**.
5. Lock your iPhone screen or open Phone dialer to see your new custom passcode keypad buttons!

> [!IMPORTANT]
> **Turn OFF Bold Text:**  
> On your iPhone, go to **Settings ➔ Display & Brightness** and make sure **Bold Text** is turned **OFF**. If Bold Text is enabled, iOS ignores cached dialer button graphics and renders system vector fonts instead.

---

## Building from Source

Prerequisites: [Rust toolchain](https://rustup.rs/) (`stable-x86_64-pc-windows-msvc`).

```powershell
# Clone the repository
git clone https://github.com/Lumid-Off/AirCard-Windows.git
cd AirCard-Windows

# Run tests
cargo test

# Build release binary
cargo build --release
```

The compiled binary will be in `target\release\aircard.exe`.

---

## Contributors
- **[@Lumid-Off](https://github.com/Lumid-Off)** (Windows Native Rust Port & Maintainer) — [GitHub](https://github.com/Lumid-Off) · [Twitter / X](https://x.com/LumidOff)
- **[@mak5er](https://github.com/mak5er)** (Original macOS App & Exploit Research) — [GitHub](https://github.com/mak5er) · [Twitter / X](https://x.com/mak5er)
- **[AirLift](https://github.com/0xjohnnydev/airlift)** by **[0xjohnny (@0xjohnnydev)](https://github.com/0xjohnnydev)**: Original AirTraffic/ATAirlock sandbox escape and proof of concept underlying `AirliftFFI`.

## Credits
- Core exploit based on `airlift` (AirTraffic sync escape).
- Theme format inspired by [Cowabunga](https://github.com/leminlimez/Cowabunga) and [Nugget](https://github.com/leminlimez/Nugget).

## Support

If you find AirCard useful, you can support future development:

- **TON**: `UQB5jbOhep98IvgKjCIsJ1hHGRh2iWkacAZh9jW2DMIVSWZm`
- **USDT (TRC20)**: `TLiVnkPZ7mVKwCD9RfE28uVTFk6sD6rux2`
- **USDT / BNB (BEP20)**: `0x8EA94e79e47FafBCE10E65342D875c86d1019541`

