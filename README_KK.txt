AirCard Windows v1.2.2 Community v9 - GitHub-ready overlay
============================================================

這是一個覆蓋到 Lumid-Off/AirCard-Windows v1.2.2 專案根目錄的更新包。
它保留 AirCard 的 Wallet / Passcode / USB-Wi-Fi 功能，加入原生卡面設計器、JSON 多語言、設定保存與交通卡 PDF 救援。

公開版已移除
----------
- GIF / APNG 動態卡面實驗
- 第一幀 PNG-only / PNG+PDF 診斷 UI
- .pkpass 資訊讀取 / 完整匯出研究 UI

這些研究功能不屬於 Community v9 的一般使用者介面。

Community v9 主要功能
-------------------
1. 原生 Layer 卡面設計器
   - 背景圖片可拖曳、Zoom、X/Y 精準調整，固定 Wallet 卡面比例。
   - 每張圖片 / Logo 都是獨立 Layer，只移動目前選取的物件。
   - 圖片 / Logo 可縮放、旋轉、調透明度、去除純色背景、加自動對比外框。
   - 圖片 / Logo 可套 Rectangle / Rounded Rectangle / Circle Mask；圓角半徑可調。
   - 可加入文字 Layer：內容、大小、粗體、顏色、旋轉、透明度、位置皆可獨立調整。
   - 可加入 Rectangle / Rounded Rectangle / Circle Shape；可調尺寸、填色、描邊與圓角。
   - Layers 面板可精準選擇重疊物件，並支援 Bring forward / Send backward / Bring to front / Send to back。
   - 可隱藏、鎖定、複製、刪除各 Layer。
   - 參考圖只顯示在設計預覽，不會輸出到 Wallet 卡面。
   - 可保存 Designer draft；來源圖片檔必須仍存在於原路徑。

2. 正確 Wallet 靜態資產
   - cardBackgroundCombined@3x.png = 1536 × 969
   - cardBackgroundCombined@2x.png = 1024 × 646
   - cardBackgroundCombined.pdf 由同一份最終合成卡面產生。
   - Designer 可直接輸出 1536 × 969 PNG，或交給 Apply Static Artwork 寫入 iPhone。

3. JSON 多語言
   內建語言：
   - English
   - 繁體中文
   - 简体中文
   - 日本語
   - 한국어
   - Español
   - Português (Brasil)
   - Français
   - Deutsch
   - Русский
   - Bahasa Indonesia
   - Tiếng Việt
   - ไทย
   - Türkçe
   - Italiano
   - Polski
   - हिन्दी
   - العربية

   語言檔位於 locales/*.json。
   English 與繁中為完整 catalog；其他語言目前覆蓋主要介面與 Designer，缺少的 key 會安全 fallback 到 English。
   啟動時也會在 exe 同層建立 locales 資料夾，因此翻譯可以不重新編譯直接修改。

4. 設定保存
   - 保存目前語言。
   - 保存最後使用的 Wallet Card Hash。
   - 保存 Designer draft（背景來源、裁切、參考圖、Layer、文字、Shape、Logo 等設定）。
   - 設定位置：%APPDATA%\AirCard\settings.json
   - 原本 Saved Cards 繼續使用原 AirCard 的保存方式。

5. Suica / transit card PDF 救援
   - 保留「🚑 Restore Original PDF / 恢復原廠 PDF」。
   - 只恢復 cardBackgroundCombined.pdf 並刷新 FrontFace / PlaceHolder / Preview cache。
   - 不修改 pass.json、manifest.json、signature 或 *.urls。

安裝 / 覆蓋
-----------
1. 關閉 aircard.exe。
2. 建議先備份你的專案。
3. 將 RAR 內容解壓到：
   C:\Users\KKacobls\Downloads\AirCard-Windows-main\AirCard-Windows-main
4. 選擇覆蓋同名檔案。
5. 在專案根目錄執行：
   cargo build --release
6. 啟動：
   .\target\release\aircard.exe

第一次 build 會因 Community v9 新增 ab_glyph 文字渲染依賴而更新 Cargo.lock，這是正常的。

Designer 基本操作
----------------
Wallet -> Import image / Open Designer

背景：
- 點 Layers 的 Background，或點畫布空白。
- 拖曳畫布：移動裁切位置。
- 滑鼠滾輪：Zoom。
- 右側可用 Zoom / Horizontal / Vertical 精準調整。

Layer：
- 點畫布上的物件，會選取最上方且未 Lock 的 Layer。
- 若重疊不好點，直接在左側 Layers 面板選指定 Layer。
- 只有目前 selected Layer 可以被畫布拖曳。
- 滑鼠滾輪會縮放目前 selected Layer。
- Lock 後仍可從 Layers 面板選取，但不能在畫布拖動。

完成：
- Use design as Wallet artwork
- 回 Wallet 主頁
- Apply Static Artwork
- 完成後強制關閉 Wallet 再開啟。

授權 / Credits
-------------
- Base project: Lumid-Off/AirCard-Windows (MIT)
- Designer workflow inspired by Susie-Meow/card-design-generator (MIT)
詳細授權資訊請見 LICENSE 與 THIRD_PARTY_NOTICES.md。

注意
----
- Community v9 不宣稱 Apple Wallet 支援 GIF/APNG 動畫，因此公開版已移除相關功能。
- Suica / 交通卡可能使用 PDF 作為主要可見卡面；請保留原始 PDF 作為救援備份。
- 此工具修改的是你自己的裝置上的 Wallet 顯示資產；請自行承擔使用與備份責任。
