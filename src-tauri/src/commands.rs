// Tauri command surface — the bridge between the React frontend and the
// Rust backend. Each #[tauri::command] function is callable from JavaScript
// via `invoke('command_name', { args })`.
//
// All DTO types use #[serde(rename_all = "camelCase")] so they line up
// with the TypeScript interfaces in src/types.ts.

use serde::{Deserialize, Serialize};
use std::sync::{Mutex as StdMutex, OnceLock};

use crate::battery::{
    self, BATTERY_CHARGING, BATTERY_CRITICAL, BATTERY_DISCHARGING, BATTERY_POWER_ON_LINE,
    BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE, BATTERY_UNKNOWN_VOLTAGE, BatterySnapshot,
};
use crate::power::{self, EmiReading, PowerChannel};
use crate::storage;

// ─── BatteryStatus ──────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatteryStatusDto {
    pub percent: f64,
    pub capacity_mwh: u32,
    pub full_charge_mwh: u32,
    pub design_mwh: u32,
    pub voltage_v: f64,
    pub rate_w: f64,
    pub power_state: String,
    pub on_ac: bool,
    pub temp_c: Option<f64>,
    pub eta_minutes: Option<i64>,
    pub eta_label: String,
    pub chemistry: String,
    pub cycle_count: u32,
    pub wear_percent: f64,
    pub manufacturer: String,
    pub device_name: String,
}

fn build_battery_status(snap: &BatterySnapshot) -> BatteryStatusDto {
    let info = &snap.info;
    let status = &snap.status;

    let full = info.full_charged_capacity.max(1) as f64;
    let percent = if status.capacity != BATTERY_UNKNOWN_CAPACITY {
        status.capacity as f64 / full * 100.0
    } else {
        0.0
    };
    let voltage_v = if status.voltage != BATTERY_UNKNOWN_VOLTAGE {
        status.voltage as f64 / 1000.0
    } else {
        0.0
    };
    let rate_w = if status.rate != BATTERY_UNKNOWN_RATE {
        status.rate as f64 / 1000.0
    } else {
        0.0
    };

    let on_ac = status.power_state & BATTERY_POWER_ON_LINE != 0;
    let charging = status.power_state & BATTERY_CHARGING != 0;
    let discharging = status.power_state & BATTERY_DISCHARGING != 0;
    let critical = status.power_state & BATTERY_CRITICAL != 0;

    let power_state = if critical {
        "critical"
    } else if charging {
        "charging"
    } else if discharging {
        "discharging"
    } else if on_ac && percent >= 99.5 {
        "full"
    } else {
        "idle"
    };

    let (eta_minutes, eta_label) = compute_eta(rate_w, status, info, on_ac, percent);

    let chemistry = String::from_utf8_lossy(&info.chemistry)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let raw_health = info.full_charged_capacity as f64 / info.designed_capacity.max(1) as f64 * 100.0;
    let wear_percent = (100.0 - raw_health).max(0.0);

    BatteryStatusDto {
        percent,
        capacity_mwh: status.capacity,
        full_charge_mwh: info.full_charged_capacity,
        design_mwh: info.designed_capacity,
        voltage_v,
        rate_w,
        power_state: power_state.to_string(),
        on_ac,
        temp_c: snap.temperature_c,
        eta_minutes,
        eta_label,
        chemistry,
        cycle_count: info.cycle_count,
        wear_percent,
        manufacturer: snap.manufacturer.clone().unwrap_or_default(),
        device_name: snap.device_name.clone().unwrap_or_default(),
    }
}

fn compute_eta(
    rate_w: f64,
    status: &battery::BatteryStatus,
    info: &battery::BatteryInformation,
    on_ac: bool,
    percent: f64,
) -> (Option<i64>, String) {
    if rate_w.abs() < 0.5 || status.rate == BATTERY_UNKNOWN_RATE {
        if on_ac && percent > 99.0 {
            return (None, "Fully charged".into());
        }
        return (None, "Rate not reported".into());
    }

    if rate_w > 0.0 {
        // charging
        let to_full = info.full_charged_capacity.saturating_sub(status.capacity);
        if to_full == 0 {
            return (None, "Fully charged".into());
        }
        let hours = to_full as f64 / status.rate as f64;
        let minutes = (hours * 60.0).round() as i64;
        let h = minutes / 60;
        let m = minutes % 60;
        let dur = if h > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{m} min")
        };
        (Some(minutes), format!("about {dur} to full at this rate"))
    } else {
        // discharging
        let hours = status.capacity as f64 / -status.rate as f64;
        let minutes = (hours * 60.0).round() as i64;
        let h = minutes / 60;
        let m = minutes % 60;
        let dur = if h > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{m} min")
        };
        (Some(minutes), format!("about {dur} left at this rate"))
    }
}

