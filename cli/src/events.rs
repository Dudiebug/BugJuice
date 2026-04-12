// Power event subscriptions: sleep/wake + AC/DC transitions + battery %.
//
// Uses DEVICE_NOTIFY_CALLBACK mode — no window handle required. Callbacks
// fire on system thread-pool threads, which means they can outlive normal
// control flow and must not borrow local state.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::GUID;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, POWERBROADCAST_SETTING,
    PowerRegisterSuspendResumeNotification, PowerSettingRegisterNotification,
    PowerSettingUnregisterNotification, PowerUnregisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS;

// ─── Constants ────────────────────────────────────────────────────────────────

const DEVICE_NOTIFY_CALLBACK: REGISTER_NOTIFICATION_FLAGS = REGISTER_NOTIFICATION_FLAGS(2);

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_POWERSETTINGCHANGE: u32 = 0x8013;

/// GUID_ACDC_POWER_SOURCE — fires on AC ↔ battery transitions.
const GUID_ACDC_POWER_SOURCE: GUID =
    GUID::from_u128(0x5d3e9a59_e9d5_4b00_a6bd_ff34ff516548);

/// GUID_BATTERY_PERCENTAGE_REMAINING — fires on each 1% change.
const GUID_BATTERY_PERCENTAGE_REMAINING: GUID =
    GUID::from_u128(0xa7ad8041_b45a_4cae_87a3_eecbb468a9e1);

/// GUID_CONSOLE_DISPLAY_STATE — fires when the display turns on/off/dim.
/// This is the real signal for "user actually woke the machine" on
/// Modern Standby systems, where PBT_APMRESUMEAUTOMATIC fires on every
/// background wake during sleep.
const GUID_CONSOLE_DISPLAY_STATE: GUID =
    GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);

// ─── Sleep drain state ────────────────────────────────────────────────────────
//
// We track a small state machine because Modern Standby complicates things:
//   Awake   → lid closed / Start→Sleep     → APMSUSPEND fires → Sleeping
//   Sleeping → (background wakes fire APMRESUMEAUTOMATIC but display stays off;
//               may fire another APMSUSPEND going back down — preserve baseline)
//   Sleeping → user opens lid / presses key → display turns ON  → Awake + measure
//
// The "real" wake signal is GUID_CONSOLE_DISPLAY_STATE going to ON while we're
// in the Sleeping state. APMRESUMEAUTOMATIC alone is unreliable.

#[derive(Clone, Copy)]
enum SleepState {
    Awake,
    Sleeping {
        baseline_mwh: u32,
        at: SystemTime,
        /// Row id in the sleep_sessions table, set when we inserted the
        /// "started" row on PBT_APMSUSPEND. Updated on wake.
        sleep_row_id: Option<i64>,
    },
}

static SLEEP_STATE: OnceLock<Mutex<SleepState>> = OnceLock::new();

fn sleep_state() -> &'static Mutex<SleepState> {
    SLEEP_STATE.get_or_init(|| Mutex::new(SleepState::Awake))
}

// ─── Timestamp helper ─────────────────────────────────────────────────────────

fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Local hour approximation (UTC — good enough for logging).
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

// ─── Callbacks ────────────────────────────────────────────────────────────────

