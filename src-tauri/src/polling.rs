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

use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::thread;
use std::time::{Duration, Instant};

use crate::battery::{
    self, BATTERY_CHARGING, BATTERY_DISCHARGING, BATTERY_POWER_ON_LINE, BATTERY_UNKNOWN_CAPACITY,
    BATTERY_UNKNOWN_RATE,
};

static LAST_BATTERY_RATE_W: AtomicU64 = AtomicU64::new(0);

pub fn last_battery_rate_w() -> f64 {
    f64::from_bits(LAST_BATTERY_RATE_W.load(AtomicOrdering::Relaxed))
}
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
    /// Whether the charge-limit notification has already been sent this
    /// charge cycle (reset when percent drops below threshold - 2%).
    charge_limit_notified: bool,
    /// Whether the low-battery notification has already been sent this
    /// discharge cycle (reset when percent rises above threshold + 2%).
    low_battery_notified: bool,
    /// Timestamp (unix secs) of the last periodic summary notification.
    last_summary_ts: i64,
    /// Battery percent when the last summary was sent (for delta calc).
    summary_start_percent: Option<f64>,
    /// Last hour boundary at which we aggregated readings into hourly_stats.
    last_aggregated_hour: i64,
    /// Last day boundary at which we aggregated hourly_stats into daily_stats.
    last_aggregated_day: i64,
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
            charge_limit_notified: false,
            low_battery_notified: false,
            last_summary_ts: 0,
            summary_start_percent: None,
            last_aggregated_hour: 0,
            last_aggregated_day: 0,
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

    // Power channels via EMI. Read directly — same code path as the CLI.
    // Pick out CPU package and GPU totals for the per-process attribution
    // below.
    let mut cpu_package_watts: Option<f64> = None;
    let mut gpu_package_watts: Option<f64> = None;
    if let Ok(readings) = power::read_all_emi(Duration::from_secs(1)) {
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

    // ── Store battery rate for confidence score ──────────────────────────────
    if snap.status.rate != BATTERY_UNKNOWN_RATE {
        LAST_BATTERY_RATE_W.store(
            (snap.status.rate as f64 / 1000.0).to_bits(),
            AtomicOrdering::Relaxed,
        );
    }

    // ── Tray tooltip + menu info update ────────────────────────────────────
    if let Some(handle) = crate::app_handle() {
        if let Some(tray) = handle.tray_by_id("main") {
            let pct = if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY && snap.info.full_charged_capacity > 0 {
                snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0
            } else {
                0.0
            };
            let state_str = if on_ac { "plugged in" } else { "on battery" };
            let tooltip = format!("BugJuice \u{2014} {pct:.0}% ({state_str})");
            let _ = tray.set_tooltip(Some(&tooltip));

            // Build info-item texts from the battery snapshot.
            let charging = snap.status.power_state & BATTERY_CHARGING != 0;
            let discharging = snap.status.power_state & BATTERY_DISCHARGING != 0;
            let rate_known = snap.status.rate != BATTERY_UNKNOWN_RATE;
            let rate_w = if rate_known {
                snap.status.rate.unsigned_abs() as f64 / 1000.0
            } else {
                0.0
            };

            let state_text = if discharging && rate_known {
                format!("Discharging at {rate_w:.1} W")
            } else if charging && rate_known {
                format!("Charging at {rate_w:.1} W")
            } else if on_ac && !discharging {
                if pct >= 99.0 {
                    "Fully charged".to_string()
                } else {
                    "Plugged in".to_string()
                }
            } else if rate_known && rate_w > 0.0 {
                format!("{rate_w:.1} W")
            } else {
                "Battery".to_string()
            };

            let eta_text = if let Some(secs) = snap.estimated_seconds {
                let hours = secs as f64 / 3600.0;
                let label = crate::format_hours(hours);
                if discharging {
                    format!("~{label} remaining")
                } else if charging {
                    format!("~{label} to full")
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            // Update the disabled menu items via the global tray menu.
            if let Some(menu) = crate::tray_menu() {
                use tauri::menu::MenuItemKind;
                if let Some(MenuItemKind::MenuItem(item)) = menu.get("info_state") {
                    let _ = item.set_text(&state_text);
                }
                if let Some(MenuItemKind::MenuItem(item)) = menu.get("info_eta") {
                    let _ = item.set_text(&eta_text);
                }
            }
        }
    }

    // ── Notification checks ──────────────────────────────────────────────────
    check_notifications(state, snap, on_ac);

    // ── Tiered aggregation ───────────────────────────────────────────────────
    check_aggregation(state, storage);

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

fn check_notifications(state: &mut PollState, snap: &battery::BatterySnapshot, on_ac: bool) {
    let prefs = crate::commands::notification_prefs().lock().unwrap().clone();
    let pct = if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY && snap.info.full_charged_capacity > 0 {
        snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0
    } else {
        return;
    };

    // Charge limit
    if prefs.notify_charge && on_ac && pct >= prefs.charge_limit {
        if !state.charge_limit_notified {
            fire_notification(
                "Charge Limit Reached",
                &format!("Battery is at {pct:.0}%. Unplug to preserve battery health."),
            );
            state.charge_limit_notified = true;
        }
    } else if pct < prefs.charge_limit - 2.0 {
        state.charge_limit_notified = false;
    }

    // Low battery
    if prefs.notify_low && !on_ac && pct <= prefs.low_threshold {
        if !state.low_battery_notified {
            fire_notification(
                "Low Battery",
                &format!("Battery is at {pct:.0}%. Plug in soon."),
            );
            state.low_battery_notified = true;
        }
    } else if pct > prefs.low_threshold + 2.0 {
        state.low_battery_notified = false;
    }

    // Periodic summary
    if prefs.summary_enabled {
        if prefs.summary_only_on_battery && on_ac {
            // Skip summaries while plugged in
            return;
        }

        let now_ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let interval_secs = prefs.summary_interval_min as i64 * 60;

        // Initialize on first tick
        if state.last_summary_ts == 0 {
            state.last_summary_ts = now_ts;
            state.summary_start_percent = Some(pct);
            return;
        }

        if now_ts - state.last_summary_ts >= interval_secs {
            let rate_w = if snap.status.rate != BATTERY_UNKNOWN_RATE {
                snap.status.rate as f64 / 1000.0
            } else {
                0.0
            };

            let mut lines: Vec<String> = Vec::new();

            if prefs.summary_show_rate {
                let abs = rate_w.abs();
                if rate_w > 0.0 {
                    lines.push(format!("Charging at {abs:.1} W"));
                } else if rate_w < 0.0 {
                    lines.push(format!("Draining at {abs:.1} W"));
                } else {
                    lines.push("Idle".into());
                }
            }

            if prefs.summary_show_delta {
                if let Some(start_pct) = state.summary_start_percent {
                    let delta = pct - start_pct;
                    let sign = if delta >= 0.0 { "+" } else { "" };
                    lines.push(format!("{sign}{delta:.1}% since last summary"));
                }
            }

            if prefs.summary_show_eta {
                if let Some(est) = snap.estimated_seconds {
                    if est > 0 && est < 999999 {
                        let h = est / 3600;
                        let m = (est % 3600) / 60;
                        if rate_w > 0.0 {
                            lines.push(format!("~{h}h {m:02}m to full"));
                        } else {
                            lines.push(format!("~{h}h {m:02}m remaining"));
                        }
                    }
                }
            }

            if prefs.summary_show_top_app {
                if let Some(storage) = crate::storage::global() {
                    if let Ok(apps) = storage.read_recent_app_power() {
                        if let Some(top) = apps.first() {
                            lines.push(format!(
                                "Top app: {} ({:.1} W)",
                                top.process_name, top.total_watts
                            ));
                        }
                    }
                }
            }

            if !lines.is_empty() {
                fire_notification(
                    &format!("Battery: {pct:.0}%"),
                    &lines.join("\n"),
                );
            }

            state.last_summary_ts = now_ts;
            state.summary_start_percent = Some(pct);
        }
    }
}

pub fn fire_notification(title: &str, body: &str) {
    if let Some(handle) = crate::app_handle() {
        use tauri_plugin_notification::NotificationExt;
        match handle.notification().builder().title(title).body(body).show() {
            Ok(_) => println!("[notification] sent: {title}"),
            Err(e) => println!("[notification] FAILED: {title} — {e}"),
        }
    } else {
        println!("[notification] no app handle yet, skipping: {title}");
    }
}

fn check_aggregation(state: &mut PollState, storage: &storage::Storage) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let current_hour = (now / 3600) * 3600;
    let current_day = (now / 86400) * 86400;

    if state.last_aggregated_hour == 0 {
        state.last_aggregated_hour = current_hour;
        state.last_aggregated_day = current_day;
        return;
    }

    if current_hour > state.last_aggregated_hour {
        let prev_hour = current_hour - 3600;
        let _ = storage.aggregate_hour(prev_hour);
        state.last_aggregated_hour = current_hour;
    }

    if current_day > state.last_aggregated_day {
        let prev_day = current_day - 86400;
        let _ = storage.aggregate_day(prev_day);
        state.last_aggregated_day = current_day;
    }
}

/// Make a sensor name SQLite-friendly: lowercase, ASCII, underscores.
fn sanitize(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
