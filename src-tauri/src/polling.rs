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
    BATTERY_UNKNOWN_RATE, BATTERY_UNKNOWN_VOLTAGE,
};

static LAST_BATTERY_RATE_W: AtomicU64 = AtomicU64::new(0);

pub fn last_battery_rate_w() -> f64 {
    f64::from_bits(LAST_BATTERY_RATE_W.load(AtomicOrdering::Relaxed))
}
use crate::events;
use crate::gpu::{GpuQuery, NvmlPower};
use crate::power;
use crate::processes::{self, ProcessEnergySnapshot, ProcessSample, ProcessorTotals};
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
    /// NVML power reader for NVIDIA discrete GPUs. Used as a fallback when
    /// EMI doesn't expose a GPU power channel.
    nvml: Option<NvmlPower>,
    /// Power state that's been committed to the database as the current
    /// battery session. None on startup.
    committed_on_ac: Option<bool>,
    /// A tentative state that differs from committed — waiting to see if
    /// it holds long enough to be the new committed state.
    pending_transition: Option<(bool, Instant)>,
    /// Whether the charge-limit notification has already been sent this
    /// charge cycle (reset when percent drops below threshold - 2%,
    /// or when the user changes the threshold in Settings).
    charge_limit_notified: bool,
    low_battery_notified: bool,
    /// Track the last-seen threshold values so we can reset the debounce
    /// flags when the user changes them in Settings.
    last_charge_limit: f64,
    last_low_threshold: f64,
    /// Timestamp (unix secs) of the last periodic summary notification.
    last_summary_ts: i64,
    /// Battery percent when the last summary was sent (for delta calc).
    summary_start_percent: Option<f64>,
    /// Last hour boundary at which we aggregated readings into hourly_stats.
    last_aggregated_hour: i64,
    /// Last day boundary at which we aggregated hourly_stats into daily_stats.
    last_aggregated_day: i64,
    /// When old readings/app_power were last pruned.
    last_prune: Instant,
    /// Previous battery capacity and timestamp, used to compute a fallback
    /// rate when the IOCTL returns BATTERY_UNKNOWN_RATE (e.g., some HP laptops).
    prev_capacity_mwh: Option<u32>,
    prev_capacity_ts: Option<Instant>,
    /// Previous per-process energy snapshots for delta computation.
    prev_energy: Option<std::collections::HashMap<u32, ProcessEnergySnapshot>>,
    prev_energy_ts: Option<Instant>,
    /// LibreHardwareMonitor WMI reader state (x64 only).
    lhm: crate::lhm::LhmState,
    /// Measured idle system power in watts. None until first measurement.
    idle_baseline_w: Option<f64>,
    /// Samples collected during low-activity periods for idle baseline.
    idle_samples: Vec<f64>,
}

