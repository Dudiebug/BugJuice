// BugJuice backend prototype — CLI entry point.
//
// Usage:
//   bugjuice-cli              → one-shot battery probe + EMI snapshot
//   bugjuice-cli --watch      → probe + live monitoring + background polling
//                               into SQLite (sleep/wake drain, AC/DC, 1% events,
//                               5-second sensor logging)
//   bugjuice-cli --stats      → print row counts from the local database

mod battery;
mod events;
mod polling;
mod power;
mod storage;

use battery::{
    BATTERY_CHARGING, BATTERY_CRITICAL, BATTERY_DISCHARGING, BATTERY_POWER_ON_LINE,
    BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE, BATTERY_UNKNOWN_VOLTAGE, BatterySnapshot,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let watch = args.iter().any(|a| a == "--watch" || a == "-w");
    let stats = args.iter().any(|a| a == "--stats");

    // Initialize storage early so even one-shot runs can log a health
    // snapshot. Failure here is non-fatal — we just print a warning.
    let db_path = storage::default_db_path();
    if let Err(e) = storage::init(&db_path) {
        eprintln!("warning: could not open database at {}: {e}", db_path.display());
    }

    if stats {
        print_stats(&db_path);
        return;
    }

    println!("BugJuice backend prototype — battery probe");
    println!("==========================================");

    let snaps = match battery::snapshot_all() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("\nError: {e}");
            std::process::exit(1);
        }
    };

    if snaps.is_empty() {
        println!("\nNo batteries found on this machine.");
        println!("(If this is a desktop, that's expected.)");
        return;
    }

    for (i, snap) in snaps.iter().enumerate() {
        print_battery(i, snap);
    }

    // Always log a health snapshot on every run so we have at least one
    // datapoint per launch even when not in watch mode.
    if let Some(s) = storage::global() {
        let _ = s.log_health_snapshot(&snaps[0]);
    }

    // CPU / system power via EMI (ARM64 primary path, also works on
    // many x64 Surface/OEM devices). Falls through with a status note
    // on platforms where neither EMI nor RAPL is reachable.
    power::print_power_summary(std::time::Duration::from_secs(2));

    println!("\n──────────────────────────────────────────────");
    println!("✓ Probed {} battery device(s) successfully.", snaps.len());

    if !watch {
        return;
    }

    // ── Watch mode ──────────────────────────────────────────────────────────
    println!("\nEntering watch mode.");
    println!("  database: {}", db_path.display());
    println!("\nEvents print as they happen:");
    println!("  • unplug / replug (AC/DC transitions)");
    println!("  • each 1% battery change");
    println!("  • sleep / wake (with drain measurement, persisted)");
    println!("\nBackground polling logs sensor readings every 5 seconds.");
    println!("Press Ctrl+C to quit.\n");

    // Reduce our own footprint: ask Windows to throttle us via EcoQoS.
    enable_ecoqos();

    // Start the background polling thread (battery + EMI → SQLite).
    polling::spawn();

    // Keep the registration alive for the lifetime of the process.
    let _handles = match events::register() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to register power events: {e}");
            std::process::exit(1);
        }
    };

    // Callbacks fire on system thread-pool threads, so the main thread just
    // needs to stay alive. Park forever.
    loop {
        std::thread::park();
    }
}

fn print_stats(db_path: &std::path::Path) {
    println!("BugJuice — database stats");
    println!("=========================");
    println!("path: {}", db_path.display());
    let Some(s) = storage::global() else {
        println!("(database not opened)");
        return;
    };
    match s.row_counts() {
        Ok(c) => {
            println!("\n  sensors:           {}", c.sensors);
            println!("  readings:          {}", c.readings);
            println!("  battery sessions:  {}", c.battery_sessions);
            println!("  sleep sessions:    {}", c.sleep_sessions);
            println!("  health snapshots:  {}", c.health_snapshots);
        }
        Err(e) => println!("(query failed: {e})"),
    }
}

/// Apply EcoQoS execution-speed throttling to our own process.
/// On Windows 11 this can reduce CPU power use by up to 90% for
/// background work, which matches BugJuice's "don't be a battery hog"
/// design principle. Failure is silent — older Windows builds and
/// non-Win11 systems just ignore it.
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

// ─── Pretty printer ───────────────────────────────────────────────────────────