#[tauri::command]
pub fn get_battery_status() -> Result<BatteryStatusDto, String> {
    let snaps = battery::snapshot_all()?;
    let snap = snaps
        .into_iter()
        .next()
        .ok_or_else(|| "no battery present".to_string())?;
    Ok(build_battery_status(&snap))
}

// ─── PowerReading ────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PowerChannelDto {
    pub name: String,
    pub watts: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PowerReadingDto {
    pub wall_input_w: Option<f64>,
    pub system_draw_w: Option<f64>,
    pub cpu_package_w: Option<f64>,
    pub gpu_w: Option<f64>,
    pub dram_w: Option<f64>,
    pub source: String,
    /// Raw per-channel values as reported by EMI, for diagnostics.
    pub channels: Vec<PowerChannelDto>,
}

#[tauri::command]
pub fn get_power_reading() -> Result<PowerReadingDto, String> {
    // Read the EMI counters directly — same code path the CLI uses and
    // proves works on Snapdragon X without elevation. Use a 1-second
    // window; the Qualcomm EMI driver sometimes returns identical counter
    // values for sub-second deltas.
    match power::read_all_emi(std::time::Duration::from_secs(1)) {
        Ok(readings) if !readings.is_empty() => Ok(power_dto_from_emi(&readings[0])),
        Ok(_) => Ok(PowerReadingDto {
            wall_input_w: None,
            system_draw_w: None,
            cpu_package_w: None,
            gpu_w: None,
            dram_w: None,
            source: "EMI: enumeration returned 0 devices".to_string(),
            channels: vec![],
        }),
        Err(e) => Ok(PowerReadingDto {
            wall_input_w: None,
            system_draw_w: None,
            cpu_package_w: None,
            gpu_w: None,
            dram_w: None,
            source: format!("EMI error: {e}"),
            channels: vec![],
        }),
    }
}

fn power_dto_from_emi(r: &EmiReading) -> PowerReadingDto {
    let mut cpu: Vec<&PowerChannel> = Vec::new();
    let mut gpu: Vec<&PowerChannel> = Vec::new();
    let mut dram: Vec<&PowerChannel> = Vec::new();
    let mut inputs: Vec<&PowerChannel> = Vec::new();
    let mut system: Vec<&PowerChannel> = Vec::new();
    for c in &r.channels {
        let n = c.name.to_ascii_uppercase();
        if n.contains("PP1") {
            gpu.push(c);
        } else if n.contains("DRAM") {
            dram.push(c);
        } else if n.contains("PKG") || n.contains("PP0") || n.contains("CPU") {
            cpu.push(c);
        } else if n.contains("GPU") {
            gpu.push(c);
        } else if n.contains("PSU") || n.contains("USBC") {
            inputs.push(c);
        } else if n == "SYS" || n.contains("SOC") || n.contains("PLATFORM") || n.contains("PSYS") {
            system.push(c);
        }
    }

    let pkg_value: Option<f64> = cpu
        .iter()
        .find(|c| c.name.to_ascii_uppercase().contains("PKG"))
        .map(|c| c.watts);
    let cpu_total: Option<f64> = if cpu.is_empty() {
        None
    } else {
        Some(pkg_value.unwrap_or_else(|| cpu.iter().map(|c| c.watts).sum()))
    };
    let gpu_total: Option<f64> = if gpu.is_empty() {
        None
    } else {
        Some(gpu.iter().map(|c| c.watts).sum())
    };
    let dram_total: Option<f64> = if dram.is_empty() {
        None
    } else {
        Some(dram.iter().map(|c| c.watts).sum())
    };
    let sys_total: Option<f64> = if system.is_empty() {
        None
    } else {
        Some(system.iter().map(|c| c.watts).sum())
    };
    let input_total: Option<f64> = if inputs.is_empty() {
        None
    } else {
        Some(inputs.iter().map(|c| c.watts).sum())
    };

    let source = format!(
        "EMI v{} — {} {}",
        r.version,
        if r.oem.is_empty() { "(unknown OEM)" } else { r.oem.as_str() },
        r.model
    );

    let channels = r
        .channels
        .iter()
        .map(|c| PowerChannelDto {
            name: c.name.clone(),
            watts: c.watts,
        })
        .collect();

    PowerReadingDto {
        wall_input_w: input_total,
        system_draw_w: sys_total,
        cpu_package_w: cpu_total,
        gpu_w: gpu_total,
        dram_w: dram_total,
        source,
        channels,
    }
}