impl PollState {
    fn new() -> Self {
        Self {
            last_health: Instant::now().checked_sub(Duration::from_secs(3600)).unwrap_or(Instant::now()),
            prev_processes: None,
            prev_cpu_totals: None,
            gpu: GpuQuery::new(),
            nvml: NvmlPower::new(),
            committed_on_ac: None,
            pending_transition: None,
            charge_limit_notified: false,
            low_battery_notified: false,
            last_charge_limit: 0.0,
            last_low_threshold: 0.0,
            last_summary_ts: 0,
            summary_start_percent: None,
            last_aggregated_hour: 0,
            last_aggregated_day: 0,
            // Set far in the past so the first tick triggers a prune.
            last_prune: Instant::now().checked_sub(Duration::from_secs(86400 * 2)).unwrap_or(Instant::now()),
            prev_capacity_mwh: None,
            prev_capacity_ts: None,
            lhm: crate::lhm::LhmState::new(),
            prev_energy: None,
            prev_energy_ts: None,
            idle_baseline_w: None,
            idle_samples: Vec::new(),
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

    // ── Computed rate fallback ─────────────────────────────────────────────
    // On hardware that returns BATTERY_UNKNOWN_RATE (e.g., some HP laptops),
    // estimate the rate from the capacity delta between successive polls.
    // Only used as a fallback — the actual IOCTL rate is always preferred.
    let computed_rate_w: Option<f64> = if snap.status.rate == BATTERY_UNKNOWN_RATE
        && snap.status.capacity != BATTERY_UNKNOWN_CAPACITY
    {
        match (state.prev_capacity_mwh, state.prev_capacity_ts) {
            (Some(prev_cap), Some(prev_ts)) => {
                let elapsed = prev_ts.elapsed().as_secs_f64();
                if elapsed >= 5.0 {
                    let delta = snap.status.capacity as f64 - prev_cap as f64;
                    Some(delta * 3.6 / elapsed) // mWh delta → watts
                } else {
                    None
                }
            }
            _ => None,
        }
    } else {
        None
    };
    // Advance the capacity baseline when we've computed a rate (or first
    // reading to establish baseline). When IOCTL provides a real rate,
    // always track so we have a baseline ready if it stops reporting.
    if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY {
        let advance = if snap.status.rate == BATTERY_UNKNOWN_RATE {
            state.prev_capacity_mwh.is_none() || computed_rate_w.is_some()
        } else {
            true
        };
        if advance {
            state.prev_capacity_mwh = Some(snap.status.capacity);
            state.prev_capacity_ts = Some(Instant::now());
        }
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
    if snap.status.voltage != BATTERY_UNKNOWN_VOLTAGE {
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
    } else if let Some(rate) = computed_rate_w {
        batch.push(ReadingInput {
            name: "battery_rate".into(),
            unit: "W",
            category: "battery",
            hw_source: Some("computed"),
            value: rate,
        });
    }

    // Power channels via the bugjuice-service named pipe. The service runs
    // as SYSTEM and reads EMI devices that require elevation, plus LHM data
    // from the helper process. Readings with oem="LHM" come from the LHM
    // helper. If the service isn't running, read_emi() returns an empty vec —
    // we just skip power channels and the rest of the app still works.
    let mut cpu_package_watts: Option<f64> = None;
    let mut gpu_package_watts: Option<f64> = None;
    if let Ok(readings) = crate::pipe_client::read_emi() {
        for r in readings {
            let is_lhm = r.oem.eq_ignore_ascii_case("lhm");
            let prefix = if is_lhm { "lhm_" } else { "" };
            let source = if is_lhm { "lhm" } else { "emi" };

            for ch in &r.channels {
                batch.push(ReadingInput {
                    name: format!("power_{}{}", prefix, sanitize(&ch.name)),
                    unit: "W",
                    category: "power",
                    hw_source: Some(source),
                    value: ch.watts,
                });
                let upper = ch.name.to_ascii_uppercase();
                // GPU: either explicit "GPU" (Snapdragon) or PP1 (Intel iGPU).
                if upper == "GPU" || upper.contains("PP1") {
                    gpu_package_watts = Some(ch.watts);
                }
            }
            // CPU core power: prefer PP0 (cores-only) over PKG (cores + uncore + iGPU).
            // PKG over-attributes because it includes memory controller, system agent,
            // and ring bus power that no individual process caused.
            if let Some(pp0) = r
                .channels
                .iter()
                .find(|c| {
                    let u = c.name.to_ascii_uppercase();
                    u.contains("PP0") || u == "CORE"
                })
            {
                cpu_package_watts = Some(pp0.watts);
            } else if let Some(pkg) = r
                .channels
                .iter()
                .find(|c| c.name.to_ascii_uppercase().contains("PKG"))
            {
                // Fallback: PKG minus PP1 (iGPU) as approximate cores-only.
                let pp1 = gpu_package_watts.unwrap_or(0.0);
                cpu_package_watts = Some((pkg.watts - pp1).max(0.0));
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

    // If EMI didn't provide GPU watts, try NVML (NVIDIA discrete GPUs).
    if gpu_package_watts.is_none() {
        if let Some(ref nvml) = state.nvml {
            gpu_package_watts = nvml.read_total_watts();
        }
    }

    // ── LibreHardwareMonitor supplement (x64 only) ──────────────────────────
    // If LHM is running, use its RAPL readings to fill gaps EMI didn't cover.
    if let Some(lhm_data) = crate::lhm::read_power(&mut state.lhm) {
        if cpu_package_watts.is_none() {
            if let Some(w) = lhm_data.cpu_cores_w.or(lhm_data.cpu_package_w) {
                cpu_package_watts = Some(w);
                batch.push(ReadingInput {
                    name: "power_lhm_cpu".into(),
                    unit: "W",
                    category: "power",
                    hw_source: Some("lhm"),
                    value: w,
                });
            }
        }
        if gpu_package_watts.is_none() {
            if let Some(w) = lhm_data.gpu_power_w {
                gpu_package_watts = Some(w);
                batch.push(ReadingInput {
                    name: "power_lhm_gpu".into(),
                    unit: "W",
                    category: "power",
                    hw_source: Some("lhm"),
                    value: w,
                });
            }
        }
    }

    let _ = storage.log_readings_batch(&batch);

    // ── Per-process CPU + GPU power attribution ─────────────────────────────
    log_app_power(state, storage, cpu_package_watts, gpu_package_watts);

    // ── Store battery rate for confidence score + command fallback ──────────
    if snap.status.rate != BATTERY_UNKNOWN_RATE {
        LAST_BATTERY_RATE_W.store(
            (snap.status.rate as f64 / 1000.0).to_bits(),
            AtomicOrdering::Relaxed,
        );
    } else if let Some(rate) = computed_rate_w {
        LAST_BATTERY_RATE_W.store(rate.to_bits(), AtomicOrdering::Relaxed);
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
            let rate_known = snap.status.rate != BATTERY_UNKNOWN_RATE || computed_rate_w.is_some();
            let rate_w = if snap.status.rate != BATTERY_UNKNOWN_RATE {
                snap.status.rate.unsigned_abs() as f64 / 1000.0
            } else if let Some(rate) = computed_rate_w {
                rate.abs()
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

    // ── Power plan auto-switching ───────────────────────────────────────────
    {
        let pct = if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY
            && snap.info.full_charged_capacity > 0
        {
            snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0
        } else {
            50.0 // safe default — won't trigger thresholds
        };
        crate::power_plan::auto_switch(pct, on_ac);
    }

    // ── Tiered aggregation ───────────────────────────────────────────────────
    check_aggregation(state, storage);

    // ── Data pruning (once every 24h) ───────────────────────────────────────
    if state.last_prune.elapsed() >= Duration::from_secs(86400) {
        let days = crate::commands::data_retention_days();
        let _ = storage.prune_old_data(days);
        state.last_prune = Instant::now();
    }

    // ── Periodic health snapshot ─────────────────────────────────────────────
    if state.last_health.elapsed() >= Duration::from_secs(60) {
        let _ = storage.log_health_snapshot(snap);
        state.last_health = Instant::now();
    }
}

/// Take a process + GPU snapshot, diff against the previous, attribute
/// power proportionally, and write rows to app_power.
///
/// Attribution strategy (in priority order):
///   1. ProcessEnergyValues (Windows E3) — energy deltas as relative weights
///   2. Fallback: CPU time share × PP0 watts (if energy API unavailable)
///
/// GPU: PDH utilization × measured GPU watts (EMI or NVML), idle subtracted.
/// Disk/network: energy deltas as relative weights, scaled to estimated component power.
///
/// All per-process values are capped so the sum never exceeds battery discharge.
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

    // ── Idle baseline collection ────────────────────────────────────────
    // During low-activity periods, sample battery rate to estimate idle power.
    if state.idle_baseline_w.is_none() {
        if let (Some(prev_totals), Some(_)) =
            (state.prev_cpu_totals.as_ref(), state.prev_processes.as_ref())
        {
            let idle_frac = processes::idle_fraction(prev_totals, &curr_totals);
            let battery_w = last_battery_rate_w();
            // Only sample when system is mostly idle and on battery (negative rate).
            if idle_frac > 0.90 && battery_w < -0.5 {
                state.idle_samples.push(battery_w.abs());
                if state.idle_samples.len() >= 5 {
                    // Use median to be robust against transient spikes.
                    state.idle_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    state.idle_baseline_w = Some(state.idle_samples[state.idle_samples.len() / 2]);
                    log::info!(
                        "idle baseline measured: {:.1}W ({} samples)",
                        state.idle_baseline_w.unwrap(),
                        state.idle_samples.len()
                    );
                }
            }
        }
    }

    // ── Energy snapshot for ProcessEnergyValues ─────────────────────────
    let pids: Vec<u32> = curr_procs.iter().map(|p| p.pid).collect();
    let curr_energy = processes::snapshot_energy_values(&pids);
    let now = Instant::now();

    // Build PID → name lookup.
    let pid_names: std::collections::HashMap<u32, String> = curr_procs
        .iter()
        .map(|p| (p.pid, p.name.clone()))
        .collect();

    if let (Some(prev_procs), Some(prev_totals)) =
        (state.prev_processes.as_ref(), state.prev_cpu_totals.as_ref())
    {
        let use_energy_api = processes::energy_api_available()
            && state.prev_energy.is_some()
            && !curr_energy.is_empty();

        let battery_w = last_battery_rate_w();
        let idle_w = state.idle_baseline_w.unwrap_or(3.0); // conservative default
        let dynamic_budget = if battery_w < -0.5 {
            (battery_w.abs() - idle_w).max(0.5)
        } else {
            // On AC: no battery constraint, use measured component totals.
            cpu_package_watts.unwrap_or(0.0) + gpu_package_watts.unwrap_or(0.0) + 2.0
        };

        let mut rows: Vec<AppPowerRow> = Vec::with_capacity(64);
        let mut seen_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();

        if use_energy_api {
            // ── Energy-based attribution (preferred) ────────────────────
            let prev_e = state.prev_energy.as_ref().unwrap();
            let energy_deltas =
                processes::compute_energy_deltas(prev_e, &curr_energy, &pid_names);

            let total_cpu_cycles: u64 = energy_deltas.iter().map(|d| d.cpu_cycles_delta).sum();
            let total_disk_energy: u64 = energy_deltas.iter().map(|d| d.disk_energy_delta).sum();
            let total_net_energy: u64 = energy_deltas.iter().map(|d| d.network_energy_delta).sum();

            let cpu_total_w = cpu_package_watts.unwrap_or(0.0);
            // Estimate disk ~2W and network ~1W when active.
            let disk_total_w = if total_disk_energy > 0 { 2.0 } else { 0.0 };
            let net_total_w = if total_net_energy > 0 { 1.0 } else { 0.0 };

            for d in energy_deltas.iter().take(80) {
                seen_pids.insert(d.pid);

                // CPU: energy cycle share × measured CPU watts.
                let cpu_share = if total_cpu_cycles > 0 {
                    d.cpu_cycles_delta as f64 / total_cpu_cycles as f64
                } else {
                    0.0
                };
                let cpu_w = cpu_share * cpu_total_w;

                // GPU: still use PDH utilization × measured GPU power.
                let gpu_pct = gpu_map.get(&d.pid).copied().unwrap_or(0.0);
                let gpu_frac = (gpu_pct / 100.0).clamp(0.0, 1.0);
                let gpu_w = gpu_package_watts.map(|w| gpu_frac * w);

                // Disk: energy share × estimated disk power.
                let disk_share = if total_disk_energy > 0 {
                    d.disk_energy_delta as f64 / total_disk_energy as f64
                } else {
                    0.0
                };
                let disk_w = disk_share * disk_total_w;

                // Network: energy share × estimated net power.
                let net_share = if total_net_energy > 0 {
                    d.network_energy_delta as f64 / total_net_energy as f64
                } else {
                    0.0
                };
                let net_w = net_share * net_total_w;

                let total_w = cpu_w + gpu_w.unwrap_or(0.0) + disk_w + net_w;

                rows.push(AppPowerRow {
                    process_name: d.name.clone(),
                    cpu_watts: Some(cpu_w),
                    gpu_watts: gpu_w,
                    disk_watts: if disk_w > 0.001 { Some(disk_w) } else { None },
                    net_watts: if net_w > 0.001 { Some(net_w) } else { None },
                    total_watts: Some(total_w),
                });
            }
        } else {
            // ── Fallback: CPU time share × PP0 watts ────────────────────
            let deltas = processes::compute_deltas(prev_procs, &curr_procs, prev_totals, &curr_totals);

            for d in deltas.iter().take(50) {
                seen_pids.insert(d.pid);
                let cpu_w = cpu_package_watts.map(|w| d.share * w);
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
        }

        // Pick up GPU-only processes not already seen.
        for (&pid, &pct) in &gpu_map {
            if seen_pids.contains(&pid) || pct < 1.0 {
                continue;
            }
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

        // ── Capping: scale down if sum exceeds dynamic budget ───────────
        let raw_sum: f64 = rows.iter().filter_map(|r| r.total_watts).sum();
        if raw_sum > dynamic_budget && raw_sum > 0.01 {
            let scale = dynamic_budget / raw_sum;
            for r in &mut rows {
                r.cpu_watts = r.cpu_watts.map(|v| v * scale);
                r.gpu_watts = r.gpu_watts.map(|v| v * scale);
                r.disk_watts = r.disk_watts.map(|v| v * scale);
                r.net_watts = r.net_watts.map(|v| v * scale);
                r.total_watts = r.total_watts.map(|v| v * scale);
            }
        }

        let _ = storage.log_app_power_batch(&rows);
    }

    state.prev_processes = Some(curr_procs);
    state.prev_cpu_totals = Some(curr_totals);
    state.prev_energy = Some(curr_energy);
    state.prev_energy_ts = Some(now);
}

fn check_notifications(state: &mut PollState, snap: &battery::BatterySnapshot, on_ac: bool) {
    let prefs = crate::commands::notification_prefs().lock().unwrap().clone();
    let pct = if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY && snap.info.full_charged_capacity > 0 {
        snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0
    } else {
        return;
    };

    // Reset debounce flags when the user changes thresholds in Settings,
    // so a new notification can fire at the new value.
    if (prefs.charge_limit - state.last_charge_limit).abs() > 0.1 {
        state.charge_limit_notified = false;
        state.last_charge_limit = prefs.charge_limit;
    }
    if (prefs.low_threshold - state.last_low_threshold).abs() > 0.1 {
        state.low_battery_notified = false;
        state.last_low_threshold = prefs.low_threshold;
    }

    // On the very first tick (last_charge_limit was 0), just record the
    // current thresholds without firing. This prevents a spurious
    // notification on app startup when the battery is already above the
    // limit. The notification will fire on the NEXT tick that crosses
    // the threshold (i.e., when the battery actually reaches it).
    if state.last_charge_limit == 0.0 && state.last_low_threshold == 0.0 {
        state.last_charge_limit = prefs.charge_limit;
        state.last_low_threshold = prefs.low_threshold;
        // If already above charge limit on startup, mark as notified so
        // we don't fire immediately — wait for a fresh crossing.
        if on_ac && pct >= prefs.charge_limit {
            state.charge_limit_notified = true;
        }
        if !on_ac && pct <= prefs.low_threshold {
            state.low_battery_notified = true;
        }
        return;
    }

    // Charge limit: fire when battery crosses UP past the limit while charging
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

    // Low battery: fire when battery drops BELOW the threshold while discharging
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
                last_battery_rate_w() // fallback to computed rate
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
