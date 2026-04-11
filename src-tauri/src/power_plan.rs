// Windows power plan (scheme) management.
//
// Uses Win32 PowerGetActiveScheme / PowerSetActiveScheme to read and
// switch the active power plan. The polling loop calls `auto_switch`
// each tick to enforce threshold-based switching on battery.
//
// Well-known GUIDs:
//   Balanced:         381b4222-f694-41f0-9685-ff5bb260df2e
//   Power Saver:      a1841308-3541-4fab-bc81-f71556f20b4a
//   High Performance: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Power::{PowerGetActiveScheme, PowerSetActiveScheme};
use windows::core::GUID;

// ─── Well-known schemes ──────────────────────────────────────────────────────

pub const BALANCED: GUID = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);
pub const POWER_SAVER: GUID = GUID::from_u128(0xa1841308_3541_4fab_bc81_f71556f20b4a);
pub const HIGH_PERFORMANCE: GUID = GUID::from_u128(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);

// ─── Config (atomics, set from commands.rs) ─────────────────────────────────

static ENABLED: AtomicBool = AtomicBool::new(false);
static LOW_THRESHOLD: AtomicU32 = AtomicU32::new(30);
static HIGH_THRESHOLD: AtomicU32 = AtomicU32::new(80);

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn set_enabled(v: bool) {
    ENABLED.store(v, Ordering::Relaxed);
}

pub fn set_thresholds(low: u32, high: u32) {
    LOW_THRESHOLD.store(low.clamp(5, 95), Ordering::Relaxed);
    HIGH_THRESHOLD.store(high.clamp(low + 5, 100), Ordering::Relaxed);
}

pub fn thresholds() -> (u32, u32) {
    (
        LOW_THRESHOLD.load(Ordering::Relaxed),
        HIGH_THRESHOLD.load(Ordering::Relaxed),
    )
}

// ─── Win32 wrappers ─────────────────────────────────────────────────────────

/// Get the currently active power scheme GUID.
pub fn get_active_scheme() -> Option<GUID> {
    unsafe {
        let mut guid_ptr: *mut GUID = std::ptr::null_mut();
        let ret = PowerGetActiveScheme(None, &mut guid_ptr);
        if ret != WIN32_ERROR(0) || guid_ptr.is_null() {
            return None;
        }
        let guid = *guid_ptr;
        // Free the GUID allocated by PowerGetActiveScheme.
        windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(guid_ptr as *mut c_void)));
        Some(guid)
    }
}

/// Set the active power scheme. Returns true on success.
pub fn set_active_scheme(guid: &GUID) -> bool {
    unsafe { PowerSetActiveScheme(None, Some(guid)) == WIN32_ERROR(0) }
}

/// Human-readable name for a well-known scheme.
pub fn scheme_name(guid: &GUID) -> &'static str {
    if *guid == BALANCED {
        "Balanced"
    } else if *guid == POWER_SAVER {
        "Power saver"
    } else if *guid == HIGH_PERFORMANCE {
        "High performance"
    } else {
        "Custom"
    }
}

// ─── Auto-switch logic (called from polling loop) ───────────────────────────

/// Track what we last switched to so we don't call SetActiveScheme every tick.
static LAST_SWITCHED: std::sync::OnceLock<std::sync::Mutex<Option<GUID>>> =
    std::sync::OnceLock::new();

fn last_switched() -> &'static std::sync::Mutex<Option<GUID>> {
    LAST_SWITCHED.get_or_init(|| std::sync::Mutex::new(None))
}

/// Check battery percent and switch power plan if thresholds are crossed.
/// Called once per polling tick. Only acts when on battery.
pub fn auto_switch(percent: f64, on_ac: bool) {
    if !is_enabled() {
        return;
    }

    let (low, high) = thresholds();
    let target = if on_ac {
        // On AC: always balanced.
        BALANCED
    } else if percent < low as f64 {
        POWER_SAVER
    } else if percent > high as f64 {
        BALANCED
    } else {
        // Between thresholds: don't change.
        return;
    };

    // Only switch if different from what we last set.
    let mut last = last_switched().lock().unwrap();
    if *last == Some(target) {
        return;
    }

    if set_active_scheme(&target) {
        log::info!(
            "auto-switch: {} (percent={:.0}%, on_ac={})",
            scheme_name(&target),
            percent,
            on_ac
        );
        *last = Some(target);
    }
}
