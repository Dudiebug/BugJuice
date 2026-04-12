// Background polling loop.
//
// Runs on a dedicated low-priority thread, reads sensors at the intervals
// the scope doc recommends, and logs everything into SQLite. For the
// prototype the intervals are fixed; adaptive behavior (slow down on
// battery, pause on display-off) is Phase 1 scope.
//
// Cadence (prototype):
//   every 5s   — battery status (capacity, rate, voltage, power state)
//   every 5s   — EMI power channels (CPU/GPU/DRAM/SYS/input)
//   every 60s  — health snapshot (design/full-charge/cycle/wear/temp)

use std::thread;
use std::time::{Duration, Instant};

use crate::battery::{self, BATTERY_POWER_ON_LINE, BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE};
use crate::power;
use crate::storage::{self, ReadingInput};

/// Spawn the polling thread. Runs until the process exits.
pub fn spawn() {
    thread::Builder::new()
        .name("bugjuice-poll".into())
        .spawn(run)
        .expect("failed to spawn polling thread");
}

fn run() {
    let mut last_health = Instant::now() - Duration::from_secs(3600); // force first tick
    loop {
        tick(&mut last_health);
        thread::sleep(Duration::from_secs(5));
    }
}

fn tick(last_health: &mut Instant) {
    let storage = match storage::global() {
        Some(s) => s,
        None => return,
    };

    // ── Battery read ─────────────────────────────────────────────────────────
    let snaps = match battery::snapshot_all() {
        Ok(s) if !s.is_empty() => s,
        _ => return, // no batteries (desktop) — nothing to log this tick
    };
    let snap = &snaps[0];

    // Ensure we have an active battery session for this power source.
    let on_ac = snap.status.power_state & BATTERY_POWER_ON_LINE != 0;
    let _ = storage.ensure_battery_session(snap, on_ac);

    // Build the full batch of readings for this tick, then insert in one
    // transaction (single fsync) to keep disk I/O minimal.
    let mut batch: Vec<ReadingInput> = Vec::with_capacity(16);

    if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY && snap.info.full_charged_capacity > 0 {
        let pct = snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0;
        batch.push(ReadingInput {
            name: "battery_percent".into(),
            unit: "%",
            category: "battery",
            hw_source: Some("ioctl"),
            value: pct,
        });
    }
    if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY {
        batch.push(ReadingInput {
            name: "battery_capacity".into(),
            unit: "mWh",
            category: "battery",
            hw_source: Some("ioctl"),
            value: snap.status.capacity as f64,
        });
    }
    if snap.status.voltage != BATTERY_UNKNOWN_CAPACITY {
        batch.push(ReadingInput {
            name: "battery_voltage".into(),
            unit: "V",
            category: "battery",
            hw_source: Some("ioctl"),
            value: snap.status.voltage as f64 / 1000.0,
        });
    }
    if snap.status.rate != BATTERY_UNKNOWN_RATE {
        batch.push(ReadingInput {
            name: "battery_rate".into(),
            unit: "W",
            category: "battery",
            hw_source: Some("ioctl"),
            value: snap.status.rate as f64 / 1000.0,
        });
    }

    // Power channels via EMI. read_all_emi takes its own pair of samples
    // internally (200ms apart) so each tick produces fresh wattage values.
    if let Ok(readings) = power::read_all_emi(Duration::from_millis(200)) {
        for r in readings {
            for ch in r.channels {
                batch.push(ReadingInput {
                    name: format!("power_{}", sanitize(&ch.name)),
                    unit: "W",
                    category: "power",
                    hw_source: Some("emi"),
                    value: ch.watts,
                });
            }
        }
    }

    let _ = storage.log_readings_batch(&batch);

    // ── Periodic health snapshot ─────────────────────────────────────────────
    if last_health.elapsed() >= Duration::from_secs(60) {
        let _ = storage.log_health_snapshot(snap);
        *last_health = Instant::now();
    }
}

/// Make a sensor name SQLite-friendly: lowercase, ASCII, underscores.
fn sanitize(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
