use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;

use crate::environment::{APPLE_RUNTIME, CheckStatus, EnvironmentCheck};

pub const APPLE_INSTALL_URL: &str = "https://www.apple.com/itunes/download/win64";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupAction {
    RepairService,
    RepairApple,
    ConnectIphone,
}

impl SetupAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::RepairService => "Fix service",
            Self::RepairApple => "Install / repair Apple support",
            Self::ConnectIphone => "Set up connection",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::RepairService => {
                "Starts or restarts the Apple service and enables automatic startup. Windows will request administrator access."
            }
            Self::RepairApple => {
                "Repairs installed Apple components. If missing, downloads and opens Apple's signed installer. Windows will request administrator access."
            }
            Self::ConnectIphone => {
                "Opens iTunes, or installs it if missing. Connect USB and tap Trust on iPhone. Close iTunes, then check again."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Recheck,
    RestartRequired,
    RebootRequired,
    TrustRequired,
}

pub fn recommended_action(checks: &[EnvironmentCheck]) -> Option<SetupAction> {
    let problem = |title: &str| {
        checks
            .iter()
            .find(|check| check.title == title && check.status != CheckStatus::Pass)
    };
    if problem(APPLE_RUNTIME).is_some() {
        return Some(SetupAction::RepairApple);
    }
    if let Some(service) = problem("Apple Mobile Device Service") {
        return Some(
            if service.detail == "The Windows service is not installed." {
                SetupAction::RepairApple
            } else {
                SetupAction::RepairService
            },
        );
    }
    if problem("Apple USB device channel").is_some() {
        return Some(SetupAction::RepairService);
    }
    if problem("Lockdown service access (BUID)").is_some() {
        return Some(SetupAction::RepairApple);
    }
    if problem("Selected iPhone / transport").is_some()
        || problem("Apple pairing / AirTraffic service")
            .is_some_and(|check| check.status == CheckStatus::Fail)
    {
        return Some(SetupAction::ConnectIphone);
    }
    None
}

pub fn run(action: SetupAction) -> Result<SetupOutcome> {
    if action == SetupAction::ConnectIphone {
        let itunes = ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|root| PathBuf::from(root).join("iTunes").join("iTunes.exe"))
            .find(|path| path.is_file());
        if let Some(itunes) = itunes {
            Command::new(itunes)
                .spawn()
                .context("Could not open iTunes")?;
            return Ok(SetupOutcome::TrustRequired);
        }
        // No setup app is installed: use the same signed Apple installer flow.
        return Ok(
            if run_elevated(&setup_script(SetupAction::ConnectIphone))? {
                SetupOutcome::RebootRequired
            } else {
                SetupOutcome::RestartRequired
            },
        );
    }
    let reboot_required = run_elevated(&setup_script(action))?;
    Ok(if reboot_required {
        SetupOutcome::RebootRequired
    } else if action == SetupAction::RepairApple {
        SetupOutcome::RestartRequired
    } else {
        SetupOutcome::Recheck
    })
}

fn setup_script(action: SetupAction) -> String {
    let repair = if action != SetupAction::RepairService {
        REPAIR_APPLE.replace("{{APPLE_INSTALL_URL}}", APPLE_INSTALL_URL)
    } else {
        String::new()
    };
    let require_itunes = if action == SetupAction::ConnectIphone {
        "$true"
    } else {
        "$false"
    };
    // Only fixed scripts are elevated. No device IDs, error messages, or shell
    // strings from the diagnostic report are interpolated into these commands.
    format!(
        "$ErrorActionPreference = 'Stop'\n$reboot = $false\n$requireItunes = {require_itunes}\ntry {{\n{repair}\nif ($reboot) {{ exit 30 }}\n{REPAIR_SERVICE}\nexit 0\n}} catch {{ exit 20 }}"
    )
}