unsafe extern "system" fn suspend_resume_callback(
    _context: *const c_void,
    type_: u32,
    _setting: *const c_void,
) -> u32 {
    match type_ {
        PBT_APMSUSPEND => {
            let mut state = sleep_state().lock().unwrap();
            match *state {
                SleepState::Awake => {
                    // Entering a new sleep session — take the baseline.
                    match crate::battery::quick_capacity_mwh() {
                        Ok(cap) => {
                            // Persist the "started" row so we don't lose
                            // this session if the process is killed or
                            // reboots before the user wakes the machine.
                            let sleep_row_id = crate::storage::global()
                                .and_then(|s| s.start_sleep_session(Some(cap)).ok());
                            *state = SleepState::Sleeping {
                                baseline_mwh: cap,
                                at: SystemTime::now(),
                                sleep_row_id,
                            };
                            println!(
                                "\n[{}] ─ SYSTEM SUSPENDING ─  baseline {cap} mWh",
                                timestamp()
                            );
                        }
                        Err(e) => println!(
                            "\n[{}] ─ SYSTEM SUSPENDING ─  (snapshot failed: {e})",
                            timestamp()
                        ),
                    }
                }
                SleepState::Sleeping { .. } => {
                    // Already in a sleep session — Modern Standby cycling.
                    // Keep the original baseline and row id.
                }
            }
        }
        // Intentionally ignore PBT_APMRESUMEAUTOMATIC: on Modern Standby
        // systems it fires on every background wake, which is useless as a
        // "user woke the machine" signal. We wait for GUID_CONSOLE_DISPLAY_STATE
        // = ON instead (handled in power_setting_callback).
        _ => {}
    }
    0 // ERROR_SUCCESS
}

fn measure_sleep_drain(baseline_mwh: u32, at: SystemTime, sleep_row_id: Option<i64>) {
    // Battery readings are stale for 2-5s after resume — give EC time.
    std::thread::sleep(std::time::Duration::from_secs(3));

    let post_cap = match crate::battery::quick_capacity_mwh() {
        Ok(c) => c,
        Err(e) => {
            println!("  could not read post-sleep capacity: {e}");
            return;
        }
    };

    let duration_h = SystemTime::now()
        .duration_since(at)
        .map(|d| d.as_secs_f64() / 3600.0)
        .unwrap_or(0.0);
    let drain_mwh = baseline_mwh as i64 - post_cap as i64;

    if duration_h < 0.0008 {
        return;
    }

    println!("  slept for {}", super::format_hours(duration_h));
    println!("  pre:  {baseline_mwh} mWh");
    println!("  post: {post_cap} mWh");

    let (drain_print, rate_mw_opt, pct_opt) = if drain_mwh <= 0 {
        println!("  drain: none (charging while asleep?)");
        (None, None, None)
    } else {
        let rate_mw = drain_mwh as f64 / duration_h;
        let pct = drain_mwh as f64 / baseline_mwh.max(1) as f64 * 100.0;
        let verdict = if rate_mw < 50.0 {
            "excellent — Modern Standby working well"
        } else if rate_mw < 200.0 {
            "normal"
        } else if rate_mw < 500.0 {
            "high — something may be waking your laptop"
        } else {
            "very high — investigate immediately"
        };
        println!("  drain: {drain_mwh} mWh ({pct:.1}% of pre-sleep capacity)");
        println!("  avg rate: {rate_mw:.0} mW  →  {verdict}");
        (Some(drain_mwh), Some(rate_mw), Some(pct))
    };

    // Persist results to the sleep_sessions row we started earlier.
    if let (Some(id), Some(storage)) = (sleep_row_id, crate::storage::global()) {
        let _ = storage.finish_sleep_session(
            id,
            Some(post_cap),
            drain_print,
            rate_mw_opt,
            pct_opt,
        );
    }
}

unsafe extern "system" fn power_setting_callback(
    _context: *const c_void,
    type_: u32,
    setting: *const c_void,
) -> u32 {
    if type_ != PBT_POWERSETTINGCHANGE || setting.is_null() {
        return 0;
    }
    unsafe {
        let pbs = setting as *const POWERBROADCAST_SETTING;
        let guid = (*pbs).PowerSetting;
        let data_ptr = ptr::addr_of!((*pbs).Data) as *const u8;
        let val = *data_ptr;

        if guid == GUID_ACDC_POWER_SOURCE {
            let desc = match val {
                0 => "AC (plugged in)",
                1 => "DC (on battery)",
                2 => "short-term UPS",
                _ => "unknown",
            };
            println!("[{}] power source → {desc}", timestamp());
        } else if guid == GUID_BATTERY_PERCENTAGE_REMAINING {
            println!("[{}] battery: {val}%", timestamp());
        } else if guid == GUID_CONSOLE_DISPLAY_STATE {
            // val: 0=off, 1=on, 2=dimmed
            if val == 1 {
                // Display turned on. If we're in an active sleep session,
                // this is the real user wake — measure drain now.
                let mut state = sleep_state().lock().unwrap();
                if let SleepState::Sleeping {
                    baseline_mwh,
                    at,
                    sleep_row_id,
                } = *state
                {
                    *state = SleepState::Awake;
                    drop(state);
                    println!("\n[{}] ─ SYSTEM RESUMED (user wake) ─", timestamp());
                    std::thread::spawn(move || {
                        measure_sleep_drain(baseline_mwh, at, sleep_row_id)
                    });
                }
            }
        }
    }
    0
}

