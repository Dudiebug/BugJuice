// LHM helper process management — spawns bugjuice-lhm.exe and reads
// power sensor data from its stdout as JSON lines.
//
// The helper runs LibreHardwareMonitorLib in-process, reading RAPL and
// GPU power sensors. Its output matches the EmiReading wire format so
// the Tauri app consumes it without changes.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::emi::EmiReading;

/// Resolve the helper exe path — same directory as the service exe.
fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let path = dir.join("bugjuice-lhm.exe");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Spawn the helper process with stdout piped, stderr inherited (for logging).
fn spawn_helper(path: &PathBuf) -> Result<Child, String> {
    Command::new(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("failed to spawn bugjuice-lhm: {e}"))
}

/// Main loop: spawn the helper, read JSON lines, store readings.
/// If the helper exits, wait 5 seconds and restart.
pub fn run(data: Arc<Mutex<Vec<EmiReading>>>, shutdown: &AtomicBool) {
    // x64 only — ARM64 has EMI and doesn't need LHM.
    if cfg!(target_arch = "aarch64") {
        return;
    }

    let path = match helper_path() {
        Some(p) => p,
        None => {
            eprintln!("[lhm-helper] bugjuice-lhm.exe not found — skipping LHM");
            return;
        }
    };

    eprintln!("[lhm-helper] Found helper at {}", path.display());

    while !shutdown.load(Ordering::Relaxed) {
        eprintln!("[lhm-helper] Spawning helper process...");
        let mut child = match spawn_helper(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[lhm-helper] Spawn failed: {e}");
                wait_or_shutdown(shutdown, 5);
                continue;
            }
        };

        // Read JSON lines from the helper's stdout.
        if let Some(stdout) = child.stdout.take() {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }

                let line = match line {
                    Ok(l) => l,
                    Err(_) => break, // pipe broken, helper likely exited
                };

                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<EmiReading>(&line) {
                    Ok(reading) => {
                        if let Ok(mut lock) = data.lock() {
                            *lock = vec![reading];
                        }
                    }
                    Err(e) => {
                        eprintln!("[lhm-helper] JSON parse error: {e} — line: {line}");
                    }
                }
            }
        }

        // Helper exited — clean up and restart after delay.
        let _ = child.kill();
        let _ = child.wait();

        if !shutdown.load(Ordering::Relaxed) {
            eprintln!("[lhm-helper] Helper exited, restarting in 5s...");
            wait_or_shutdown(shutdown, 5);
        }
    }
}

/// Wait `secs` seconds, checking for shutdown every 100ms.
fn wait_or_shutdown(shutdown: &AtomicBool, secs: u64) {
    for _ in 0..(secs * 10) {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