const REPAIR_APPLE: &str = r#"
$roots = @('HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*')
$packages = @(Get-ItemProperty -Path $roots -ErrorAction SilentlyContinue | Where-Object {
    $_.DisplayName -in @('Apple Mobile Device Support', 'Apple Application Support (64-bit)', 'Apple Application Support (32-bit)', 'iTunes') -and
    $_.Publisher -eq 'Apple Inc.' -and $_.WindowsInstaller -eq 1 -and
    $_.PSChildName -match '^\{[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}\}$'
} | Sort-Object PSChildName -Unique)
$needsInstaller = $false
if ($packages.Count -gt 0 -and -not $requireItunes) {
    foreach ($package in $packages) {
        $installer = Start-Process -FilePath "$env:SystemRoot\System32\msiexec.exe" -ArgumentList @('/fa', $package.PSChildName, '/passive', '/norestart') -WindowStyle Hidden -Wait -PassThru
        if ($installer.ExitCode -eq 1602) { exit 21 }
        if ($installer.ExitCode -notin @(0, 3010, 1641)) { $needsInstaller = $true; break }
        if ($installer.ExitCode -in @(3010, 1641)) { $reboot = $true }
    }
}
$support = "$env:ProgramFiles\Common Files\Apple\Mobile Device Support"
$ready = (Test-Path "$support\MobileDevice.dll") -and (Test-Path "$support\CoreFoundation.dll") -and (Test-Path "$support\AirTrafficHost.dll") -and (Get-Service -Name 'Apple Mobile Device Service' -ErrorAction SilentlyContinue)
if (-not $ready -or $needsInstaller -or ($requireItunes -and -not (Test-Path "$env:ProgramFiles\iTunes\iTunes.exe"))) {
    $folder = Join-Path $env:TEMP ('AirCard-Apple-' + [guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $folder | Out-Null
    $setup = Join-Path $folder 'iTunes64Setup.exe'
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -UseBasicParsing -Uri '{{APPLE_INSTALL_URL}}' -OutFile $setup -TimeoutSec 180
    } catch { exit 22 }
    $signature = Get-AuthenticodeSignature -FilePath $setup
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch '(?:^|,\s*)O=Apple Inc\.(?:,|$)') { exit 23 }
    $installer = Start-Process -FilePath $setup -Wait -PassThru
    if ($installer.ExitCode -notin @(0, 3010, 1641)) { exit 24 }
    if ($installer.ExitCode -in @(3010, 1641)) { $reboot = $true }
    if (-not (Test-Path "$support\MobileDevice.dll") -or -not (Test-Path "$support\CoreFoundation.dll") -or -not (Test-Path "$support\AirTrafficHost.dll")) { exit 24 }
    if ($requireItunes -and -not (Test-Path "$env:ProgramFiles\iTunes\iTunes.exe")) { exit 24 }
}
"#;

const REPAIR_SERVICE: &str = r#"
try {
    $service = Get-Service -Name 'Apple Mobile Device Service'
    Set-Service -Name $service.Name -StartupType Automatic
    if ($service.Status -eq 'Running') {
        Restart-Service -Name $service.Name -Force
    } else {
        Start-Service -Name $service.Name
    }
    $service.Refresh()
    $service.WaitForStatus([System.ServiceProcess.ServiceControllerStatus]::Running, [TimeSpan]::FromSeconds(30))
} catch { exit 25 }
"#;

