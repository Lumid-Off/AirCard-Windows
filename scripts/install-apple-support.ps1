$ErrorActionPreference = "Stop"

$appleDirs = @(
    "C:\Program Files\Common Files\Apple\Mobile Device Support",
    "C:\Program Files (x86)\Common Files\Apple\Mobile Device Support"
)

$dlls = @(
    "CoreFoundation.dll",
    "MobileDevice.dll",
    "AirTrafficHost.dll"
)

function Find-AppleSupportDir {
    foreach ($dir in $appleDirs) {
        if (Test-Path $dir) {
            return $dir
        }
    }
    return $appleDirs[0]
}

function Test-AppleRuntime {
    $dir = Find-AppleSupportDir
    $allOk = $true

    Write-Host ""
    Write-Host "=== Apple Mobile Device Support Check ===" -ForegroundColor Cyan
    Write-Host "Apple Support path: $dir" -ForegroundColor DarkGray

    foreach ($dll in $dlls) {
        $path = Join-Path $dir $dll
        $ok = Test-Path $path

        if ($ok) {
            Write-Host "[PASS] $dll" -ForegroundColor Green
        } else {
            Write-Host "[FAIL] $dll" -ForegroundColor Red
            $allOk = $false
        }
    }

    Write-Host ""
    return $allOk
}

Write-Host "============================================" -ForegroundColor Cyan
Write-Host " AirCard Apple Support - One Click Setup" -ForegroundColor Cyan
Write-Host " AirCard Apple 支援 - 一鍵安裝與檢查" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan

if (Test-AppleRuntime) {
    Write-Host "[READY] Apple Mobile Device Support is ready." -ForegroundColor Green
    Write-Host "[完成] Apple Mobile Device Support 已可使用。" -ForegroundColor Green
    Write-Host ""
    Write-Host "Next / 下一步：" -ForegroundColor Yellow
    Write-Host "1. Connect iPhone by USB / USB 連接 iPhone"
    Write-Host "2. Unlock iPhone / 解鎖 iPhone"
    Write-Host "3. Tap Trust / 按下「信任」"
    Write-Host "4. Start aircard.exe / 開啟 aircard.exe"
    exit 0
}

Write-Host "[INFO] Apple runtime is incomplete. Downloading official 64-bit iTunes..." -ForegroundColor Yellow
Write-Host "[資訊] Apple 元件不完整，開始下載 Apple 官方 64-bit iTunes..." -ForegroundColor Yellow

$installer = Join-Path $env:TEMP "iTunes64Setup.exe"
$url = "https://www.apple.com/itunes/download/win64"

try {
    Invoke-WebRequest $url -OutFile $installer
    Write-Host "[OK] Download complete: $installer" -ForegroundColor Green
    Write-Host "[OK] 下載完成：$installer" -ForegroundColor Green
} catch {
    Write-Host "[ERROR] Download failed / 下載失敗" -ForegroundColor Red
    Write-Host $_.Exception.Message -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "[INFO] Starting iTunes installer. Keep the default options." -ForegroundColor Yellow
Write-Host "[資訊] 正在開啟 iTunes 安裝程式，保持預設選項完成安裝即可。" -ForegroundColor Yellow

$proc = Start-Process $installer -Verb RunAs -Wait -PassThru

Write-Host ""
Write-Host "[INFO] Installer closed. Checking required DLL files..." -ForegroundColor Cyan
Write-Host "[資訊] 安裝程式已結束，開始檢查 AirCard 所需 DLL..." -ForegroundColor Cyan

Start-Sleep -Seconds 2

if (Test-AppleRuntime) {
    $service = Get-Service -ErrorAction SilentlyContinue | Where-Object {
        $_.DisplayName -like "*Apple Mobile Device*"
    } | Select-Object -First 1

    if ($service) {
        Write-Host "[PASS] Apple Mobile Device service: $($service.Status)" -ForegroundColor Green
    }

    Write-Host ""
    Write-Host "============================================" -ForegroundColor Green
    Write-Host " READY / 安裝檢查完成" -ForegroundColor Green
    Write-Host "============================================" -ForegroundColor Green
    Write-Host "1. USB connect your iPhone / USB 連接 iPhone"
    Write-Host "2. Unlock it / 解鎖"
    Write-Host "3. Tap Trust / 按「信任」"
    Write-Host "4. Run aircard.exe / 執行 aircard.exe"
    exit 0
}

Write-Host ""
Write-Host "============================================" -ForegroundColor Red
Write-Host " NOT READY / 尚未完成" -ForegroundColor Red
Write-Host "============================================" -ForegroundColor Red
Write-Host "One or more required DLL files are still missing." -ForegroundColor Red
Write-Host "仍有 AirCard 所需 DLL 找不到。" -ForegroundColor Red
Write-Host ""
Write-Host "Please restart Windows, then run this script again." -ForegroundColor Yellow
Write-Host "請重新啟動 Windows，之後再執行一次此腳本。" -ForegroundColor Yellow
exit 1
