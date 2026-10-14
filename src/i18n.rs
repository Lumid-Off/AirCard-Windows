use std::borrow::Cow;
use std::path::Path;

use anyhow::{Context, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Language {
    English,
    SimplifiedChinese,
}

impl Language {
    pub fn load() -> Self {
        let Some(root) = std::env::var_os("LOCALAPPDATA") else {
            return Self::English;
        };
        let path = std::path::PathBuf::from(root)
            .join("AirCard")
            .join("settings.json");
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|settings| serde_json::from_value(settings.get("language")?.clone()).ok())
            .unwrap_or_else(|| {
                let locale = std::env::var("LANG")
                    .or_else(|_| std::env::var("LANGUAGE"))
                    .or_else(|_| std::env::var("LC_ALL"))
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if locale.starts_with("zh") {
                    Self::SimplifiedChinese
                } else {
                    Self::English
                }
            })
    }

    pub fn save(self) -> Result<()> {
        let root = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?;
        self.save_at(
            &std::path::PathBuf::from(root)
                .join("AirCard")
                .join("settings.json"),
        )
    }

    fn save_at(self, path: &Path) -> Result<()> {
        // Preserve any other preferences already stored by an earlier build.
        let mut settings = match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes)
                    .context("Cannot read existing preferences")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
            Err(error) => return Err(error.into()),
        };
        settings.insert("language".into(), serde_json::to_value(self)?);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&settings)?)?;
        Ok(())
    }

    pub fn option_label(self, option: Self) -> &'static str {
        match (self, option) {
            (Self::English, Self::English) => "English",
            (Self::English, Self::SimplifiedChinese) => "Simplified Chinese",
            (Self::SimplifiedChinese, Self::English) => "英语",
            (Self::SimplifiedChinese, Self::SimplifiedChinese) => "简体中文",
        }
    }

    pub fn choose<'a>(self, english: &'a str, chinese: &'a str) -> &'a str {
        if self == Self::SimplifiedChinese {
            chinese
        } else {
            english
        }
    }

    pub fn text<'a>(self, value: &'a str) -> Cow<'a, str> {
        if self == Self::English {
            return Cow::Borrowed(value);
        }
        let translated = match value {
            "Wallet" => "钱包",
            "Passcode" => "密码主题",
            "Environment" => "环境检测",
            "Help" => "帮助",
            "Refresh" => "刷新",
            "Ready" => "就绪",
            "Unavailable" => "未就绪",
            "No device" => "未连接设备",
            "Select..." => "请选择…",
            "Auto (USB preferred)" => "自动（优先 USB）",
            "USB only" => "仅 USB",
            "WiFi only" => "仅 WiFi",
            "Logs" => "日志",
            "Logs [x]" => "关闭日志",
            "Copy Logs" => "复制日志",
            "Save to File..." => "保存文件…",
            "Clear" => "清空",
            "No events logged yet." => "暂无日志。",
            "Card Configuration" => "卡面设置",
            "Target your card and choose replacement artwork" => "选择卡片，导入替换图片",
            "Target Card Hash" => "卡片 Hash",
            "Base64 pass hash..." => "输入卡片 Hash…",
            "Scan" => "扫描",
            "Stop" => "停止",
            "Saved cards" => "已保存卡片",
            "Card Skin Artwork" => "卡面图片",
            "PNG, JPG, WebP - auto-scaled to 1536x969" => "PNG / JPG / WebP · 自动缩放至 1536×969",
            "Choose Image..." => "选择图片…",
            "Export PNG" => "导出 PNG",
            "Write to iPhone" => "应用到 iPhone",
            "Apply Card Skin" => "应用卡面",
            "Wallet Preview" => "卡面预览",
            "1536 x 969 px pass canvas" => "1536 × 969 px",
            "No artwork loaded" => "尚未导入图片",
            "1.585 ratio" => "1.585 比例",
            "No image" => "无图片",
            "After applying, force close Apple Wallet and reopen it." => {
                "应用后，强制关闭 Wallet 并重新打开。"
            }
            "Scanning syslog..." => "正在扫描…",
            "Open Wallet on iPhone and tap your card" => "在 iPhone 上打开 Wallet 并点击卡片",
            "Passcode Theme" => "密码键盘主题",
            "Custom lockscreen keypad from Cowabunga or Nugget" => {
                "导入 Cowabunga / Nugget 锁屏键盘主题"
            }
            "Theme Package" => "主题文件",
            "Choose a .passthm archive containing dialer artwork" => {
                "选择包含键盘图片的 .passthm 文件"
            }
            "Choose .passthm..." => "选择 .passthm…",
            "Target iOS Cache" => "iOS 缓存版本",
            "Select cache format based on connected iOS version" => "按 iPhone 的 iOS 版本选择",
            "Auto (TelephonyUI-10)" => "自动（TelephonyUI-10）",
            "TelephonyUI-8 (Legacy)" => "TelephonyUI-8（旧版）",
            "Keypad Language" => "键盘字母布局",
            "Subtext alphabet layout (English, Russian, Ukrainian, Japanese, or Universal)" => {
                "英文 / 俄文 / 乌克兰文 / 日文 / 通用"
            }
            "English" => "英文",
            "Russian" => "俄文",
            "Ukrainian" => "乌克兰文",
            "Japanese" => "日文",
            "All Languages (Universal)" => "通用",
            "Bold Text (iOS Accessibility)" => "粗体文本（iOS 辅助功能）",
            "Generates *-bold.png for devices with Bold Text turned ON in iPhone Settings -> Display" => {
                "iPhone 开启“粗体文本”时，生成对应的粗体图片"
            }
            "Apply Passcode Theme" => "应用密码主题",
            "Keypad Preview" => "键盘预览",
            "Dialer button artwork" => "拨号键盘图片",
            "No theme loaded" => "尚未导入主题",
            "No theme" => "无主题",
            "3x4 Keypad" => "3×4 键盘",
            "After applying, lock your iPhone to see the new keypad." => {
                "应用后，锁定 iPhone 查看新键盘。"
            }
            "Setup & Card Hash Guide" => "连接与卡片指南",
            "Everything you need to connect and capture your card" => "连接设备，获取卡片 Hash",
            "Prerequisites" => "使用前准备",
            "- 64-bit iTunes or Apple Mobile Device Support installed" => {
                "• 安装 64 位 iTunes / Apple Mobile Device Support"
            }
            "- First-time setup: connect by USB and tap \"Trust this Computer\"" => {
                "• 首次使用：通过 USB 连接并点击“信任此电脑”"
            }
            "- WiFi: enable WiFi sync, then use the same local network" => {
                "• WiFi：开启无线同步，保持同一局域网"
            }
            "- Select Auto, USB only, or WiFi only in the top bar" => {
                "• 在顶部选择自动 / USB / WiFi 连接模式"
            }
            "Finding Your Card Hash" => "获取卡片 Hash",
            "1. Click \"Scan\" in the Wallet tab" => "1. 在钱包页面点击“扫描”",
            "2. Open Apple Wallet on your iPhone" => "2. 在 iPhone 上打开 Wallet",
            "3. Tap the card you want to customize" => "3. 点击要更换卡面的卡片",
            "4. AirCard captures the pass hash automatically" => "4. AirCard 自动捕获卡片 Hash",
            "5. Click \"Stop\" once detected" => "5. 捕获后点击“停止”",
            "Activation & Theme Guide" => "应用与主题指南",
            "Applying skins and dialer keypad packages" => "应用卡面与密码键盘主题",
            "Activating Apple Wallet Skin" => "应用卡面",
            "1. Click \"Apply Card Skin\" and wait for completion" => "1. 点击“应用卡面”，等待完成",
            "2. Open App Switcher on iPhone (swipe up from bottom)" => {
                "2. 从屏幕底部上滑，打开 App 切换器"
            }
            "3. Force close Apple Wallet by swiping up on it" => "3. 向上滑动 Wallet，强制关闭",
            "4. Reopen Wallet - your new skin appears!" => "4. 重新打开 Wallet 查看卡面",
            "Passcode Themes (.passthm)" => "密码键盘主题（.passthm）",
            "- Compatible with Cowabunga & Nugget theme packages" => {
                "• 支持 Cowabunga / Nugget 主题包"
            }
            "- iOS 18+: Select \"Auto (TelephonyUI-10)\"" => {
                "• iOS 18+：选择“自动（TelephonyUI-10）”"
            }
            "- iOS 16-17: Select \"TelephonyUI-9\"" => "• iOS 16–17：选择 TelephonyUI-9",
            "- Lock screen to verify your updated keypad artwork" => "• 锁屏后检查键盘效果",
            "Ready. Connect iPhone via USB or paired WiFi and unlock it." => {
                "就绪。请连接并解锁 iPhone。"
            }
            "No iPhone connected via USB or paired WiFi." => "未连接 iPhone。",
            "Please select a connected iPhone." => "请选择已连接的 iPhone。",
            "Please enter or scan a target card hash." => "请输入或扫描卡片 Hash。",
            "Please choose a card skin image first." => "请先选择卡面图片。",
            "Syslog scanning stopped." => "扫描已停止。",
            "Scanning syslog... Open Wallet or tap your card on iPhone." => {
                "正在扫描，请在 iPhone 上打开 Wallet 并点击卡片。"
            }
            "Writing card skin to iPhone..." => "正在应用卡面…",
            "Writing passcode theme buttons..." => "正在应用密码键盘主题…",
            "Please select a .passthm theme file first." => "请先选择 .passthm 主题。",
            "Syslog scan finished" => "扫描完成",
            "Card skin successfully flashed! Force quit Wallet on iPhone and reopen it." => {
                "卡面应用完成，请强制关闭 Wallet 后重新打开。"
            }
            "Passcode theme applied! Lock your iPhone to view the new keypad." => {
                "密码主题应用完成，锁屏查看新键盘。"
            }
            "Disconnect the USB cable and refresh to guarantee the full AirTraffic path uses WiFi." => {
                "请拔掉 USB 并刷新，以使用完整的 WiFi 同步路径。"
            }
            "Checking the environment... Keep the iPhone unlocked." => {
                "正在检测，请保持 iPhone 解锁。"
            }
            "Environment report copied to clipboard." => "诊断报告已复制。",
            "Services opened. Find Apple Mobile Device Service." => {
                "服务管理器已打开，请找到 Apple Mobile Device Service。"
            }
            "Error: Environment check stopped unexpectedly." => "错误：环境检测意外停止。",
            "Check again" => "重新检测",
            "One-click setup" => "一键配置",
            "Fix service" => "修复服务",
            "Install / repair Apple support" => "安装 / 修复 Apple 组件",
            "Set up connection" => "配置连接",
            "Restart AirCard" => "重启 AirCard",
            "Restart Windows" => "请重启 Windows",
            "Configuring environment" => "正在配置环境",
            "Configuring environment… Complete the Windows prompt if shown." => {
                "正在配置，请完成 Windows 弹出的授权或安装步骤。"
            }
            "Starts or restarts the Apple service and enables automatic startup. Windows will request administrator access." => {
                "启动或重启服务，设为自动启动；Windows 将请求管理员授权。"
            }
            "Repairs installed Apple components. If missing, downloads and opens Apple's signed installer. Windows will request administrator access." => {
                "修复已有组件；缺少时下载并打开 Apple 官方安装器。Windows 将请求管理员授权。"
            }
            "Opens iTunes, or installs it if missing. Connect USB and tap Trust on iPhone. Close iTunes, then check again." => {
                "打开 iTunes；缺少时下载安装。USB 连接并解锁手机，点击“信任”；关闭 iTunes 后重新检测。"
            }
            "Service configured. Checking again…" => "服务已配置，正在重新检测…",
            "Apple setup finished. Restart AirCard to reload the components. Restart Windows if the installer requests it." => {
                "Apple 配置完成，请重启 AirCard；若安装器要求，请重启 Windows。"
            }
            "Apple setup requires a Windows restart. Restart Windows, then reopen AirCard." => {
                "Apple 配置需要重启 Windows，请重启电脑后打开 AirCard。"
            }
            "iTunes opened. Connect USB, unlock iPhone and tap Trust. Close iTunes, then check again." => {
                "已打开 iTunes。USB 连接并解锁手机，点击“信任”；关闭 iTunes 后重新检测。"
            }
            "Administrator request was cancelled. Click the setup button to try again." => {
                "已取消管理员授权，可点击配置按钮重试。"
            }
            "Apple installation was cancelled. Click the setup button to try again." => {
                "已取消 Apple 安装，可点击配置按钮重试。"
            }
            "Apple download failed. Check your network and try again." => {
                "Apple 下载失败，请检查网络后重试。"
            }
            "Apple installer signature could not be verified. The installer was not run." => {
                "Apple 安装器签名验证失败，已停止安装。"
            }
            "Apple component repair failed or needs installation media. Use the Apple installer and check again." => {
                "Apple 修复失败或缺少安装源，请使用官方安装器后重新检测。"
            }
            "Apple service could not start. Repair Apple components and check again." => {
                "Apple 服务启动失败，请修复组件后重新检测。"
            }
            "Environment setup stopped unexpectedly. Check again and copy the report." => {
                "环境配置意外停止，请重新检测并复制报告。"
            }
            "Copy report" => "复制报告",
            "Open Services" => "服务管理",
            "Apple setup guide" => "Apple 安装指南",
            "Open Apple folder" => "组件目录",
            "Open Lockdown folder" => "配对目录",
            "Copy command" => "复制命令",
            "Environment check" => "环境检测",
            "Connection essentials, at a glance." => "连接准备，一目了然。",
            "Checks" => "检查项目",
            "Next step" => "下一步",
            "Quick setup" => "快速配置",
            "Advanced help" => "高级排查",
            "Technical details" => "技术详情",
            "Passed" => "通过",
            "Needs attention" => "待处理",
            "Failed" => "失败",
            "Not checked" => "未检查",
            "Checking" => "检测中",
            "Waiting" => "等待中",
            "Checking your setup" => "正在检测环境",
            "Connect your iPhone" => "连接 iPhone",
            "Setup needs attention" => "需要配置",
            "Environment ready" => "环境已就绪",
            "Check this connection again" => "请重新检测当前连接",
            "Finish the current operation first." => "请先结束当前扫描或操作。",
            "Keep iPhone unlocked. Maximum 20 seconds." => "保持 iPhone 解锁，最长 20 秒。",
            "Selection changed. Check again." => "设备或连接模式已变更，请重新检测。",
            "Apple components" => "Apple 组件",
            "Background service" => "后台服务",
            "Device channel" => "设备通道",
            "Pairing storage" => "配对访问",
            "iPhone connection" => "iPhone 连接",
            "Sync service" => "同步服务",
            "Apple libraries loaded" => "Apple 组件已加载",
            "Apple components unavailable" => "缺少或无法加载 Apple 组件",
            "Service is running" => "服务正在运行",
            "Service is stopped or missing" => "服务未运行或未安装",
            "Service state is unknown" => "无法读取服务状态",
            "usbmuxd is responding" => "设备通道可访问",
            "Device channel unavailable" => "设备通道不可访问",
            "Service identity readable" => "Apple 服务可读取 BUID",
            "Service cannot read BUID" => "Apple 服务无法读取 BUID",
            "Device found on this connection" => "当前连接可用",
            "No device on this connection" => "当前连接未找到设备",
            "Native session and service available" => "原生认证与同步服务可用",
            "Native authentication failed" => "原生认证或同步服务不可用",
            "Check the prerequisites first" => "请先完成前置配置",
            "The check did not finish" => "检测未完成",
            "Check completion" => "检测进程",
            "Keep iPhone unlocked." => "请保持 iPhone 解锁。",
            "Waiting for previous checks" => "等待前置检查",
            "Check timed out. Reconnect and try again." => "检测超时，请重新连接并再试。",
            "Install Apple support" => "安装 Apple 组件",
            "Install 64-bit desktop iTunes." => "安装 64 位桌面版 iTunes。",
            "Connect & trust" => "连接并信任",
            "Connect USB, unlock iPhone, then tap Trust in iTunes." => {
                "USB 连接并解锁 iPhone，在 iTunes 中点击“信任”。"
            }
            "Connect USB, open iTunes, then tap Trust on iPhone." => {
                "USB 连接并打开 iTunes，在 iPhone 上点击“信任”。"
            }
            "Close iTunes" => "关闭 iTunes",
            "Keep iPhone unlocked before applying artwork." => "应用前关闭 iTunes，保持手机解锁。",
            "Use WiFi" => "使用 WiFi",
            "Enable WiFi sync, use the same network, then unplug USB and refresh." => {
                "开启 WiFi 同步，保持同一网络，拔掉 USB 后刷新。"
            }
            "Check the result on iPhone after applying artwork." => {
                "应用后仍需在 iPhone 上确认效果。"
            }
            "No changes to pairing, permissions, or artwork." => "检测不会更改配对、权限或卡面。",
            "Restart in Administrator PowerShell:" => "在管理员 PowerShell 中重启服务：",
            "BUID failures can cause 0xE8000003 or a sync timeout. Check the service Log On account and its Lockdown access." => {
                "BUID 不可读可能导致 0xE8000003 或同步超时。检查服务的登录账户及 Lockdown 访问权限。"
            }
            "Pairing files are in %ProgramData%\\Apple\\Lockdown. Repair Apple support if permissions are wrong." => {
                "配对目录：%ProgramData%\\Apple\\Lockdown。权限异常时修复 Apple 组件。"
            }
            "Install or repair 64-bit desktop iTunes from Apple, then restart AirCard." => {
                "安装或修复 Apple 的 64 位桌面版 iTunes，然后重启 AirCard。"
            }
            "Open Services, start Apple Mobile Device Service, and reconnect the iPhone." => {
                "在服务管理中启动 Apple Mobile Device Service，再重新连接 iPhone。"
            }
            "Check the service Log On account and its access to %ProgramData%\\Apple\\Lockdown. Repair Apple Mobile Device Support if needed. This can cause 0xE8000003 and AirTraffic timeouts." => {
                "检查服务登录账户能否访问 %ProgramData%\\Apple\\Lockdown；必要时修复 Apple 组件。这可能导致 0xE8000003 或同步超时。"
            }
            "Connect USB and open iTunes. Unlock iPhone and tap Trust when prompted. For WiFi, enable WiFi sync on the same network, unplug USB, then refresh." => {
                "USB 连接并打开 iTunes，解锁 iPhone 后点击“信任”。WiFi 需开启无线同步、使用同一网络，拔掉 USB 后刷新。"
            }
            "Keep the iPhone unlocked during device operations." => "操作期间保持 iPhone 解锁。",
            "This checks prerequisites. A completed card or theme transfer still requires verification on the iPhone." => {
                "基础环境通过；应用后仍需在 iPhone 上确认效果。"
            }
            "Keep the iPhone unlocked. Open iTunes and complete Trust this Computer, then close iTunes and run these checks again." => {
                "解锁 iPhone，打开 iTunes 并在手机上完成“信任此电脑”，关闭 iTunes 后重新检测。"
            }
            "Resolve the checks above, then run the checks again." => {
                "先处理前面的异常，再重新检测。"
            }
            "Open Services and start or restart Apple Mobile Device Service." => {
                "在服务管理中启动或重启 Apple Mobile Device Service。"
            }
            "Install or repair 64-bit desktop iTunes from Apple." => {
                "安装或修复 Apple 的 64 位桌面版 iTunes。"
            }
            "Open Services and verify Apple Mobile Device Service manually." => {
                "在服务管理中检查 Apple Mobile Device Service。"
            }
            "Run the checks again after reconnecting the iPhone." => "重新连接 iPhone 后再检测。",
            "Run the checks again." => "请重新检测。",
            "Run the checks again. Copy the report if the problem continues." => {
                "重新检测；若仍失败，请复制报告排查。"
            }
            "connect iPhone" => "连接 iPhone",
            "choose available transport" => "选择可用连接",
            "enter card hash" => "输入卡片 Hash",
            "choose image" => "选择图片",
            "select theme" => "选择主题",
            "Language" => "语言",
            "Simplified Chinese" => "简体中文",
            "Language changed." => "语言已切换。",
            "Transport mode" => "连接方式",
            "entries" => "条记录",
            "Crop position updated." => "裁切位置已更新。",
            "Card skin updated successfully!" => "卡片皮肤更新成功！",
            "Restoring original card face..." => "正在恢复原卡面...",
            "Restoring original card artwork..." => "正在恢复原卡面图片...",
            "Clearing .cache cache..." => "正在清理 .cache 缓存...",
            "Clearing .pkcache cache..." => "正在清理 .pkcache 缓存...",
            "Original card face restored successfully!" => "原卡面恢复成功！",
            "Original card face restored. Force close Wallet and reopen it." => {
                "原卡面已恢复。请强制关闭并重新打开 Wallet。"
            }
            "Original card backup not found." => "未找到原卡面备份。",
            "Apply a card skin once to create an original backup." => {
                "先应用一次卡片皮肤以创建原卡面备份。"
            }
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
            "connection. Refresh devices or change transport mode." => {
                "连接。请刷新设备或更改连接方式。"
            }
            "Drag inside the preview to reposition the crop." => "在预览区域内拖动以调整裁切位置。",
            "Restore Original" => "恢复原卡面",
            "Need: " => "需要：",
            "assets" => "个资源",
            "TelephonyUI-10 (iOS 18+)" => "TelephonyUI-10（iOS 18+）",
            "TelephonyUI-9 (iOS 16-17)" => "TelephonyUI-9（iOS 16-17）",
            _ => return self.dynamic_text(value),
        };
        Cow::Borrowed(translated)
    }

    fn dynamic_text<'a>(self, value: &'a str) -> Cow<'a, str> {
        for (prefix, translated) in [
            ("Error: ", "错误："),
            ("Could not enumerate devices: ", "设备读取失败："),
            ("Could not prepare image: ", "图片处理失败："),
            ("Could not save PNG: ", "图片保存失败："),
            ("Failed to parse theme: ", "主题解析失败："),
            ("Saved prepared PNG: ", "PNG 已保存："),
            ("Transport mode: ", "连接模式："),
            ("Could not save language: ", "语言设置保存失败："),
            ("Could not open folder: ", "目录打开失败："),
            ("Could not open Services: ", "服务管理器打开失败："),
            ("Could not restart AirCard: ", "AirCard 重启失败："),
        ] {
            if let Some(rest) = value.strip_prefix(prefix) {
                return Cow::Owned(format!("{translated}{}", self.text(rest)));
            }
        }
        if let Some(count) = value.strip_suffix(" entries") {
            return Cow::Owned(format!("{count} 条日志"));
        }
        if let Some(rest) = value.strip_prefix("Found ") {
            if let Some((count, mode)) = rest.split_once(" connected device(s); transport mode: ") {
                return Cow::Owned(format!("已连接 {count} 台设备 · {}", self.text(mode)));
            }
        }
        if let Some(rest) = value.strip_prefix("Environment check completed: ") {
            return Cow::Owned(
                format!("检测完成 · {rest}")
                    .replace(" failure(s), ", " 项失败，")
                    .replace(" item(s) need attention.", " 项待处理。"),
            );
        }
        Cow::Borrowed(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saving_language_preserves_existing_preferences() {
        let path =
            std::env::temp_dir().join(format!("aircard-language-{}.json", std::process::id()));
        std::fs::write(&path, br#"{"language":"English","other_preference":17}"#).unwrap();
        Language::SimplifiedChinese.save_at(&path).unwrap();
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(settings["language"], "SimplifiedChinese");
        assert_eq!(settings["other_preference"], 17);
        std::fs::remove_file(path).unwrap();
    }
}
