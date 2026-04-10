// BugJuice Tauri entry point.
//
// Wires together the backend modules (copied from cli/src/) and exposes
// them to the React frontend as Tauri commands.

mod battery;
mod commands;
mod events;
mod gpu;
mod polling;
mod power;
mod processes;
mod storage;

use std::path::PathBuf;
use tauri::Manager;

/// Apply EcoQoS execution-speed throttling to ourselves.
fn enable_ecoqos() {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetProcessInformation,
    };
    unsafe {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        };
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const c_void,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

/// Plain-english duration helper, used by event callbacks for log output.
pub(crate) fn format_hours(h: f64) -> String {
    if !h.is_finite() || h < 0.0 || h > 99.0 {
        return "a while".to_string();
    }
    let total_minutes = (h * 60.0).round() as u32;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours == 0 {
        format!("{minutes} min")
    } else {
        format!("{hours}h {minutes:02}m")
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Database lives in the per-user app data dir, e.g.
            //   C:\Users\<user>\AppData\Roaming\com.dudiebug.bugjuice\bugjuice.db
            let db_path: PathBuf = app
                .path()
                .app_data_dir()
                .map(|p| p.join("bugjuice.db"))
                .unwrap_or_else(|_| PathBuf::from("bugjuice.db"));

            if let Err(e) = storage::init(&db_path) {
                eprintln!("storage init failed at {:?}: {e}", db_path);
            } else {
                log::info!("storage opened at {db_path:?}");
            }

            // Match the "don't be a battery hog" rule from the scope doc:
            // ask Windows to throttle our process. Win11 only; older builds
            // silently ignore the call.
            enable_ecoqos();

            // Spawn the background polling thread (battery + EMI + per-app
            // attribution → SQLite every adaptive interval).
            polling::spawn();

            // Register power-event callbacks (sleep/wake, AC/DC, 1% changes,
            // display state). The handles must outlive the program; leak them.
            match events::register() {
                Ok(handles) => {
                    std::mem::forget(handles);
                }
                Err(e) => log::warn!("power event registration failed: {e}"),
            }

            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_battery_status,
            commands::get_power_reading,
            commands::get_top_apps,
            commands::get_battery_history,
            commands::get_battery_sessions,
            commands::get_sleep_sessions,
            commands::get_health_history,
            commands::get_session_detail,
            commands::get_db_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
