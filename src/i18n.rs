use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    English,
    SimplifiedChinese,
}

impl Default for Language {
    fn default() -> Self {
        Self::English
    }
}

impl Language {
    pub fn load() -> Self {
        let path = settings_path();
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str::<Settings>(&contents) {
                return settings.language;
            }
        }

        let system_locale = std::env::var("LANG")
            .or_else(|_| std::env::var("LANGUAGE"))
            .or_else(|_| std::env::var("LC_ALL"))
            .unwrap_or_default()
            .to_ascii_lowercase();

        if system_locale.starts_with("zh") {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }

    pub fn save(self) {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let settings = Settings { language: self };
        if let Ok(contents) = serde_json::to_string_pretty(&settings) {
            let _ = fs::write(path, contents);
        }
    }

    pub fn text<'a>(self, source: &'a str) -> &'a str {
        if self == Self::English {
            return source;
        }

        match source {
            "Wallet" => "钱包",
            "Passcode" => "锁屏密码",
            "Help" => "帮助",
            "Refresh" => "刷新",
            "Auto (USB preferred)" => "自动（优先 USB）",
            "USB only" => "仅 USB",
            "WiFi only" => "仅 WiFi",
            "No device" => "没有设备",
            "Ready" => "就绪",
            "Unavailable" => "不可用",
            "Logs" => "日志",
            "Logs [x]" => "日志 [x]",
            "Copy Logs" => "复制日志",
            "Save to File..." => "保存到文件...",
            "Clear" => "清空",
            "No events logged yet." => "暂无日志记录。",
            "Language" => "语言",
            "English" => "英语",
            "Simplified Chinese" => "简体中文",
            "Language changed." => "语言已切换。",
            "Transport mode" => "连接方式",
            "entries" => "条记录",
            "Ready. Connect iPhone via USB or paired WiFi and unlock it." => "就绪。请通过 USB 或已配对的 WiFi 连接并解锁 iPhone。",
            "No iPhone connected via USB or paired WiFi." => "未检测到通过 USB 或已配对 WiFi 连接的 iPhone。",
            "Please select a connected iPhone." => "请选择已连接的 iPhone。",
            "Disconnect the USB cable and refresh to guarantee the full AirTraffic path uses WiFi." => "请断开 USB 线并刷新，以确保完整的 AirTraffic 路径使用 WiFi。",
            "Please enter or scan a target card hash." => "请输入或扫描目标卡片 Hash。",
            "Please choose a card skin image first." => "请先选择卡片皮肤图片。",
            "Crop position updated." => "裁切位置已更新。",
            "Scanning syslog... Open Wallet or tap your card on iPhone." => "正在扫描 syslog... 请在 iPhone 上打开钱包并点击卡片。",
            "Syslog scanning stopped." => "syslog 扫描已停止。",
            "Writing card skin to iPhone..." => "正在将卡片皮肤写入 iPhone...",
            "Please select a .passthm theme file first." => "请先选择一个 .passthm 主题文件。",
            "Writing passcode theme buttons..." => "正在将锁屏密码主题按钮写入 iPhone...",
            "Syslog scan finished" => "syslog 扫描完成",
            "Card skin successfully flashed! Force quit Wallet on iPhone and reopen it." => "卡片皮肤应用成功！请在 iPhone 上强制关闭并重新打开 Wallet。",
            "Passcode theme applied! Lock your iPhone to view the new keypad." => "锁屏密码主题应用成功！锁定 iPhone 查看新的键盘样式。",
            "Card skin updated successfully!" => "卡片皮肤更新成功！",
            "Restoring original card face..." => "正在恢复原卡面...",
            "Restoring original card artwork..." => "正在恢复原卡面图片...",
            "Clearing .cache cache..." => "正在清理 .cache 缓存...",
            "Clearing .pkcache cache..." => "正在清理 .pkcache 缓存...",
            "Original card face restored successfully!" => "原卡面恢复成功！",
            "Original card face restored. Force close Wallet and reopen it." => "原卡面已恢复。请强制关闭并重新打开 Wallet。",
            "Original card backup not found." => "未找到原卡面备份。",
            "Apply a card skin once to create an original backup." => "先应用一次卡片皮肤以创建原卡面备份。",
            "Passcode theme applied successfully!" => "锁屏密码主题应用成功！",
            "Failed to parse theme:" => "解析主题失败：",
            "Card captured" => "已捕获卡片",
            "Loaded" => "已加载",
            "target:" => "目标：",
            "lang:" => "语言：",
            "bold:" => "粗体：",
            "ON" => "开",
            "OFF" => "关",
            "Error: " => "错误：",
            "Found" => "已找到",
            "connected device(s); transport mode:" => "台已连接设备；连接方式：",
            "Could not enumerate devices:" => "无法枚举设备：",
            "Selected iPhone has no" => "选中的 iPhone 没有",
            "connection. Refresh devices or change transport mode." => "连接。请刷新设备或更改连接方式。",
            "Scanning syslog..." => "正在扫描 syslog...",
            "Open Wallet on iPhone and tap your card" => "请在 iPhone 上打开钱包并点击目标卡片",
            "Stop" => "停止",
            "Scan" => "扫描",
            "Card Configuration" => "卡片配置",
            "Target your card and choose replacement artwork" => "选择目标卡片和替换图片",
            "Target Card Hash" => "目标卡片 Hash",
            "Base64 pass hash..." => "Base64 卡片 Hash...",
            "Saved cards" => "已保存的卡片",
            "Select..." => "请选择...",
            "Card Skin Artwork" => "卡片皮肤图片",
            "PNG, JPG, WebP - auto-scaled to 1536x969" => "PNG、JPG、WebP，将自动缩放到 1536x969",
            "Drag inside the preview to reposition the crop." => "在预览区域内拖动以调整裁切位置。",
            "Choose Image..." => "选择图片...",
            "Export PNG" => "导出 PNG",
            "Write to iPhone" => "写入 iPhone",
            "Apply Card Skin" => "应用卡片皮肤",
            "Restore Original" => "恢复原卡面",
            "connect iPhone" => "连接 iPhone",
            "choose available transport" => "选择可用的连接方式",
            "enter card hash" => "输入卡片 Hash",
            "choose image" => "选择图片",
            "select theme" => "选择主题",
            "Need: " => "需要：",
            "Wallet Preview" => "钱包预览",
            "1536 x 969 px pass canvas" => "1536 x 969 像素卡片画布",
            "No artwork loaded" => "尚未加载图片",
            "No image" => "没有图片",
            "After applying, force close Apple Wallet and reopen it." => "应用后，请强制关闭 Apple Wallet 并重新打开。",
            "Passcode Theme" => "锁屏密码主题",
            "Custom lockscreen keypad from Cowabunga or Nugget" => "来自 Cowabunga 或 Nugget 的自定义锁屏键盘",
            "Theme Package" => "主题包",
            "Choose a .passthm archive containing dialer artwork" => "选择包含拨号键盘图片的 .passthm 压缩包",
            "Choose .passthm..." => "选择 .passthm...",
            "assets" => "个资源",
            "Target iOS Cache" => "目标 iOS 缓存",
            "Select cache format based on connected iOS version" => "根据连接设备的 iOS 版本选择缓存格式",
            "Auto (TelephonyUI-10)" => "自动（TelephonyUI-10）",
            "TelephonyUI-10 (iOS 18+)" => "TelephonyUI-10（iOS 18+）",
            "TelephonyUI-9 (iOS 16-17)" => "TelephonyUI-9（iOS 16-17）",
            "TelephonyUI-8 (Legacy)" => "TelephonyUI-8（旧版本）",
            "Keypad Language" => "键盘语言",
            "Subtext alphabet layout (English, Russian, Ukrainian, Japanese, or Universal)" => "按键副文字母布局（英语、俄语、乌克兰语、日语或通用）",
            "Russian" => "俄语",
            "Ukrainian" => "乌克兰语",
            "Japanese" => "日语",
            "All Languages (Universal)" => "全部语言（通用）",
            "Bold Text (iOS Accessibility)" => "粗体文字（iOS 辅助功能）",
            "Generates *-bold.png for devices with Bold Text turned ON in iPhone Settings -> Display" => "为 iPhone 设置 -> 显示与亮度中启用粗体文字的设备生成 *-bold.png",
            "Apply Passcode Theme" => "应用锁屏密码主题",
            "Keypad Preview" => "键盘预览",
            "Dialer button artwork" => "拨号按钮图片",
            "No theme loaded" => "尚未加载主题",
            "3x4 Keypad" => "3x4 键盘",
            "No theme" => "没有主题",
            "After applying, lock your iPhone to see the new keypad." => "应用后，请锁定 iPhone 查看新的键盘样式。",
            "Setup & Card Hash Guide" => "设置与卡片 Hash 指南",
            "Everything you need to connect and capture your card" => "连接设备并捕获卡片所需的全部信息",
            "Prerequisites" => "使用前准备",
            "- 64-bit iTunes or Apple Mobile Device Support installed" => "- 已安装 64 位 iTunes 或 Apple Mobile Device Support",
            "Install 64-bit iTunes / Apple Mobile Device Support, then connect and trust your iPhone." => "安装 64 位 iTunes / Apple Mobile Device Support，然后连接并信任 iPhone。",
            "Connect your iPhone by USB and trust it in Finder and on the phone. Uses macOS Apple frameworks." => "通过 USB 连接 iPhone，并在 Finder 和手机上完成信任。本应用使用 macOS Apple 框架。",
            "Linux supports image and theme previews. Device scanning and applying themes require Windows or macOS; Apple's AirTraffic runtime is unavailable on Linux." => "Linux 支持图片和主题预览。设备扫描与应用主题需要 Windows 或 macOS；Linux 无法使用 Apple AirTraffic 运行环境。",
            "- First-time setup: connect by USB and tap \"Trust this Computer\"" => "- 首次使用：通过 USB 连接并点击“信任此电脑”",
            "- WiFi: enable WiFi sync, then use the same local network" => "- WiFi：启用 WiFi 同步，并确保双方处于同一局域网",
            "- Select Auto, USB only, or WiFi only in the top bar" => "- 在顶部栏选择自动、仅 USB 或仅 WiFi",
            "Finding Your Card Hash" => "查找卡片 Hash",
            "1. Click \"Scan\" in the Wallet tab" => "1. 在钱包页点击“扫描”",
            "2. Open Apple Wallet on your iPhone" => "2. 在 iPhone 上打开 Apple Wallet",
            "3. Tap the card you want to customize" => "3. 点击要自定义的卡片",
            "4. AirCard captures the pass hash automatically" => "4. AirCard 会自动捕获卡片 Hash",
            "5. Click \"Stop\" once detected" => "5. 检测到后点击“停止”",
            "Activation & Theme Guide" => "应用与主题指南",
            "Applying skins and dialer keypad packages" => "应用卡片皮肤和拨号键盘主题",
            "Activating Apple Wallet Skin" => "应用 Apple Wallet 皮肤",
            "1. Click \"Apply Card Skin\" and wait for completion" => "1. 点击“应用卡片皮肤”并等待完成",
            "2. Open App Switcher on iPhone (swipe up from bottom)" => "2. 在 iPhone 上打开 App 切换器（从底部向上滑动）",
            "3. Force close Apple Wallet by swiping up on it" => "3. 向上滑动并强制关闭 Apple Wallet",
            "4. Reopen Wallet - your new skin appears!" => "4. 重新打开钱包，即可看到新的皮肤！",
            "Passcode Themes (.passthm)" => "锁屏密码主题（.passthm）",
            "- Compatible with Cowabunga & Nugget theme packages" => "- 兼容 Cowabunga 和 Nugget 主题包",
            "- iOS 18+: Select \"Auto (TelephonyUI-10)\"" => "- iOS 18+：选择“自动（TelephonyUI-10）”",
            "- iOS 16-17: Select \"TelephonyUI-9\"" => "- iOS 16-17：选择“TelephonyUI-9”",
            "- Lock screen to verify your updated keypad artwork" => "- 锁定屏幕以查看更新后的键盘图片",
            _ => source,
        }
    }

    pub fn option_label(self, option: Self) -> &'static str {
        match (self, option) {
            (Self::English, Self::English) => "English",
            (Self::English, Self::SimplifiedChinese) => "Simplified Chinese",
            (Self::SimplifiedChinese, Self::English) => "英语",
            (Self::SimplifiedChinese, Self::SimplifiedChinese) => "简体中文",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Settings {
    language: Language,
}

fn settings_path() -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| r"C:\Users\Default\AppData\Local".to_string());
    PathBuf::from(local_app_data)
        .join("AirCard")
        .join("settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_translation_has_english_fallback() {
        assert_eq!(Language::SimplifiedChinese.text("Wallet"), "钱包");
        assert_eq!(
            Language::SimplifiedChinese.text("unknown string"),
            "unknown string"
        );
    }

    #[test]
    fn language_option_labels_follow_current_language() {
        assert_eq!(
            Language::English.option_label(Language::SimplifiedChinese),
            "Simplified Chinese"
        );
        assert_eq!(
            Language::SimplifiedChinese.option_label(Language::SimplifiedChinese),
            "简体中文"
        );
    }
}