fn print_battery(index: usize, snap: &BatterySnapshot) {
    let info = &snap.info;
    let status = &snap.status;

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Battery #{index}");
    println!("  tag: {}", snap.tag);

    let chem = String::from_utf8_lossy(&info.chemistry)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let chem_upper = chem.to_ascii_uppercase();
    // Chemistry is the truth: any lithium-based chemistry is rechargeable
    // regardless of the Technology field. Some OEM firmwares (notably HP)
    // report Technology=0 on obviously-rechargeable batteries.
    let is_rechargeable = info.technology == 1
        || chem_upper.starts_with("LI")
        || chem_upper.starts_with("NI");

    let design = info.designed_capacity.max(1);
    let raw_health = info.full_charged_capacity as f64 / design as f64 * 100.0;
    let health_pct = raw_health.min(100.0);
    let wear_pct = (100.0 - raw_health).max(0.0);

    println!("\nHealth & design");
    println!("  chemistry:            {chem}");
    println!(
        "  technology:           {}",
        if is_rechargeable {
            "rechargeable"
        } else {
            "non-rechargeable"
        }
    );
    println!("  designed capacity:    {} mWh", info.designed_capacity);
    println!("  full-charge capacity: {} mWh", info.full_charged_capacity);
    println!(
        "  wear level:           {wear_pct:.1}%  →  your battery holds {health_pct:.0}% of its original capacity — {}",
        wear_verdict(wear_pct)
    );
    println!(
        "  cycle count:          {} cycles used out of roughly 1,000 expected lifespan",
        info.cycle_count
    );

    if let Some(s) = &snap.manufacturer {
        if !is_placeholder_string(s) {
            println!("  manufacturer:         {s}");
        }
    }
    if let Some(s) = &snap.device_name {
        if !is_placeholder_string(s) {
            println!("  device name:          {s}");
        }
    }
    if let Some(s) = &snap.serial {
        if !is_placeholder_string(s) {
            println!("  serial number:        {s}");
        }
    }
    if let Some(d) = &snap.manufacture_date {
        println!(
            "  manufactured:         {:04}-{:02}-{:02}",
            d.year, d.month, d.day
        );
    }
    if let Some(c) = snap.temperature_c {
        println!("  temperature:          {c:.1}°C");
    }

    println!("\nLive status");

    // Power state flags → readable
    let mut states: Vec<&str> = Vec::new();
    if status.power_state & BATTERY_POWER_ON_LINE != 0 {
        states.push("on AC");
    }
    if status.power_state & BATTERY_CHARGING != 0 {
        states.push("charging");
    }
    if status.power_state & BATTERY_DISCHARGING != 0 {
        states.push("discharging");
    }
    if status.power_state & BATTERY_CRITICAL != 0 {
        states.push("CRITICAL");
    }
    let state_str = if states.is_empty() {
        "idle".to_string()
    } else {
        states.join(", ")
    };
    println!("  power state:          {state_str}");

    if status.capacity != BATTERY_UNKNOWN_CAPACITY {
        let pct = status.capacity as f64 / info.full_charged_capacity.max(1) as f64 * 100.0;
        println!(
            "  capacity:             {} mWh ({pct:.1}% of full)",
            status.capacity
        );
    } else {
        println!("  capacity:             unknown");
    }

    if status.voltage != BATTERY_UNKNOWN_VOLTAGE {
        println!("  voltage:              {:.3} V", status.voltage as f64 / 1000.0);
    }

    if status.rate == BATTERY_UNKNOWN_RATE || status.rate == 0 {
        println!("  rate:                 idle / not reporting");
    } else if status.rate > 0 {
        let watts = status.rate as f64 / 1000.0;
        let remaining_to_full = info.full_charged_capacity.saturating_sub(status.capacity);
        let time_h = remaining_to_full as f64 / status.rate as f64;
        println!(
            "  rate:                 charging at {watts:.2} W — about {} to full at this rate",
            format_hours(time_h)
        );
    } else {
        let watts = (-status.rate) as f64 / 1000.0;
        let time_h = status.capacity as f64 / (-status.rate) as f64;
        println!(
            "  rate:                 draining at {watts:.2} W — about {} left at this rate",
            format_hours(time_h)
        );
    }

    if let Some(secs) = snap.estimated_seconds {
        println!(
            "  kernel estimate:      {} remaining",
            format_hours(secs as f64 / 3600.0)
        );
    }
}

// ─── Formatting helpers ───────────────────────────────────────────────────────

/// Filter out obvious placeholder/garbage values that some OEM firmwares
/// (HP especially) return for manufacturer / serial / device name fields.
fn is_placeholder_string(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return true;
    }
    let upper = t.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "SERIALNUMBER" | "MANUFACTURER" | "DEVICENAME" | "NAME" | "UNKNOWN" | "N/A" | "NONE" | "PRIMARY"
    ) || t.chars().all(|c| c == '0')
}

fn wear_verdict(wear_pct: f64) -> &'static str {
    if wear_pct < 10.0 {
        "still healthy"
    } else if wear_pct < 25.0 {
        "some wear, normal for age"
    } else if wear_pct < 40.0 {
        "noticeably worn"
    } else {
        "heavy wear, nearing replacement"
    }
}

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