// ─── App power ───────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppPowerDto {
    pub pid: u32,        // we don't have a stable PID per row in the DB; we use 0 here
    pub name: String,
    pub cpu_w: f64,
    pub gpu_w: f64,
    pub disk_w: f64,
    pub net_w: f64,
    pub total_w: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TopAppsResponse {
    pub apps: Vec<AppPowerDto>,
    pub confidence_percent: f64,
    pub battery_discharge_w: f64,
}

#[tauri::command]
pub fn get_top_apps() -> Result<TopAppsResponse, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rows = storage
        .read_recent_app_power()
        .map_err(|e| format!("read app_power: {e}"))?;

    let apps: Vec<AppPowerDto> = rows
        .into_iter()
        .map(|r| AppPowerDto {
            pid: 0,
            name: r.process_name,
            cpu_w: r.cpu_watts,
            gpu_w: r.gpu_watts,
            disk_w: 0.0,
            net_w: 0.0,
            total_w: r.total_watts,
        })
        .collect();

    let total_attributed: f64 = apps.iter().map(|a| a.total_w).sum();

    // Get latest battery discharge rate for confidence calculation
    let battery_w = crate::polling::last_battery_rate_w();
    let confidence = if battery_w.abs() > 0.5 {
        (total_attributed / battery_w.abs() * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    Ok(TopAppsResponse {
        apps,
        confidence_percent: confidence,
        battery_discharge_w: battery_w,
    })
}

// ─── History ─────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPointDto {
    pub ts: i64,
    pub percent: f64,
    pub rate_w: f64,
}

#[tauri::command]
pub fn get_battery_history(minutes: i64) -> Result<Vec<HistoryPointDto>, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rows = storage
        .read_recent_history(minutes * 60)
        .map_err(|e| format!("read history: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|r| HistoryPointDto {
            ts: r.ts,
            percent: r.percent,
            rate_w: r.rate_w,
        })
        .collect())
}

// ─── Sessions ────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatterySessionDto {
    pub id: i64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub start_percent: f64,
    pub end_percent: Option<f64>,
    pub start_capacity: i64,
    pub end_capacity: Option<i64>,
    pub avg_drain_w: Option<f64>,
    pub on_ac: bool,
}

#[tauri::command]
pub fn get_battery_sessions() -> Result<Vec<BatterySessionDto>, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rows = storage
        .read_battery_sessions()
        .map_err(|e| format!("read battery sessions: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|r| BatterySessionDto {
            id: r.id,
            started_at: r.started_at,
            ended_at: r.ended_at,
            start_percent: r.start_percent.unwrap_or(0.0),
            end_percent: r.end_percent,
            start_capacity: r.start_capacity.unwrap_or(0),
            end_capacity: r.end_capacity,
            avg_drain_w: r.avg_drain_watts,
            on_ac: r.on_ac,
        })
        .collect())
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SleepSessionDto {
    pub id: i64,
    pub sleep_at: i64,
    pub wake_at: Option<i64>,
    pub pre_capacity: i64,
    pub post_capacity: Option<i64>,
    pub drain_mwh: Option<i64>,
    pub drain_percent: Option<f64>,
    pub drain_rate_mw: Option<f64>,
    pub drips_percent: Option<f64>,
    pub verdict: Option<String>,
}

fn classify_verdict(rate_mw: Option<f64>) -> Option<String> {
    let r = rate_mw?;
    Some(
        if r < 50.0 {
            "excellent"
        } else if r < 200.0 {
            "normal"
        } else if r < 500.0 {
            "high"
        } else {
            "very-high"
        }
        .to_string(),
    )
}

