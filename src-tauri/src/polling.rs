// Background polling loop.
//
// Runs on a dedicated low-priority thread, reads sensors at the intervals
// the scope doc recommends, and logs everything into SQLite.
//
// Adaptive cadence:
//   AC + display on   →   5s
//   battery + display on →  10s
//   AC + display off  →  15s
//   battery + display off → 30s
//
// AC/DC transitions are debounced: a change must persist for ≥ 10s
// before the battery session is rotated. That way a flicker or a brief
// charger disconnect doesn't create a new row in battery_sessions.

use std::thread;
use std::time::{Duration, Instant};

use crate::battery::{self, BATTERY_POWER_ON_LINE, BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE};
use crate::events;
use crate::gpu::GpuQuery;
use crate::power;
use crate::processes::{self, ProcessSample, ProcessorTotals};
use crate::storage::{self, AppPowerRow, ReadingInput};

/// Debounce window: an AC/DC change must hold for this long before the
/// session rotates.
const AC_DC_DEBOUNCE: Duration = Duration::from_secs(10);

/// State the polling loop carries between ticks.
struct PollState {
    last_health: Instant,
    /// Previous process snapshot, used as the baseline for CPU delta.
    prev_processes: Option<Vec<ProcessSample>>,
    prev_cpu_totals: Option<ProcessorTotals>,
    /// PDH-based GPU utilization query, held across ticks so each sample
    /// reports the delta since the previous collect.
    gpu: Option<GpuQuery>,
    /// Power state that's been committed to the database as the current
    /// battery session. None on startup.
    committed_on_ac: Option<bool>,
    /// A tentative state that differs from committed — waiting to see if
    /// it holds long enough to be the new committed state.
    pending_transition: Option<(bool, Instant)>,
}

impl PollState {
    fn new() -> Self {
        Self {
            last_health: Instant::now() - Duration::from_secs(3600),
            prev_processes: None,
            prev_cpu_totals: None,
            gpu: GpuQuery::new(),
            committed_on_ac: None,
            pending_transition: None,
        }
    }

    /// How long to sleep until the next tick. Depends on current power
    /// source and display state — we slow down dramatically when the
    /// laptop is on battery with the display off so we don't become the
    /// thing we're trying to measure.
    fn next_interval(&self) -> Duration {
        let on_ac = self.committed_on_ac.unwrap_or(true);
        let display_on = events::is_display_on();
        match (on_ac, display_on) {
            (true, true) => Duration::from_secs(5),
            (true, false) => Duration::from_secs(15),
            (false, true) => Duration::from_secs(10),
            (false, false) => Duration::from_secs(30),
        }
    }
}

/// Spawn the polling thread. Runs until the process exits.
pub fn spawn() {
    thread::Builder::new()
        .name("bugjuice-poll".into())
        .spawn(run)
        .expect("failed to spawn polling thread");
}

fn run() {
    let mut state = PollState::new();
    loop {
        tick(&mut state);
        thread::sleep(state.next_interval());
    }
}

fn tick(state: &mut PollState) {
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

    // ── Debounced AC/DC session rotation ─────────────────────────────────────
    let on_ac = snap.status.power_state & BATTERY_POWER_ON_LINE != 0;
    let should_commit = match state.committed_on_ac {
        None => {
            // First tick after startup — commit the initial state so the
            // rest of the run has a session to write readings under.
            true
        }
        Some(committed) if committed == on_ac => {
            // Current state matches committed. Any pending transition
            // has resolved itself (flicker).
            state.pending_transition = None;
            false
        }
        Some(_) => {
            // Differs from committed. Check/update pending.
            match state.pending_transition {
                Some((pending, since)) if pending == on_ac => {
                    since.elapsed() >= AC_DC_DEBOUNCE
                }
                _ => {
                    state.pending_transition = Some((on_ac, Instant::now()));
                    false
                }
            }
        }
    };

    if should_commit {
        let _ = storage.ensure_battery_session(snap, on_ac);
        state.committed_on_ac = Some(on_ac);
        state.pending_transition = None;
    }

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

    // Power channels via EMI. Pick out CPU package and GPU totals for the
    // per-process attribution below.
    let mut cpu_package_watts: Option<f64> = None;
    let mut gpu_package_watts: Option<f64> = None;
    if let Ok(readings) = power::read_all_emi(Duration::from_millis(200)) {
        for r in readings {
            for ch in &r.channels {
                batch.push(ReadingInput {
                    name: format!("power_{}", sanitize(&ch.name)),
                    unit: "W",
                    category: "power",
                    hw_source: Some("emi"),
                    value: ch.watts,
                });
                let upper = ch.name.to_ascii_uppercase();
                // GPU: either explicit "GPU" (Snapdragon) or PP1 (Intel iGPU).
                if upper == "GPU" || upper.contains("PP1") {
                    gpu_package_watts = Some(ch.watts);
                }
            }
            // CPU package: prefer Intel-style PKG if present, else sum
            // Snapdragon-style CPU_CLUSTER_* channels.
            if let Some(pkg) = r
                .channels
                .iter()
                .find(|c| c.name.to_ascii_uppercase().contains("PKG"))
            {
                cpu_package_watts = Some(pkg.watts);
            } else {
                let cluster_sum: f64 = r
                    .channels
                    .iter()
                    .filter(|c| c.name.to_ascii_uppercase().contains("CPU_CLUSTER"))
                    .map(|c| c.watts)
                    .sum();
                if cluster_sum > 0.0 {
                    cpu_package_watts = Some(cluster_sum);
                }
            }
        }
    }

    let _ = storage.log_readings_batch(&batch);

    // ── Per-process CPU + GPU power attribution ─────────────────────────────
    log_app_power(state, storage, cpu_package_watts, gpu_package_watts);

    // ── Periodic health snapshot ─────────────────────────────────────────────
    if state.last_health.elapsed() >= Duration::from_secs(60) {
        let _ = storage.log_health_snapshot(snap);
        state.last_health = Instant::now();
    }
}

