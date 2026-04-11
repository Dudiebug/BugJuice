// LibreHardwareMonitor setup helper — download, extract, launch, verify.
//
// On x64 Intel/AMD laptops, LHM is the primary path to CPU/GPU power data.
// This module provides Tauri commands that guide the user through a one-click
// setup: auto-download from GitHub, extract to AppData, launch with admin
// elevation, and create a scheduled task for auto-start.
//
// All heavy lifting (HTTP, zip extraction, elevation) is done via PowerShell
// subprocesses to avoid adding Rust dependencies. The `CREATE_NO_WINDOW`
// flag suppresses console flashes.

use serde::Serialize;
use std::path::PathBuf;

/// Creation flag to suppress the console window when spawning PowerShell.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// ─── DTO types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LhmStatusDto {
    /// True if this is an x64 machine AND neither EMI nor LHM provides power data.
    pub needed: bool,
    /// True if LHM's WMI namespace is responding (LHM is running).
    pub running: bool,
    /// True if we've previously extracted LHM to our managed directory.
    pub installed: bool,
    /// True if the "BugJuice-LHM" scheduled task exists for auto-start.
    pub auto_start_enabled: bool,
    /// The directory where LHM is (or would be) installed.
    pub lhm_dir: String,
    /// True if the bugjuice service returned EMI channels.
    pub has_emi: bool,
    /// True if the bugjuice service named pipe is connectable.
    pub service_running: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LhmDownloadResult {
    pub success: bool,
    pub zip_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LhmInstallResult {
    pub success: bool,
    pub exe_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Resolved LHM installation directory: %LOCALAPPDATA%\BugJuice\LibreHardwareMonitor
fn lhm_dir() -> PathBuf {
    let local_app_data = std::env::var("LOCALAPPDATA")
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
            format!("{home}\\AppData\\Local")
        });
    PathBuf::from(local_app_data)
        .join("BugJuice")
        .join("LibreHardwareMonitor")
}

/// Path to the LHM executable inside our managed directory.
fn lhm_exe() -> PathBuf {
    lhm_dir().join("LibreHardwareMonitor.exe")
}

/// Run a PowerShell command with CREATE_NO_WINDOW, returning (success, stdout, stderr).
#[cfg(target_os = "windows")]
fn run_powershell(script: &str) -> (bool, String, String) {
    use std::os::windows::process::CommandExt;
    match std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            (output.status.success(), stdout, stderr)
        }
        Err(e) => (false, String::new(), format!("failed to spawn powershell: {e}")),
    }
}

#[cfg(not(target_os = "windows"))]
fn run_powershell(_script: &str) -> (bool, String, String) {
    (false, String::new(), "not supported on this platform".into())
}

/// Check if the bugjuice service pipe is connectable.
fn is_service_running() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(r"\\.\pipe\bugjuice")
        .is_ok()
}

/// Check if the "BugJuice-LHM" scheduled task exists.
fn is_autostart_enabled() -> bool {
    let (ok, _, _) = run_powershell(
        "schtasks /Query /TN \"BugJuice-LHM\" 2>$null; exit $LASTEXITCODE"
    );
    ok
}

// ─── Tauri commands ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_lhm_status() -> Result<LhmStatusDto, String> {
    let is_x64 = cfg!(target_arch = "x86_64");
    let running = crate::lhm::is_available();

    // Check EMI availability via the pipe client (quick, non-blocking).
    let has_emi = crate::pipe_client::read_emi()
        .map(|readings| {
            readings.iter().any(|r| !r.channels.is_empty())
        })
        .unwrap_or(false);

    let service_running = is_service_running();
    let installed = lhm_exe().exists();
    let auto_start_enabled = if installed { is_autostart_enabled() } else { false };

    // LHM setup is "needed" on x64 when neither EMI nor LHM provides power data.
    let needed = is_x64 && !has_emi && !running;

    Ok(LhmStatusDto {
        needed,
        running,
        installed,
        auto_start_enabled,
        lhm_dir: lhm_dir().to_string_lossy().to_string(),
        has_emi,
        service_running,
    })
}

#[tauri::command]
pub async fn lhm_download() -> Result<LhmDownloadResult, String> {
    let dir = lhm_dir();
    let zip_path = dir.join("LibreHardwareMonitor.zip");

    // Ensure the target directory exists.
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Ok(LhmDownloadResult {
            success: false,
            zip_path: String::new(),
            error: Some(format!("failed to create directory: {e}")),
        });
    }

    let zip_str = zip_path.to_string_lossy().replace('\'', "''");

    // Single PowerShell script: query GitHub API for latest release, find the
    // main zip asset (not the .NET 10 variant), and download it.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
    $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/LibreHardwareMonitor/LibreHardwareMonitor/releases/latest' -UseBasicParsing
    # Match "LibreHardwareMonitor.zip" or legacy "LibreHardwareMonitor-net472.zip",
    # but NOT "LibreHardwareMonitor.NET.10.zip" or "LibreHardwareMonitorLib*".
    $asset = $release.assets | Where-Object {{ $_.name -match '^LibreHardwareMonitor(|-net\d+)\.zip$' }} | Select-Object -First 1
    if (-not $asset) {{
        Write-Error 'Could not find LibreHardwareMonitor zip in latest release'
        exit 1
    }}
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile '{zip_str}' -UseBasicParsing
    Write-Output 'OK'
}} catch {{
    Write-Error $_.Exception.Message
    exit 1
}}
"#
    );

    let (ok, stdout, stderr) = run_powershell(&script);

    if ok && stdout.trim().contains("OK") {
        Ok(LhmDownloadResult {
            success: true,
            zip_path: zip_path.to_string_lossy().to_string(),
            error: None,
        })
    } else {
        let err_msg = if !stderr.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            "download failed (unknown error)".to_string()
        };
        Ok(LhmDownloadResult {
            success: false,
            zip_path: String::new(),
            error: Some(err_msg),
        })
    }
}

