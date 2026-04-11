// BugJuice Tauri entry point.
//
// Wires together the backend modules (copied from cli/src/) and exposes
// them to the React frontend as Tauri commands.

mod battery;
mod commands;
mod events;
mod gpu;
mod lhm;
mod lhm_setup;
mod pipe_client;
mod polling;
mod power;
mod power_plan;
mod processes;
mod storage;

use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::{Emitter, Manager};

/// Global AppHandle so the polling thread and event callbacks can access
/// Tauri APIs (tray tooltip, notifications) without plumbing through args.
static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn app_handle() -> Option<&'static tauri::AppHandle> {
    APP_HANDLE.get()
}

/// Global tray menu so the polling thread can update the info items.
static TRAY_MENU: OnceLock<tauri::menu::Menu<tauri::Wry>> = OnceLock::new();

pub fn tray_menu() -> Option<&'static tauri::menu::Menu<tauri::Wry>> {
    TRAY_MENU.get()
}

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
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            // Store the AppHandle globally so the polling thread and event
            // callbacks can access tray icon and notification APIs.
            APP_HANDLE.set(app.handle().clone()).ok();

            // ── Auto-enable autostart on first launch ─────────────────
            {
                let data_dir = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| PathBuf::from("."));
                let marker = data_dir.join(".autostart-initialized");
                if !marker.exists() {
                    use tauri_plugin_autostart::ManagerExt;
                    let _ = app.autolaunch().enable();
                    let _ = std::fs::create_dir_all(&data_dir);
                    let _ = std::fs::write(&marker, "1");
                }
            }

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
                // Prune stale data on startup using the default retention.
                if let Some(s) = storage::global() {
                    let days = commands::data_retention_days();
                    match s.prune_old_data(days) {
                        Ok((r, a)) if r + a > 0 => {
                            log::info!("startup prune: {r} readings + {a} app_power rows");
                        }
                        _ => {}
                    }
                }
            }

            // Match the "don't be a battery hog" rule from the scope doc:
            // ask Windows to throttle our process. Win11 only; older builds
            // silently ignore the call.
            enable_ecoqos();

            // Spawn the background polling thread (battery + EMI + per-app
            // attribution -> SQLite every adaptive interval).
            polling::spawn();

            // Register power-event callbacks (sleep/wake, AC/DC, 1% changes,
            // display state). The handles must outlive the program; leak them.
            match events::register() {
                Ok(handles) => {
                    std::mem::forget(handles);
                }
                Err(e) => log::warn!("power event registration failed: {e}"),
            }

            // ── System tray ─────────────────────────────────────────────
            use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            let info_state =
                MenuItem::with_id(app, "info_state", "Checking battery\u{2026}", false, None::<&str>)?;
            let info_eta =
                MenuItem::with_id(app, "info_eta", "", false, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let show_item = MenuItem::with_id(app, "show", "Show BugJuice", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &info_state,
                    &info_eta,
                    &sep1,
                    &show_item,
                    &settings_item,
                    &separator,
                    &quit_item,
                ],
            )?;

            TRAY_MENU.set(menu.clone()).ok();

            let tray_icon = app
                .default_window_icon()
                .cloned()
                .unwrap_or_else(|| {
                    tauri::image::Image::from_bytes(include_bytes!("../icons/icon.ico"))
                        .expect("failed to load tray icon from icons/icon.ico")
                });
            let _tray = TrayIconBuilder::with_id("main")
                .icon(tray_icon)
                .menu(&menu)
                .tooltip("BugJuice")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "settings" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                            let _ = w.emit("navigate", "/settings");
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            // ── Start minimized to tray ─────────────────────────────
            // The window starts hidden (visible: false in tauri.conf.json).
            // Show it now unless the user opted to start minimized to tray.
            {
                let data_dir = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| PathBuf::from("."));
                let start_minimized = data_dir.join(".start-minimized").exists();
                if !start_minimized {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
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
        .on_window_event(|window, event| {
            // Hide the window to tray instead of quitting when the user
            // clicks the X button. The "Quit" menu item in the tray calls
            // app.exit(0) to actually terminate.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_battery_status,
            commands::get_power_reading,
            commands::get_top_apps,
            commands::get_battery_history,
            commands::get_component_history,
            commands::get_battery_sessions,
            commands::get_sleep_sessions,
            commands::get_health_history,
            commands::get_session_detail,
            commands::get_unified_timeline,
            commands::get_db_stats,
            commands::get_accent_color,
            commands::set_notification_prefs,
            commands::enable_autostart,
            commands::disable_autostart,
            commands::is_autostart_enabled,
            commands::set_start_minimized,
            commands::get_start_minimized,
            commands::test_notification,
            commands::get_charge_speed,
            commands::get_charge_habits,
            commands::export_report_json,
            commands::export_report_pdf,
            commands::get_unplug_estimate,
            commands::get_power_plan_status,
            commands::set_power_plan_config,
            commands::set_data_retention,
            lhm_setup::get_lhm_status,
            lhm_setup::lhm_download,
            lhm_setup::lhm_find_download,
            lhm_setup::lhm_install,
            lhm_setup::lhm_verify,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
