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
        // Fallback: use the computed rate from the polling thread (capacity
        // delta over time). Returns 0.0 if no computed rate is available yet.
        crate::polling::last_battery_rate_w()
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
    if rate_w.abs() < 0.5 {
        if on_ac && percent > 99.0 {
            return (None, "Fully charged".into());
        }
        return (None, "Rate not reported".into());
    }

    // Use rate_w (watts) for all ETA math. This works whether the rate came
    // from the IOCTL directly or from the computed capacity-delta fallback.
    let rate_mw = rate_w * 1000.0; // convert W → mW for mWh capacity math

    if rate_w > 0.0 {
        // charging
        let to_full = info.full_charged_capacity.saturating_sub(status.capacity);
        if to_full == 0 {
            return (None, "Fully charged".into());
        }
        let hours = to_full as f64 / rate_mw;
        let minutes = (hours * 60.0).round() as i64;
        let h = minutes / 60;
        let m = minutes % 60;
        let dur = if h > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{m} min")
        };
        let suffix = if status.rate == BATTERY_UNKNOWN_RATE { " (estimated)" } else { "" };
        (Some(minutes), format!("about {dur} to full at this rate{suffix}"))
    } else {
        // discharging
        let hours = status.capacity as f64 / -rate_mw;
        let minutes = (hours * 60.0).round() as i64;
        let h = minutes / 60;
        let m = minutes % 60;
        let dur = if h > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{m} min")
        };
        let suffix = if status.rate == BATTERY_UNKNOWN_RATE { " (estimated)" } else { "" };
        (Some(minutes), format!("about {dur} left at this rate{suffix}"))
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
    // Read EMI data from the bugjuice-service via named pipe. The service
    // runs as SYSTEM and handles the privileged EMI reads. The service may
    // return multiple readings (EMI + LHM helper), so we merge them all.
    match crate::pipe_client::read_emi() {
        Ok(readings) if !readings.is_empty() => Ok(power_dto_merged(&readings)),
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

/// Merge all readings from the pipe (EMI + LHM helper) into one DTO.
/// LHM channels are prefixed with `power_lhm_` so the frontend can detect
/// enhanced mode. LHM values fill gaps where EMI has no data.
fn power_dto_merged(readings: &[EmiReading]) -> PowerReadingDto {
    // Partition into EMI vs LHM readings by OEM field.
    let mut emi_readings: Vec<&EmiReading> = Vec::new();
    let mut lhm_readings: Vec<&EmiReading> = Vec::new();
    for r in readings {
        if r.oem.eq_ignore_ascii_case("lhm") {
            lhm_readings.push(r);
        } else {
            emi_readings.push(r);
        }
    }

    let emi_dto = emi_readings.first().map(|r| power_dto_from_emi(r));
    let lhm_dto = lhm_readings.first().map(|r| power_dto_from_emi(r));

    // Build merged channels: EMI raw + LHM with prefix.
    let mut channels: Vec<PowerChannelDto> = Vec::new();
    for r in &emi_readings {
        for ch in &r.channels {
            channels.push(PowerChannelDto {
                name: ch.name.clone(),
                watts: ch.watts,
            });
        }
    }
    for r in &lhm_readings {
        for ch in &r.channels {
            channels.push(PowerChannelDto {
                name: format!("power_lhm_{}", ch.name),
                watts: ch.watts,
            });
        }
    }

    // Merge power values: EMI first, LHM fills gaps.
    let (cpu, gpu, dram, sys, input, base_source) = match (&emi_dto, &lhm_dto) {
        (Some(e), Some(l)) => (
            e.cpu_package_w.or(l.cpu_package_w),
            e.gpu_w.or(l.gpu_w),
            e.dram_w.or(l.dram_w),
            e.system_draw_w.or(l.system_draw_w),
            e.wall_input_w.or(l.wall_input_w),
            e.source.clone(),
        ),
        (Some(e), None) => (
            e.cpu_package_w, e.gpu_w, e.dram_w, e.system_draw_w, e.wall_input_w,
            e.source.clone(),
        ),
        (None, Some(l)) => (
            l.cpu_package_w, l.gpu_w, l.dram_w, l.system_draw_w, l.wall_input_w,
            l.source.clone(),
        ),
        (None, None) => (None, None, None, None, None, "no data".to_string()),
    };

    let source = if lhm_dto.is_some() {
        format!("{} + LibreHardwareMonitor", base_source)
    } else {
        base_source
    };

    PowerReadingDto {
        wall_input_w: input,
        system_draw_w: sys,
        cpu_package_w: cpu,
        gpu_w: gpu,
        dram_w: dram,
        source,
        channels,
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
    pub system_overhead_w: f64,
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
            disk_w: r.disk_watts,
            net_w: r.net_watts,
            total_w: r.total_watts,
        })
        .collect();

    let total_attributed: f64 = apps.iter().map(|a| a.total_w).sum();

    // Battery discharge rate (negative = discharging).
    let battery_w = crate::polling::last_battery_rate_w();
    let discharge = battery_w.abs();

    // System overhead = total battery drain minus what we attributed to apps.
    // This includes idle platform power (display, DRAM refresh, VRMs, etc.)
    // plus any measurement gap.
    let system_overhead = if discharge > 0.5 {
        (discharge - total_attributed).max(0.0)
    } else {
        0.0
    };

    let confidence = if discharge > 0.5 {
        ((total_attributed + system_overhead) / discharge * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    Ok(TopAppsResponse {
        apps,
        confidence_percent: confidence,
        battery_discharge_w: battery_w,
        system_overhead_w: system_overhead,
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

// ─── Charge speed tracking ────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChargeSpeedDto {
    pub current_rate_w: f64,
    pub max_rate_w: f64,
    pub avg_rate_w: f64,
    pub time_to_full_min: Option<i64>,
    pub eta_label: String,
    pub start_percent: f64,
    pub current_percent: f64,
}

#[tauri::command]
pub fn get_charge_speed() -> Result<ChargeSpeedDto, String> {
    let snap = battery::snapshot_all().and_then(|v| v.into_iter().next().ok_or_else(|| "no battery found".into())).map_err(|e| format!("battery: {e}"))?;
    let storage = storage::global().ok_or("storage not initialized")?;
    let session_id = storage.current_session_id();

    let rate_w = if snap.status.rate != BATTERY_UNKNOWN_RATE {
        snap.status.rate as f64 / 1000.0
    } else {
        crate::polling::last_battery_rate_w()
    };

    let (max_r, avg_r) = storage
        .read_session_charge_stats(session_id)
        .unwrap_or((0.0, 0.0));

    let percent = if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY
        && snap.info.full_charged_capacity > 0
    {
        snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0
    } else {
        0.0
    };

    let on_ac = snap.status.power_state & BATTERY_POWER_ON_LINE != 0;
    let (eta_min, eta_label) = compute_eta(rate_w, &snap.status, &snap.info, on_ac, percent);

    // Start percent comes from the current session row.
    let start_pct = storage
        .read_battery_sessions()
        .ok()
        .and_then(|sessions| {
            sessions
                .into_iter()
                .find(|s| s.id == session_id && s.on_ac)
                .and_then(|s| s.start_percent)
        })
        .unwrap_or(percent);

    Ok(ChargeSpeedDto {
        current_rate_w: rate_w.max(0.0),
        max_rate_w: max_r,
        avg_rate_w: avg_r,
        time_to_full_min: eta_min,
        eta_label,
        start_percent: start_pct,
        current_percent: percent,
    })
}

// ─── Charge habit scoring ─────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChargeHabitDto {
    pub score: u32,
    pub verdict: String,
    pub has_enough_data: bool,
    pub is_provisional: bool,
    pub data_days: f64,
    pub metrics: ChargeHabitMetricsDto,
    pub tips: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChargeHabitMetricsDto {
    pub avg_max_charge: f64,
    pub overcharge_pct: f64,
    pub deep_discharge_pct: f64,
    pub time_at_100_minutes: f64,
    pub charges_to_100: i64,
    pub discharges_below_20: i64,
}

#[tauri::command]
pub fn get_charge_habits() -> Result<ChargeHabitDto, String> {
    let storage = storage::global().ok_or("storage not initialized")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Look back 30 days — the scoring algorithm uses whatever sessions are
    // available, even if only 24 hours old.
    let since = now - 30 * 86400;
    let d = storage
        .read_charge_habit_data(since)
        .map_err(|e| format!("charge habits: {e}"))?;

    let total_sessions = d.charge_sessions + d.discharge_sessions;
    if total_sessions < 2 {
        return Ok(ChargeHabitDto {
            score: 0,
            verdict: String::new(),
            has_enough_data: false,
            is_provisional: true,
            data_days: 0.0,
            metrics: ChargeHabitMetricsDto {
                avg_max_charge: 0.0,
                overcharge_pct: 0.0,
                deep_discharge_pct: 0.0,
                time_at_100_minutes: 0.0,
                charges_to_100: 0,
                discharges_below_20: 0,
            },
            tips: Vec::new(),
        });
    }

    let data_days = match (d.oldest_ts, d.newest_ts) {
        (Some(a), Some(b)) => (b - a).max(0) as f64 / 86400.0,
        _ => 0.0,
    };

    // ── Scoring: start at 100, subtract penalties ──
    let mut penalties: Vec<(f64, &str)> = Vec::new();

    // Overcharge: charges ending > 80%
    if d.charge_sessions > 0 {
        let ratio = d.charges_above_80 as f64 / d.charge_sessions as f64;
        penalties.push((ratio * 30.0, "Charging above 80% wears the battery faster. Try setting BugJuice's charge-limit reminder to 80%."));
    }

    // Full charge: charges ending ≥ 99%
    if d.charge_sessions > 0 {
        let ratio = d.charges_to_100 as f64 / d.charge_sessions as f64;
        penalties.push((ratio * 15.0, "Topping off to 100% regularly adds stress. Unplugging at 80% can significantly extend lifespan."));
    }

    // Deep discharge: below 20%
    if d.discharge_sessions > 0 {
        let ratio = d.discharges_below_20 as f64 / d.discharge_sessions as f64;
        penalties.push((ratio * 20.0, "Draining below 20% stresses lithium-ion cells. Try plugging in sooner."));
    }

    // Critical discharge: below 10%
    if d.discharge_sessions > 0 {
        let ratio = d.discharges_below_10 as f64 / d.discharge_sessions as f64;
        penalties.push((ratio * 20.0, "Deep discharges below 10% cause accelerated wear. Avoid letting the battery get critical."));
    }

    // Time at 100%
    let hours_at_100 = d.time_at_100_secs as f64 / 3600.0;
    penalties.push((
        (hours_at_100 * 2.0).min(15.0),
        "Extended time at 100% while plugged in degrades capacity. Unplug once fully charged.",
    ));

    let total_penalty: f64 = penalties.iter().map(|(p, _)| *p).sum();
    let score = (100.0 - total_penalty).clamp(0.0, 100.0) as u32;

    let verdict = match score {
        85..=100 => "excellent",
        70..=84 => "good",
        50..=69 => "fair",
        _ => "poor",
    }
    .to_string();

    // Top 2 tips from the biggest penalties
    let mut sorted = penalties.clone();
    sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let tips: Vec<String> = sorted
        .iter()
        .filter(|(p, _)| *p > 1.0)
        .take(2)
        .map(|(_, msg)| msg.to_string())
        .collect();

    let overcharge_pct = if d.charge_sessions > 0 {
        d.charges_above_80 as f64 / d.charge_sessions as f64 * 100.0
    } else {
        0.0
    };
    let deep_discharge_pct = if d.discharge_sessions > 0 {
        d.discharges_below_20 as f64 / d.discharge_sessions as f64 * 100.0
    } else {
        0.0
    };

    Ok(ChargeHabitDto {
        score,
        verdict,
        has_enough_data: true,
        is_provisional: data_days < 7.0,
        data_days,
        metrics: ChargeHabitMetricsDto {
            avg_max_charge: d.avg_max_soc,
            overcharge_pct,
            deep_discharge_pct,
            time_at_100_minutes: d.time_at_100_secs as f64 / 60.0,
            charges_to_100: d.charges_to_100,
            discharges_below_20: d.discharges_below_20,
        },
        tips,
    })
}

// ─── Data retention / pruning ────────────────────────────────────────────

use std::sync::atomic::{AtomicU32, Ordering as RetentionOrdering};

static DATA_RETENTION_DAYS: AtomicU32 = AtomicU32::new(30);

pub fn data_retention_days() -> u32 {
    DATA_RETENTION_DAYS.load(RetentionOrdering::Relaxed)
}

#[tauri::command]
pub fn set_data_retention(days: u32) -> Result<(), String> {
    let days = days.clamp(7, 365);
    DATA_RETENTION_DAYS.store(days, RetentionOrdering::Relaxed);
    if let Some(s) = storage::global() {
        let _ = s.prune_old_data(days);
    }
    Ok(())
}

// ─── Component power history (for the Components page stacked chart) ──────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComponentHistoryPointDto {
    pub ts: i64,
    pub cpu: f64,
    pub gpu: f64,
    pub dram: f64,
    pub modem: f64,
    pub npu: f64,
    pub other: f64,
}

fn categorize_channel(name: &str) -> &'static str {
    let n = name.to_ascii_uppercase();
    // GPU: Intel iGPU (PP1) or explicit GPU channel
    if n.contains("PP1") || n.contains("GPU") {
        "gpu"
    // Modem / WWAN (Snapdragon X cellular)
    } else if n.contains("MODEM") || n.contains("WWAN") || n.contains("CELLULAR") {
        "modem"
    // NPU / DSP (Snapdragon X neural engine)
    } else if n.contains("NPU") || n.contains("DSP") || n.contains("NEURAL") {
        "npu"
    // Camera / ISP
    } else if n.contains("ISP") || n.contains("CAMERA") {
        "npu" // group with NPU for chart simplicity
    } else if n.contains("DRAM") {
        "dram"
    // CPU: cores, cache, per-core channels
    } else if n.contains("PKG") || n.contains("PP0") || n.contains("CPU")
        || n.contains("CORE") || n.contains("L3") || n.contains("LLC") || n.contains("CACHE")
    {
        "cpu"
    // Whole-system / SoC / interconnect
    } else if n == "SYS" || n.contains("SOC") || n.contains("PLATFORM") || n.contains("PSYS")
        || n.contains("FABRIC") || n.contains("NOC") || n.contains("INTERCONNECT")
    {
        "other"
    // Wall input — exclude from the component stack.
    } else if n.contains("PSU") || n.contains("USBC") || n.contains("USB_C") {
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
    // category. The chart wants { ts, cpu, gpu, dram, modem, npu, other }.
    use std::collections::BTreeMap;
    #[derive(Default, Clone, Copy)]
    struct Bucket { cpu: f64, gpu: f64, dram: f64, modem: f64, npu: f64, other: f64 }
    let mut buckets: BTreeMap<i64, Bucket> = BTreeMap::new();
    for (name, samples) in grouped {
        let cat = categorize_channel(&name);
        if cat == "skip" {
            continue;
        }
        for (ts, watts) in samples {
            let b = buckets.entry(ts).or_default();
            match cat {
                "cpu" => b.cpu += watts,
                "gpu" => b.gpu += watts,
                "dram" => b.dram += watts,
                "modem" => b.modem += watts,
                "npu" => b.npu += watts,
                _ => b.other += watts,
            }
        }
    }

    Ok(buckets
        .into_iter()
        .map(|(ts, b)| ComponentHistoryPointDto {
            ts,
            cpu: b.cpu,
            gpu: b.gpu,
            dram: b.dram,
            modem: b.modem,
            npu: b.npu,
            other: b.other,
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
    let duration = match (rows.first(), rows.last()) {
        (Some(first), Some(last)) => last.ts - first.ts,
        _ => 0,
    };
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
        avg_rate_w: if avg.is_finite() { avg } else { 0.0 },
        total_energy_mwh: if total_energy.is_finite() { total_energy } else { 0.0 },
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
    pub total_energy: f64,
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
        #[derive(Default, Clone, Copy)]
        struct B { cpu: f64, gpu: f64, dram: f64, modem: f64, npu: f64, other: f64 }
        let mut buckets: BTreeMap<i64, B> = BTreeMap::new();
        for (name, samples) in grouped {
            let cat = categorize_channel(&name);
            if cat == "skip" {
                continue;
            }
            for (ts, watts) in samples {
                let b = buckets.entry(ts).or_default();
                match cat {
                    "cpu" => b.cpu += watts,
                    "gpu" => b.gpu += watts,
                    "dram" => b.dram += watts,
                    "modem" => b.modem += watts,
                    "npu" => b.npu += watts,
                    _ => b.other += watts,
                }
            }
        }
        buckets
            .into_iter()
            .map(|(ts, b)| ComponentHistoryPointDto {
                ts,
                cpu: b.cpu,
                gpu: b.gpu,
                dram: b.dram,
                modem: b.modem,
                npu: b.npu,
                other: b.other,
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
            total_energy: r.total_energy,
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

/// Debug command: fires a test notification and returns what the polling
/// thread would currently check (prefs + battery state).
#[tauri::command]
pub fn test_notification() -> Result<String, String> {
    let prefs = notification_prefs().lock().unwrap().clone();
    let snap = crate::battery::snapshot_all()
        .map_err(|e| format!("battery snapshot: {e}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "no battery".to_string())?;

    let pct = if snap.status.capacity != crate::battery::BATTERY_UNKNOWN_CAPACITY
        && snap.info.full_charged_capacity > 0
    {
        snap.status.capacity as f64 / snap.info.full_charged_capacity as f64 * 100.0
    } else {
        0.0
    };
    let on_ac = snap.status.power_state & crate::battery::BATTERY_POWER_ON_LINE != 0;

    // Fire the test notification
    crate::polling::fire_notification(
        "BugJuice Test",
        &format!(
            "This is a test notification.\n\
             Battery: {pct:.1}% | On AC: {on_ac}\n\
             Charge limit: {:.0}% (notify: {})\n\
             Low threshold: {:.0}% (notify: {})",
            prefs.charge_limit, prefs.notify_charge,
            prefs.low_threshold, prefs.notify_low,
        ),
    );

    // Return debug info
    Ok(format!(
        "Fired test notification.\n\
         Battery: {pct:.1}% | on_ac: {on_ac}\n\
         charge_limit: {:.0}% (enabled: {}) → would fire: {}\n\
         low_threshold: {:.0}% (enabled: {}) → would fire: {}",
        prefs.charge_limit,
        prefs.notify_charge,
        prefs.notify_charge && on_ac && pct >= prefs.charge_limit,
        prefs.low_threshold,
        prefs.notify_low,
        prefs.notify_low && !on_ac && pct <= prefs.low_threshold,
    ))
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

// ─── Start minimized ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn set_start_minimized(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri::Manager;
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let flag = data_dir.join(".start-minimized");
    if enabled {
        std::fs::write(&flag, "1").map_err(|e| e.to_string())
    } else {
        if flag.exists() {
            std::fs::remove_file(&flag).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }
}

#[tauri::command]
pub async fn get_start_minimized(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri::Manager;
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(data_dir.join(".start-minimized").exists())
}

// ─── Export to JSON ──────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BugJuiceReport {
    exported_at: String,
    version: String,
    battery_status: BatteryStatusDto,
    health_history: Vec<HealthSnapshotDto>,
    charge_habits: ChargeHabitDto,
    battery_sessions: Vec<BatterySessionDto>,
    sleep_sessions: Vec<SleepSessionDto>,
}

#[tauri::command]
pub async fn export_report_json(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let snap = battery::snapshot_all().and_then(|v| v.into_iter().next().ok_or_else(|| "no battery found".into())).map_err(|e| format!("battery: {e}"))?;
    let status_dto = build_battery_status(&snap);

    let storage = storage::global().ok_or("storage not initialized")?;

    let health = storage
        .read_health_history()
        .map_err(|e| format!("health: {e}"))?
        .into_iter()
        .map(|r| HealthSnapshotDto {
            ts: r.ts,
            design_capacity: r.design_capacity,
            full_charge_capacity: r.full_charge_capacity,
            cycle_count: r.cycle_count.unwrap_or(0),
            wear_percent: r.wear_percent.unwrap_or(0.0),
        })
        .collect();

    let habits = get_charge_habits().unwrap_or_else(|_| ChargeHabitDto {
        score: 0,
        verdict: String::new(),
        has_enough_data: false,
        is_provisional: true,
        data_days: 0.0,
        metrics: ChargeHabitMetricsDto {
            avg_max_charge: 0.0,
            overcharge_pct: 0.0,
            deep_discharge_pct: 0.0,
            time_at_100_minutes: 0.0,
            charges_to_100: 0,
            discharges_below_20: 0,
        },
        tips: Vec::new(),
    });

    let sessions = get_battery_sessions().unwrap_or_default();
    let sleeps = get_sleep_sessions().unwrap_or_default();

    let report = BugJuiceReport {
        exported_at: chrono_now(),
        version: "1.0.0".to_string(),
        battery_status: status_dto,
        health_history: health,
        charge_habits: habits,
        battery_sessions: sessions,
        sleep_sessions: sleeps,
    };

    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("serialize: {e}"))?;

    // Show save-file dialog.
    let path = app
        .dialog()
        .file()
        .set_file_name("bugjuice-report.json")
        .add_filter("JSON", &["json"])
        .blocking_save_file();

    let Some(path) = path else {
        return Ok("cancelled".to_string());
    };

    std::fs::write(path.as_path().unwrap(), &json)
        .map_err(|e| format!("write: {e}"))?;

    Ok("exported".to_string())
}

// ─── Power plan auto-switching ───────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PowerPlanStatusDto {
    pub enabled: bool,
    pub low_threshold: u32,
    pub high_threshold: u32,
    pub active_scheme: String,
}

#[tauri::command]
pub fn get_power_plan_status() -> Result<PowerPlanStatusDto, String> {
    let (low, high) = crate::power_plan::thresholds();
    let active = crate::power_plan::get_active_scheme()
        .map(|g| crate::power_plan::scheme_name(&g).to_string())
        .unwrap_or_else(|| "unknown".into());
    Ok(PowerPlanStatusDto {
        enabled: crate::power_plan::is_enabled(),
        low_threshold: low,
        high_threshold: high,
        active_scheme: active,
    })
}

#[tauri::command]
pub fn set_power_plan_config(enabled: bool, low: u32, high: u32) -> Result<(), String> {
    crate::power_plan::set_enabled(enabled);
    crate::power_plan::set_thresholds(low, high);
    Ok(())
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // ISO-ish format from unix timestamp (no chrono dependency).
    let secs_per_day = 86400u64;
    let days = now / secs_per_day;
    let rem = now % secs_per_day;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    // Rough year/month/day from days-since-epoch.
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Simple Gregorian conversion. Good enough for a timestamp.
    let mut y = 1970;
    loop {
        let ylen = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if days < ylen {
            break;
        }
        days -= ylen;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut mo = 0;
    for &ml in &mdays {
        if days < ml {
            break;
        }
        days -= ml;
        mo += 1;
    }
    (y, mo + 1, days + 1)
}

// ─── Export to PDF ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn export_report_pdf(app: tauri::AppHandle) -> Result<String, String> {
    use printpdf::*;
    use tauri_plugin_dialog::DialogExt;

    let snap = battery::snapshot_all()
        .and_then(|v| v.into_iter().next().ok_or_else(|| "no battery found".into()))
        .map_err(|e| format!("battery: {e}"))?;
    let status = build_battery_status(&snap);

    let habits = get_charge_habits().unwrap_or_else(|_| ChargeHabitDto {
        score: 0,
        verdict: String::new(),
        has_enough_data: false,
        is_provisional: true,
        data_days: 0.0,
        metrics: ChargeHabitMetricsDto {
            avg_max_charge: 0.0,
            overcharge_pct: 0.0,
            deep_discharge_pct: 0.0,
            time_at_100_minutes: 0.0,
            charges_to_100: 0,
            discharges_below_20: 0,
        },
        tips: Vec::new(),
    });

    let sessions = get_battery_sessions().unwrap_or_default();
    let sleeps = get_sleep_sessions().unwrap_or_default();

    // ── Build PDF ───────────────────────────────────────────────────
    let (doc, page1, layer1) = PdfDocument::new(
        "BugJuice Battery Report",
        Mm(210.0),
        Mm(297.0),
        "Content",
    );
    let font = doc.add_builtin_font(BuiltinFont::Helvetica).map_err(|e| format!("font: {e}"))?;
    let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold).map_err(|e| format!("font: {e}"))?;
    let layer = doc.get_page(page1).get_layer(layer1);

    let black = Color::Rgb(Rgb::new(0.1, 0.1, 0.1, None));
    let gray = Color::Rgb(Rgb::new(0.4, 0.4, 0.4, None));
    let green = Color::Rgb(Rgb::new(0.13, 0.77, 0.37, None));
    let mut y = 270.0f32;

    // Title
    layer.set_fill_color(green);
    layer.use_text("BugJuice Battery Report", 22.0, Mm(20.0), Mm(y), &bold);
    y -= 8.0;
    layer.set_fill_color(gray.clone());
    layer.use_text(&format!("Generated {}", chrono_now()), 10.0, Mm(20.0), Mm(y), &font);
    y -= 14.0;

    // Battery Status
    layer.set_fill_color(black.clone());
    layer.use_text("Battery Status", 16.0, Mm(20.0), Mm(y), &bold);
    y -= 7.0;
    layer.set_fill_color(gray.clone());
    for line in &[
        format!("Charge: {:.1}%  |  Wear: {:.1}%  |  Cycles: {}", status.percent, status.wear_percent, status.cycle_count),
        format!("Rate: {:.2} W  |  Voltage: {:.2} V", status.rate_w, status.voltage_v),
        format!("Chemistry: {}  |  Manufacturer: {}  |  Model: {}", status.chemistry, status.manufacturer, status.device_name),
        format!("Designed: {} mWh  |  Full charge: {} mWh", status.design_mwh, status.full_charge_mwh),
    ] {
        layer.use_text(line, 10.0, Mm(20.0), Mm(y), &font);
        y -= 5.0;
    }
    y -= 6.0;

    // Charge Habits
    layer.set_fill_color(black.clone());
    layer.use_text("Charge Habits", 16.0, Mm(20.0), Mm(y), &bold);
    y -= 7.0;
    layer.set_fill_color(gray.clone());
    if habits.has_enough_data {
        layer.use_text(&format!("Score: {} / 100 -- {}", habits.score, habits.verdict), 11.0, Mm(20.0), Mm(y), &font);
        y -= 5.0;
        layer.use_text(&format!("Avg max charge: {:.0}%  |  Overcharge: {:.0}%  |  Deep discharge: {:.0}%",
            habits.metrics.avg_max_charge, habits.metrics.overcharge_pct, habits.metrics.deep_discharge_pct),
            10.0, Mm(20.0), Mm(y), &font);
        y -= 5.0;
        for tip in &habits.tips {
            layer.use_text(&format!("  - {tip}"), 9.0, Mm(20.0), Mm(y), &font);
            y -= 4.5;
        }
    } else {
        layer.use_text("Not enough data yet", 10.0, Mm(20.0), Mm(y), &font);
        y -= 5.0;
    }
    y -= 6.0;

    // Recent Sessions
    layer.set_fill_color(black.clone());
    layer.use_text("Recent Battery Sessions", 16.0, Mm(20.0), Mm(y), &bold);
    y -= 7.0;
    layer.set_fill_color(gray.clone());
    for s in sessions.iter().take(10) {
        let kind = if s.on_ac { "AC" } else { "Battery" };
        let drain = s.avg_drain_w.map(|w| format!("{w:.1}W avg")).unwrap_or_default();
        let pct = format!("{:.0}% -> {}", s.start_percent,
            s.end_percent.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "ongoing".into()));
        layer.use_text(&format!("{kind}  {pct}  {drain}"), 9.0, Mm(20.0), Mm(y), &font);
        y -= 4.5;
        if y < 20.0 { break; }
    }
    y -= 6.0;

    // Sleep Sessions
    if !sleeps.is_empty() && y > 40.0 {
        layer.set_fill_color(black);
        layer.use_text("Recent Sleep Sessions", 16.0, Mm(20.0), Mm(y), &bold);
        y -= 7.0;
        layer.set_fill_color(gray);
        for s in sleeps.iter().take(10) {
            let drain = s.drain_mwh.map(|m| format!("{m} mWh")).unwrap_or_default();
            let rate = s.drain_rate_mw.map(|r| format!("{r:.0} mW")).unwrap_or_default();
            let verdict = s.verdict.as_deref().unwrap_or("unknown");
            layer.use_text(&format!("Drain: {drain}  Rate: {rate}  ({verdict})"),
                9.0, Mm(20.0), Mm(y), &font);
            y -= 4.5;
            if y < 20.0 { break; }
        }
    }

    // Footer
    let footer_gray = Color::Rgb(Rgb::new(0.5, 0.5, 0.5, None));
    layer.set_fill_color(footer_gray);
    layer.use_text("Generated by BugJuice -- dudiebug.net/bugjuice", 8.0, Mm(20.0), Mm(10.0), &font);

    // ── Save via dialog ─────────────────────────────────────────────
    let bytes = doc.save_to_bytes().map_err(|e| format!("pdf save: {e}"))?;

    let path = app
        .dialog()
        .file()
        .set_file_name("bugjuice-report.pdf")
        .add_filter("PDF", &["pdf"])
        .blocking_save_file();

    let Some(path) = path else {
        return Ok("cancelled".to_string());
    };

    std::fs::write(path.as_path().unwrap(), &bytes)
        .map_err(|e| format!("write: {e}"))?;

    Ok("exported".to_string())
}

// ─── "Before I unplug" estimate ─────────────────────────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UnplugDrainEntry {
    pub name: String,
    pub watts: f64,
    pub est_hours: f64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UnplugEstimateDto {
    pub total_hours: f64,
    pub total_label: String,
    pub top_drains: Vec<UnplugDrainEntry>,
    pub system_overhead_w: f64,
}

#[tauri::command]
pub fn get_unplug_estimate() -> Result<UnplugEstimateDto, String> {
    let snap = battery::snapshot_all().and_then(|v| v.into_iter().next().ok_or_else(|| "no battery found".into())).map_err(|e| format!("battery: {e}"))?;
    let storage = storage::global().ok_or("storage not initialized")?;

    let remaining_mwh = if snap.status.capacity != BATTERY_UNKNOWN_CAPACITY {
        snap.status.capacity as f64
    } else {
        return Err("capacity unknown".into());
    };

    let rows = storage
        .read_recent_app_power()
        .map_err(|e| format!("app_power: {e}"))?;

    let total_app_w: f64 = rows.iter().map(|r| r.total_watts).sum();
    // Use the latest battery rate as ground truth if available, else sum of apps + estimate.
    let battery_w = crate::polling::last_battery_rate_w().abs();
    let total_draw = if battery_w > 0.5 { battery_w } else { total_app_w + 3.0 };
    let system_overhead = (total_draw - total_app_w).max(0.0);

    let total_hours = if total_draw > 0.1 {
        remaining_mwh / (total_draw * 1000.0)
    } else {
        0.0
    };

    let total_label = if total_hours > 0.0 {
        let h = total_hours as u32;
        let m = ((total_hours - h as f64) * 60.0).round() as u32;
        if h > 0 {
            format!("about {h}h {m}m of battery at current usage")
        } else {
            format!("about {m} min of battery at current usage")
        }
    } else {
        "estimating…".to_string()
    };

    let mut top_drains: Vec<UnplugDrainEntry> = rows
        .iter()
        .filter(|r| r.total_watts > 0.01)
        .take(5)
        .map(|r| {
            let est_h = if r.total_watts > 0.01 {
                remaining_mwh / (r.total_watts * 1000.0)
            } else {
                0.0
            };
            UnplugDrainEntry {
                name: r.process_name.clone(),
                watts: r.total_watts,
                est_hours: est_h,
            }
        })
        .collect();
    top_drains.truncate(5);

    Ok(UnplugEstimateDto {
        total_hours,
        total_label,
        top_drains,
        system_overhead_w: system_overhead,
    })
}