#[tauri::command]
pub fn get_sleep_sessions() -> Result<Vec<SleepSessionDto>, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rows = storage
        .read_sleep_sessions()
        .map_err(|e| format!("read sleep sessions: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|r| SleepSessionDto {
            id: r.id,
            sleep_at: r.sleep_at,
            wake_at: r.wake_at,
            pre_capacity: r.pre_capacity.unwrap_or(0),
            post_capacity: r.post_capacity,
            drain_mwh: r.drain_mwh,
            drain_percent: r.drain_percent,
            drain_rate_mw: r.drain_rate_mw,
            drips_percent: r.drips_percent,
            verdict: classify_verdict(r.drain_rate_mw),
        })
        .collect())
}

// ─── Health ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshotDto {
    pub ts: i64,
    pub design_capacity: i64,
    pub full_charge_capacity: i64,
    pub cycle_count: i64,
    pub wear_percent: f64,
}

#[tauri::command]
pub fn get_health_history() -> Result<Vec<HealthSnapshotDto>, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rows = storage
        .read_health_history()
        .map_err(|e| format!("read health: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|r| HealthSnapshotDto {
            ts: r.ts,
            design_capacity: r.design_capacity,
            full_charge_capacity: r.full_charge_capacity,
            cycle_count: r.cycle_count.unwrap_or(0),
            wear_percent: r.wear_percent.unwrap_or(0.0),
        })
        .collect())
}

// ─── Component power history (for the Components page stacked chart) ──────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentHistoryPointDto {
    pub ts: i64,
    pub cpu: f64,
    pub gpu: f64,
    pub dram: f64,
    pub other: f64,
}

fn categorize_channel(name: &str) -> &'static str {
    let n = name.to_ascii_uppercase();
    if n.contains("PP1") || n.contains("GPU") {
        "gpu"
    } else if n.contains("DRAM") {
        "dram"
    } else if n.contains("PKG") || n.contains("PP0") || n.contains("CPU") {
        "cpu"
    } else if n == "SYS" || n.contains("SOC") || n.contains("PLATFORM") || n.contains("PSYS") {
        // Treat whole-system channels as "other" for the stacked chart.
        // The chart is a component breakdown, not a total.
        "other"
    } else if n.contains("PSU") || n.contains("USBC") {
        // Wall input — exclude from the component stack.
        "skip"
    } else {
        "other"
    }
}

#[tauri::command]
pub fn get_component_history(minutes: i64) -> Result<Vec<ComponentHistoryPointDto>, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let grouped = storage
        .read_power_history(minutes * 60)
        .map_err(|e| format!("read power history: {e}"))?;

    // Bucket per-second, categorize per channel, then sum within each
    // category. The chart wants { ts, cpu, gpu, dram, other } rows.
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<i64, (f64, f64, f64, f64)> = BTreeMap::new();
    for (name, samples) in grouped {
        let cat = categorize_channel(&name);
        if cat == "skip" {
            continue;
        }
        for (ts, watts) in samples {
            let slot = buckets.entry(ts).or_insert((0.0, 0.0, 0.0, 0.0));
            match cat {
                "cpu" => slot.0 += watts,
                "gpu" => slot.1 += watts,
                "dram" => slot.2 += watts,
                _ => slot.3 += watts,
            }
        }
    }

    Ok(buckets
        .into_iter()
        .map(|(ts, (cpu, gpu, dram, other))| ComponentHistoryPointDto {
            ts,
            cpu,
            gpu,
            dram,
            other,
        })
        .collect())
}

// ─── Session detail (drill-down for the Sessions page) ─────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetailDto {
    pub history: Vec<HistoryPointDto>,
    pub min_rate_w: f64,
    pub max_rate_w: f64,
    pub avg_rate_w: f64,
    pub total_energy_mwh: f64,
    pub duration_sec: i64,
}

