# AirCard (Windows) 🎴

> **Apple Wallet Card Skinner & Lockscreen Passcode Themer for iOS 18+ (No Jailbreak Required)**  
> Native Windows client written in Rust. Powered by the `airlift` AirTraffic sync exploit.

---

## Compatibility

| iOS Version | Status | Notes |
| :--- | :--- | :--- |
| **iOS 18.0 – 27.0.1** | ✅ Supported | Full support for Wallet skins and Lockscreen passcode themes |
| **iOS 27.2 beta 1 – beta 2** | ✅ Supported | Working |
| **iOS 27.2 beta 3+** | ❌ Patched | Apple patched the underlying `airlift` exploit. Flashing will not work. |

---

> [!IMPORTANT]
> Do not update to **iOS 27.2 beta 3 or newer** if you want to continue using AirCard. The underlying AirTraffic sync exploit was patched by Apple in beta 3.
> 


## Features

- 🎨 **Custom Card Skins:** Import PNG, JPG, or WebP artwork, drag the preview to adjust the crop, and apply it to Apple Pay or Apple Cash cards.
- 🔢 **Lock Screen Passcode Themes (.passthm):** Apply custom keypad button artwork from popular Cowabunga & Nugget `.passthm` themes directly to iOS lockscreen.
- 📱 **Card Detection:** Scan while tapping a card in Wallet to capture and save its hash automatically.
- 🩺 **Environment Check:** Check Apple components, background service, connection, and iPhone pairing. See what needs attention and the next step to take.
- 🛠️ **One-click Setup:** Repair the Apple service or components, verify and open Apple's official installer when needed, and guide USB trust setup from the app.
- 📶 **USB & WiFi Transport:** Scan card events and apply Wallet or passcode assets through USB or a paired local WiFi connection.
- 🌐 **English / Simplified Chinese UI:** Switch the interface language from the status bar; the selection is saved locally for future launches.
- 🔄 **Original Card Restore:** Save available original card artwork locally before replacement. Use **Restore Original** when a backup exists.
- ⚡ **Native Windows App:** A standalone Rust executable with no Python or webview runtime.
- 🪟 **Material Design 3 Interface:** Dark theme with animated check progress, concise status summaries, and expandable error details.
- 🚀 **Zero Jailbreak:** Utilizes Apple's built-in AirTraffic sync conduit without modifying system partitions or disabling security.

---

## Requirements

- **Windows 10 / 11 (64-bit)**
- **Apple Mobile Device Support / 64-bit desktop iTunes** for device communication; use **Environment → One-click setup** to install or repair it.
- A Lightning or USB-C cable for the initial trust/pairing setup.
- For WiFi mode, enable WiFi sync and keep the PC and iPhone on the same local network.
- Internet access when setup needs to download the Apple installer.

---

## Environment setup & troubleshooting

Open **Environment** to check your PC and iPhone connection. AirCard opens this page automatically when Apple components cannot load.

1. Connect your iPhone by USB and keep it unlocked for the initial setup.
2. Review the results and click **One-click setup** to run the recommended action below.
3. Follow any restart prompt, then click **Check again**. Recheck after changing the selected iPhone or connection mode.

| Setup action | What it does |
| :--- | :--- |
| **Fix service** | Starts or restarts Apple Mobile Device Service, enables automatic startup, and checks again. |
| **Install / repair Apple support** | Repairs installed Apple components. When needed, downloads the official 64-bit iTunes installer, verifies its Apple signature, and opens it. |
| **Set up connection** | Opens iTunes, or installs it if missing, so you can complete USB trust on the iPhone. |

> [!IMPORTANT]
> Approve the Windows administrator prompt when requested and complete any installer steps. **Trust this Computer** must be confirmed on the iPhone. Close iTunes before applying artwork. After component repair, restart AirCard; restart Windows if prompted.

If a check still fails, expand **Technical details** for the original error and use **Copy report** to share the results and setup outcome. **Advanced help** provides the component repair button, service controls, and folder shortcuts. Skipped checks show **Not checked**. Passing these checks confirms connection prerequisites; verify the applied artwork on your iPhone afterward.

<details>
<summary>Technical checks and sync timeout diagnosis</summary>

Checks cover Apple DLL loading and exports, Windows service status, usbmuxd, the service's **ReadBUID** response, selected USB/WiFi transport, and native pairing/session authentication with a `com.apple.atc` probe. A separate process limits checks to 20 seconds; device operations pause while checks or setup are running.

If ReadBUID fails, check the Apple service's Log On account and access to `%ProgramData%\Apple\Lockdown`. Access problems can cause `0xE8000003` or AirTraffic timeouts even when the iPhone is listed. Setup preserves pairing records and directory permissions. Reports exclude pairing certificates and private keys.

</details>

---

## Installation

### Pre-built Executable

1. Download **`aircard.exe`** from [Releases](https://github.com/Lumid-Off/AirCard-Windows/releases).
2. Run **`aircard.exe`** and open **Environment**. Use **One-click setup** if a check needs attention.
3. Connect your iPhone via USB, unlock it, and tap **"Trust this Computer"** if prompted. Close iTunes and check again before applying artwork.
4. To use AirCard without the cable, complete the WiFi setup below.

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
   - AirCard intercepts and saves the card hash automatically. Click **Stop**.
4. Click **Choose Image...** to pick PNG, JPG, or WebP artwork. Drag inside the preview to position the crop; AirCard scales it to `1536 × 969` automatically.
5. Click **Apply Card Skin**.
6. Force-close the **Wallet** app on your iPhone from the App Switcher (swipe up from bottom, then swipe Wallet away) and reopen Wallet to see your new card!
7. AirCard backs up available original artwork before replacing it. **Restore Original** is available when a local backup exists; if no original artwork is found, flashing can continue without a restore backup.

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