/// Take a process + GPU snapshot, diff against the previous, attribute
/// the measured CPU and GPU package wattage proportionally, and write
/// rows to app_power. Skips the very first tick (no baseline yet).
fn log_app_power(
    state: &mut PollState,
    storage: &storage::Storage,
    cpu_package_watts: Option<f64>,
    gpu_package_watts: Option<f64>,
) {
    let curr_procs = match processes::snapshot_processes() {
        Ok(p) => p,
        Err(_) => return,
    };
    let curr_totals = match processes::snapshot_processor_totals() {
        Ok(t) => t,
        Err(_) => return,
    };

    // PDH GPU sample: PID → summed utilization percent (0..100+).
    let gpu_map = state
        .gpu
        .as_mut()
        .map(|g| g.sample())
        .unwrap_or_default();

    if let (Some(prev_procs), Some(prev_totals)) =
        (state.prev_processes.as_ref(), state.prev_cpu_totals.as_ref())
    {
        let deltas = processes::compute_deltas(prev_procs, &curr_procs, prev_totals, &curr_totals);

        // We want to log anything with CPU *or* GPU activity. Start with
        // the CPU-sorted top 50, then fold in any GPU-using PIDs we might
        // have missed.
        let mut seen_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut rows: Vec<AppPowerRow> = Vec::with_capacity(64);

        for d in deltas.iter().take(50) {
            seen_pids.insert(d.pid);
            let cpu_w = cpu_package_watts.map(|w| d.share * w);
            // GPU watts = (process_percent / 100) × GPU package watts.
            // Clamp the fraction at 1.0 — parallel engines can push the
            // raw sum above 100%, but the physical GPU chip isn't doing
            // more work than it's drawing power for.
            let gpu_pct = gpu_map.get(&d.pid).copied().unwrap_or(0.0);
            let gpu_frac = (gpu_pct / 100.0).clamp(0.0, 1.0);
            let gpu_w = gpu_package_watts.map(|w| gpu_frac * w);

            let total_w = match (cpu_w, gpu_w) {
                (Some(c), Some(g)) => Some(c + g),
                (Some(c), None) => Some(c),
                (None, Some(g)) => Some(g),
                _ => None,
            };

            rows.push(AppPowerRow {
                process_name: d.name.clone(),
                cpu_watts: cpu_w,
                gpu_watts: gpu_w,
                disk_watts: None,
                net_watts: None,
                total_watts: total_w,
            });
        }

        // Pick up GPU-only processes that aren't in the CPU top 50.
        for (&pid, &pct) in &gpu_map {
            if seen_pids.contains(&pid) || pct < 1.0 {
                continue;
            }
            // Use the name from the current process snapshot, if we can
            // find it. Otherwise fall back to pid_NNNN.
            let name = curr_procs
                .iter()
                .find(|p| p.pid == pid)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| format!("pid_{pid}"));
            let gpu_frac = (pct / 100.0).clamp(0.0, 1.0);
            let gpu_w = gpu_package_watts.map(|w| gpu_frac * w);
            rows.push(AppPowerRow {
                process_name: name,
                cpu_watts: None,
                gpu_watts: gpu_w,
                disk_watts: None,
                net_watts: None,
                total_watts: gpu_w,
            });
        }

        let _ = storage.log_app_power_batch(&rows);
    }

    state.prev_processes = Some(curr_procs);
    state.prev_cpu_totals = Some(curr_totals);
}

/// Make a sensor name SQLite-friendly: lowercase, ASCII, underscores.
fn sanitize(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