#[tauri::command]
pub fn get_session_detail(session_id: i64) -> Result<SessionDetailDto, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rows = storage
        .read_session_history(session_id)
        .map_err(|e| format!("read session detail: {e}"))?;
    if rows.is_empty() {
        return Ok(SessionDetailDto {
            history: vec![],
            min_rate_w: 0.0,
            max_rate_w: 0.0,
            avg_rate_w: 0.0,
            total_energy_mwh: 0.0,
            duration_sec: 0,
        });
    }
    let mut min_r = f64::INFINITY;
    let mut max_r = f64::NEG_INFINITY;
    let mut sum_r = 0.0;
    for r in &rows {
        if r.rate_w < min_r {
            min_r = r.rate_w;
        }
        if r.rate_w > max_r {
            max_r = r.rate_w;
        }
        sum_r += r.rate_w;
    }
    let avg = sum_r / rows.len() as f64;
    let duration = rows.last().unwrap().ts - rows.first().unwrap().ts;
    let total_energy = avg.abs() * (duration as f64 / 3600.0) * 1000.0;

    let history = rows
        .into_iter()
        .map(|r| HistoryPointDto {
            ts: r.ts,
            percent: r.percent,
            rate_w: r.rate_w,
        })
        .collect();

    Ok(SessionDetailDto {
        history,
        min_rate_w: if min_r.is_finite() { min_r } else { 0.0 },
        max_rate_w: if max_r.is_finite() { max_r } else { 0.0 },
        avg_rate_w: avg,
        total_energy_mwh: total_energy,
        duration_sec: duration,
    })
}

// ─── App power summary ──────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppPowerSummaryDto {
    pub name: String,
    pub avg_watts: f64,
    pub max_watts: f64,
    pub sample_count: i64,
}

// ─── Unified timeline (Sessions page) ─────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedTimelineDto {
    pub history: Vec<HistoryPointDto>,
    pub battery_sessions: Vec<BatterySessionDto>,
    pub sleep_sessions: Vec<SleepSessionDto>,
    pub component_history: Vec<ComponentHistoryPointDto>,
    pub app_power_summary: Vec<AppPowerSummaryDto>,
}

#[tauri::command]
pub fn get_unified_timeline(start_ts: i64, end_ts: i64) -> Result<UnifiedTimelineDto, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;

    let history = storage
        .read_history_range(start_ts, end_ts)
        .map_err(|e| format!("read history range: {e}"))?
        .into_iter()
        .map(|r| HistoryPointDto {
            ts: r.ts,
            percent: r.percent,
            rate_w: r.rate_w,
        })
        .collect();

    let battery_sessions = storage
        .read_battery_sessions_range(start_ts, end_ts)
        .map_err(|e| format!("read battery sessions range: {e}"))?
        .into_iter()
        .map(|r| BatterySessionDto {
            id: r.id,
            started_at: r.started_at,
            ended_at: r.ended_at,
            start_percent: r.start_percent.unwrap_or(0.0),
            end_percent: r.end_percent,
            start_capacity: r.start_capacity.unwrap_or(0),
            end_capacity: r.end_capacity,
            avg_drain_w: r.avg_drain_watts,
            on_ac: r.on_ac,
        })
        .collect();

    let sleep_sessions = storage
        .read_sleep_sessions_range(start_ts, end_ts)
        .map_err(|e| format!("read sleep sessions range: {e}"))?
        .into_iter()
        .map(|r| SleepSessionDto {
            id: r.id,
            sleep_at: r.sleep_at,
            wake_at: r.wake_at,
            pre_capacity: r.pre_capacity.unwrap_or(0),
            post_capacity: r.post_capacity,
            drain_mwh: r.drain_mwh,
            drain_percent: r.drain_percent,
            drain_rate_mw: r.drain_rate_mw,
            drips_percent: r.drips_percent,
            verdict: classify_verdict(r.drain_rate_mw),
        })
        .collect();

    // Component power history — same categorization logic as get_component_history
    let component_history = {
        let grouped = storage
            .read_power_history_range(start_ts, end_ts)
            .map_err(|e| format!("read power history range: {e}"))?;
        use std::collections::BTreeMap;
        let mut buckets: BTreeMap<i64, (f64, f64, f64, f64)> = BTreeMap::new();
        for (name, samples) in grouped {
            let cat = categorize_channel(&name);
            if cat == "skip" {
                continue;
            }
            for (ts, watts) in samples {
                let slot = buckets.entry(ts).or_insert((0.0, 0.0, 0.0, 0.0));
                match cat {
                    "cpu" => slot.0 += watts,
                    "gpu" => slot.1 += watts,
                    "dram" => slot.2 += watts,
                    _ => slot.3 += watts,
                }
            }
        }
        buckets
            .into_iter()
            .map(|(ts, (cpu, gpu, dram, other))| ComponentHistoryPointDto {
                ts,
                cpu,
                gpu,
                dram,
                other,
            })
            .collect()
    };

    // App power summary — top processes by average wattage in the range
    let app_power_summary = storage
        .read_app_power_summary(start_ts, end_ts)
        .map_err(|e| format!("read app power summary: {e}"))?
        .into_iter()
        .map(|r| AppPowerSummaryDto {
            name: r.process_name,
            avg_watts: r.avg_watts,
            max_watts: r.max_watts,
            sample_count: r.sample_count,
        })
        .collect();

    Ok(UnifiedTimelineDto {
        history,
        battery_sessions,
        sleep_sessions,
        component_history,
        app_power_summary,
    })
}