// ─── Registration ─────────────────────────────────────────────────────────────

/// Handles kept alive for the lifetime of the monitoring session. Dropping
/// this struct unregisters the callbacks.
pub struct EventHandles {
    suspend: *mut c_void,
    acdc: *mut c_void,
    percent: *mut c_void,
    display: *mut c_void,
}

// SAFETY: HPOWERNOTIFY handles are thread-safe per Win32 docs.
unsafe impl Send for EventHandles {}
unsafe impl Sync for EventHandles {}

impl Drop for EventHandles {
    fn drop(&mut self) {
        unsafe {
            let _ = PowerUnregisterSuspendResumeNotification(HPOWERNOTIFY(self.suspend as isize));
            let _ = PowerSettingUnregisterNotification(HPOWERNOTIFY(self.acdc as isize));
            let _ = PowerSettingUnregisterNotification(HPOWERNOTIFY(self.percent as isize));
            let _ = PowerSettingUnregisterNotification(HPOWERNOTIFY(self.display as isize));
        }
    }
}

pub fn register() -> Result<EventHandles, String> {
    unsafe {
        // The DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS struct must outlive the
        // registration. Box::leak pins it — we never unregister until process
        // exit, so leaking is fine.
        let suspend_params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(suspend_resume_callback),
            Context: ptr::null_mut(),
        }));
        let mut suspend: *mut c_void = ptr::null_mut();
        let err = PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(suspend_params as *mut _ as *mut c_void),
            &mut suspend,
        );
        if err.0 != 0 {
            return Err(format!(
                "PowerRegisterSuspendResumeNotification failed: {}", err.0
            ));
        }

        let acdc_params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_setting_callback),
            Context: ptr::null_mut(),
        }));
        let mut acdc: *mut c_void = ptr::null_mut();
        let err = PowerSettingRegisterNotification(
            &GUID_ACDC_POWER_SOURCE,
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(acdc_params as *mut _ as *mut c_void),
            &mut acdc,
        );
        if err.0 != 0 {
            return Err(format!("PowerSettingRegisterNotification(ACDC) failed: {}", err.0));
        }

        let pct_params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_setting_callback),
            Context: ptr::null_mut(),
        }));
        let mut percent: *mut c_void = ptr::null_mut();
        let err = PowerSettingRegisterNotification(
            &GUID_BATTERY_PERCENTAGE_REMAINING,
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(pct_params as *mut _ as *mut c_void),
            &mut percent,
        );
        if err.0 != 0 {
            return Err(format!("PowerSettingRegisterNotification(PERCENT) failed: {}", err.0));
        }

        let display_params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(power_setting_callback),
            Context: ptr::null_mut(),
        }));
        let mut display: *mut c_void = ptr::null_mut();
        let err = PowerSettingRegisterNotification(
            &GUID_CONSOLE_DISPLAY_STATE,
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(display_params as *mut _ as *mut c_void),
            &mut display,
        );
        if err.0 != 0 {
            return Err(format!(
                "PowerSettingRegisterNotification(DISPLAY) failed: {}", err.0
            ));
        }

        Ok(EventHandles {
            suspend,
            acdc,
            percent,
            display,
        })
    }
}