fn run_elevated(script: &str) -> Result<bool> {
    #[repr(C)]
    struct ShellExecuteInfo {
        size: u32,
        mask: u32,
        window: *mut std::ffi::c_void,
        verb: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
        instance: *mut std::ffi::c_void,
        id_list: *mut std::ffi::c_void,
        class: *const u16,
        class_key: *mut std::ffi::c_void,
        hot_key: u32,
        icon_or_monitor: *mut std::ffi::c_void,
        process: *mut std::ffi::c_void,
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WaitForSingleObject(handle: *mut std::ffi::c_void, milliseconds: u32) -> u32;
        fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    let wide = |value: &OsStr| value.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let system_root = std::env::var_os("SystemRoot").context("SystemRoot is missing")?;
    let powershell =
        PathBuf::from(system_root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let file = wide(powershell.as_os_str());
    let parameters = wide(OsStr::new(&format!(
        "-NoProfile -NonInteractive -EncodedCommand {encoded}"
    )));
    let verb = wide(OsStr::new("runas"));
    let mut info: ShellExecuteInfo = unsafe { std::mem::zeroed() };
    info.size = std::mem::size_of::<ShellExecuteInfo>() as u32;
    info.mask = 0x40 | 0x100; // SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC
    info.verb = verb.as_ptr();
    info.file = file.as_ptr();
    info.parameters = parameters.as_ptr();
    info.show = 0;
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(1223) {
                bail!("Administrator request was cancelled. Click the setup button to try again.");
            }
            return Err(error).context("Could not start environment setup");
        }
        ensure!(!info.process.is_null(), "Setup process unavailable");
        // Wait on a background thread; never interrupt Windows Installer midway.
        let wait = WaitForSingleObject(info.process, u32::MAX);
        let mut code = 0;
        let got_code = GetExitCodeProcess(info.process, &mut code);
        CloseHandle(info.process);
        ensure!(wait == 0 && got_code != 0, "Could not read setup result");
        match code {
            0 => Ok(false),
            30 => Ok(true),
            21 => bail!("Apple installation was cancelled. Click the setup button to try again."),
            22 => bail!("Apple download failed. Check your network and try again."),
            23 => {
                bail!("Apple installer signature could not be verified. The installer was not run.")
            }
            24 => bail!(
                "Apple component repair failed or needs installation media. Use the Apple installer and check again."
            ),
            25 => bail!("Apple service could not start. Repair Apple components and check again."),
            _ => bail!(
                "Environment setup failed (exit code {code}). Check again and copy the report."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_component_repair_before_phone_setup() {
        let mut checks = vec![EnvironmentCheck::new(
            "Selected iPhone / transport",
            CheckStatus::Warning,
            "No device",
            "",
        )];
        assert_eq!(
            recommended_action(&checks),
            Some(SetupAction::ConnectIphone)
        );
        checks.push(EnvironmentCheck::new(
            APPLE_RUNTIME,
            CheckStatus::Fail,
            "Missing DLL",
            "",
        ));
        assert_eq!(recommended_action(&checks), Some(SetupAction::RepairApple));
        assert_eq!(
            recommended_action(&[EnvironmentCheck::new(
                "Apple pairing / AirTraffic service",
                CheckStatus::Warning,
                "Not checked because support is missing",
                ""
            )]),
            None
        );
        assert_eq!(recommended_action(&[]), None);
        let checks = vec![
            EnvironmentCheck::new(
                "Apple Mobile Device Service",
                CheckStatus::Fail,
                "Service stopped",
                "",
            ),
            EnvironmentCheck::new(
                "Lockdown service access (BUID)",
                CheckStatus::Fail,
                "Connection refused",
                "",
            ),
        ];
        assert_eq!(
            recommended_action(&checks),
            Some(SetupAction::RepairService)
        );
    }

    #[test]
    fn service_fix_does_not_install_components_or_change_pairing() {
        let script = setup_script(SetupAction::RepairService);
        assert!(script.contains("Restart-Service"));
        assert!(!script.contains("Invoke-WebRequest"));
        assert!(!script.contains("msiexec"));
        assert!(!script.contains("Lockdown"));
        let checks = vec![EnvironmentCheck::new(
            "Apple Mobile Device Service",
            CheckStatus::Fail,
            "The Windows service is not installed.",
            "",
        )];
        assert_eq!(recommended_action(&checks), Some(SetupAction::RepairApple));
    }

    #[test]
    fn setup_scripts_parse_in_windows_powershell_without_running() {
        use std::io::Write;
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        for action in [
            SetupAction::RepairService,
            SetupAction::RepairApple,
            SetupAction::ConnectIphone,
        ] {
            let mut parser = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[scriptblock]::Create([Console]::In.ReadToEnd()) | Out-Null",
                ])
                .creation_flags(0x08000000)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            parser
                .stdin
                .take()
                .unwrap()
                .write_all(setup_script(action).as_bytes())
                .unwrap();
            let output = parser.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