#[tauri::command]
pub async fn lhm_find_download() -> Result<Option<String>, String> {
    let downloads = std::env::var("USERPROFILE")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .unwrap_or_else(|_| PathBuf::from(r"C:\Users\Public\Downloads"));

    if !downloads.is_dir() {
        return Ok(None);
    }

    let entries = std::fs::read_dir(&downloads).map_err(|e| format!("read_dir: {e}"))?;

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("librehardwaremonitor") && name.ends_with(".zip") {
            if let Ok(meta) = entry.metadata() {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().map_or(true, |(t, _)| modified > *t) {
                    best = Some((modified, entry.path()));
                }
            }
        }
    }

    Ok(best.map(|(_, p)| p.to_string_lossy().to_string()))
}

#[tauri::command]
pub async fn lhm_install(zip_path: Option<String>) -> Result<LhmInstallResult, String> {
    let dir = lhm_dir();
    let exe = lhm_exe();

    // Determine which zip to use.
    let zip = if let Some(ref p) = zip_path {
        PathBuf::from(p)
    } else {
        dir.join("LibreHardwareMonitor.zip")
    };

    if !zip.exists() {
        return Ok(LhmInstallResult {
            success: false,
            exe_path: String::new(),
            error: Some(format!("zip file not found: {}", zip.display())),
        });
    }

    // Ensure target directory exists.
    let _ = std::fs::create_dir_all(&dir);

    let zip_str = zip.to_string_lossy().replace('\'', "''");
    let dir_str = dir.to_string_lossy().replace('\'', "''");

    // Step 1: Extract the zip.
    let extract_script = format!(
        "Expand-Archive -Path '{zip_str}' -DestinationPath '{dir_str}' -Force"
    );
    let (ok, _, stderr) = run_powershell(&extract_script);
    if !ok {
        return Ok(LhmInstallResult {
            success: false,
            exe_path: String::new(),
            error: Some(format!("extraction failed: {}", stderr.trim())),
        });
    }

    // Step 2: Handle nested directory. The LHM zip sometimes extracts into a
    // subdirectory like "LibreHardwareMonitor-net472/". If the exe isn't at
    // the expected path, look one level deeper and move contents up.
    if !exe.exists() {
        // Look for the exe in any subdirectory.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let sub_exe = entry.path().join("LibreHardwareMonitor.exe");
                if sub_exe.exists() {
                    // Move all files from the subdirectory up to dir.
                    let sub_dir = entry.path();
                    if let Ok(sub_entries) = std::fs::read_dir(&sub_dir) {
                        for sub_entry in sub_entries.flatten() {
                            let dest = dir.join(sub_entry.file_name());
                            let _ = std::fs::rename(sub_entry.path(), &dest);
                        }
                    }
                    // Remove the now-empty subdirectory.
                    let _ = std::fs::remove_dir(&sub_dir);
                    break;
                }
            }
        }
    }

    if !exe.exists() {
        return Ok(LhmInstallResult {
            success: false,
            exe_path: String::new(),
            error: Some("extraction succeeded but LibreHardwareMonitor.exe not found".into()),
        });
    }

    // Step 3: Launch LHM with admin elevation AND create scheduled task for
    // auto-start, all in a single elevated PowerShell session (one UAC prompt).
    let exe_str = exe.to_string_lossy().replace('\'', "''").replace('"', r#"\""#);
    let launch_script = format!(
        r#"Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile -Command "schtasks /Create /TN \"BugJuice-LHM\" /TR \"\\\"{exe_str}\\\"\" /SC ONLOGON /RL HIGHEST /F; Start-Process \\\"{exe_str}\\\""' -Wait"#
    );
    let (ok, _, stderr) = run_powershell(&launch_script);

    // UAC denial returns a non-zero exit code. The scheduled task creation is
    // best-effort — if it fails, LHM still launches.
    if !ok {
        // Try launching without the scheduled task (simpler elevation).
        let fallback_script = format!(
            r#"Start-Process '{exe_str}' -Verb RunAs"#
        );
        let (ok2, _, stderr2) = run_powershell(&fallback_script);
        if !ok2 {
            return Ok(LhmInstallResult {
                success: false,
                exe_path: exe.to_string_lossy().to_string(),
                error: Some(format!(
                    "launch failed (UAC denied?): {}",
                    if !stderr.trim().is_empty() { stderr.trim() } else { stderr2.trim() }
                )),
            });
        }
    }

    // Step 4: Clean up the downloaded zip (best-effort).
    let _ = std::fs::remove_file(&zip);

    Ok(LhmInstallResult {
        success: true,
        exe_path: exe.to_string_lossy().to_string(),
        error: None,
    })
}

#[tauri::command]
pub async fn lhm_verify() -> Result<bool, String> {
    // Give LHM a moment to initialize its WMI provider if it just launched.
    // The caller (frontend) handles retry logic; we just do a single fresh check.
    Ok(crate::lhm::check_now())
}