// ─── DB stats (for a debug page or footer) ─────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DbStatsDto {
    pub readings: i64,
    pub battery_sessions: i64,
    pub sleep_sessions: i64,
    pub health_snapshots: i64,
    pub sensors: i64,
    pub app_power: i64,
}

#[tauri::command]
pub fn get_db_stats() -> Result<DbStatsDto, String> {
    let storage = storage::global().ok_or_else(|| "storage not initialized".to_string())?;
    let rc = storage
        .row_counts()
        .map_err(|e| format!("row counts: {e}"))?;
    Ok(DbStatsDto {
        readings: rc.readings,
        battery_sessions: rc.battery_sessions,
        sleep_sessions: rc.sleep_sessions,
        health_snapshots: rc.health_snapshots,
        sensors: rc.sensors,
        app_power: rc.app_power,
    })
}

// ─── Accent color ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_accent_color() -> Result<String, String> {
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
    };
    use windows::core::w;

    unsafe {
        let mut hkey = Default::default();
        let err = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\DWM"),
            None,
            KEY_READ,
            &mut hkey,
        );
        if err.0 != 0 {
            return Err(format!("RegOpenKeyExW failed: {}", err.0));
        }

        let mut data = [0u8; 4];
        let mut data_size = 4u32;
        let mut data_type = REG_DWORD;
        let err = RegQueryValueExW(
            hkey,
            w!("AccentColor"),
            None,
            Some(&mut data_type),
            Some(data.as_mut_ptr()),
            Some(&mut data_size),
        );
        if err.0 != 0 {
            return Err(format!("RegQueryValueExW failed: {}", err.0));
        }

        // AccentColor is stored as 0xAABBGGRR (alpha, blue, green, red)
        let r = data[0];
        let g = data[1];
        let b = data[2];
        Ok(format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

// ─── Notification preferences ──────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPrefs {
    pub notify_charge: bool,
    pub charge_limit: f64,
    pub notify_low: bool,
    pub low_threshold: f64,
    pub notify_sleep_drain: bool,
    // Periodic summary
    pub summary_enabled: bool,
    pub summary_interval_min: u32,
    pub summary_only_on_battery: bool,
    pub summary_show_rate: bool,
    pub summary_show_eta: bool,
    pub summary_show_delta: bool,
    pub summary_show_top_app: bool,
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            notify_charge: true,
            charge_limit: 80.0,
            notify_low: true,
            low_threshold: 20.0,
            notify_sleep_drain: true,
            summary_enabled: true,
            summary_interval_min: 15,
            summary_only_on_battery: false,
            summary_show_rate: true,
            summary_show_eta: true,
            summary_show_delta: true,
            summary_show_top_app: true,
        }
    }
}

static NOTIFICATION_PREFS: OnceLock<StdMutex<NotificationPrefs>> = OnceLock::new();

pub fn notification_prefs() -> &'static StdMutex<NotificationPrefs> {
    NOTIFICATION_PREFS.get_or_init(|| StdMutex::new(NotificationPrefs::default()))
}

#[tauri::command]
pub fn set_notification_prefs(prefs: NotificationPrefs) -> Result<(), String> {
    *notification_prefs().lock().unwrap() = prefs;
    Ok(())
}

// ─── Autostart ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn enable_autostart(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().enable().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn disable_autostart(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().disable().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn is_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}
