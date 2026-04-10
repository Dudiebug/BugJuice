// Live dashboard mode.
//
// Refreshes every 2 seconds in place using ANSI cursor movement. Shows
// battery status, EMI power channels, and top processes by attributed
// power (CPU + GPU). Also starts the background polling thread so your
// history continues to persist to SQLite while you're watching.
//
// Terminal requirements: Windows 10 1607+ (virtual terminal sequences).
// We explicitly enable ENABLE_VIRTUAL_TERMINAL_PROCESSING at startup so
// older cmd.exe hosts work too.

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::battery::{
    self, BATTERY_CHARGING, BATTERY_CRITICAL, BATTERY_DISCHARGING, BATTERY_POWER_ON_LINE,
    BATTERY_UNKNOWN_CAPACITY, BATTERY_UNKNOWN_RATE, BATTERY_UNKNOWN_VOLTAGE, BatterySnapshot,
};
use crate::gpu::GpuQuery;
use crate::power::{self, EmiReading, PowerChannel};
use crate::processes::{self, ProcessSample, ProcessorTotals};

const REFRESH: Duration = Duration::from_secs(2);

pub fn run() {
    enable_vt();

    // Hide cursor, clear screen once.
    print!("\x1B[?25l\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();

    let mut gpu_query = GpuQuery::new();
    let mut prev_procs: Option<Vec<ProcessSample>> = None;
    let mut prev_cpu_totals: Option<ProcessorTotals> = None;

    loop {
        let frame = build_frame(&mut gpu_query, &mut prev_procs, &mut prev_cpu_totals);
        redraw(&frame);
        std::thread::sleep(REFRESH);
    }
}

// ─── Frame builder ────────────────────────────────────────────────────────────

fn build_frame(
    gpu: &mut Option<GpuQuery>,
    prev_procs: &mut Option<Vec<ProcessSample>>,
    prev_cpu_totals: &mut Option<ProcessorTotals>,
) -> String {
    let mut out = String::with_capacity(4096);

    // Header
    let _ = writeln!(
        out,
        "BugJuice — Live Monitor{:>40}",
        format!("  {}  [Ctrl+C to quit]", timestamp())
    );
    let _ = writeln!(out, "{}", "─".repeat(78));

    // ── Battery ──────────────────────────────────────────────────────────
    let snap = match battery::snapshot_all() {
        Ok(s) if !s.is_empty() => Some(s),
        _ => None,
    };

    match snap.as_ref().and_then(|v| v.first()) {
        Some(snap) => render_battery(&mut out, snap),
        None => {
            let _ = writeln!(out, "\nBATTERY");
            let _ = writeln!(out, "  (no battery detected)");
        }
    }

    // ── Power / EMI ──────────────────────────────────────────────────────
    let (cpu_pkg_w, gpu_pkg_w) = match power::read_all_emi(Duration::from_millis(200)) {
        Ok(readings) if !readings.is_empty() => render_power(&mut out, &readings),
        Ok(_) => {
            let _ = writeln!(out, "\nPOWER");
            let _ = writeln!(out, "  no EMI devices present");
            (None, None)
        }
        Err(e) => {
            let _ = writeln!(out, "\nPOWER");
            let _ = writeln!(out, "  EMI: {e}");
            if e.contains("denied") || e.contains("Access") {
                let _ = writeln!(
                    out,
                    "  (on Snapdragon X, EMI needs admin — run from elevated PowerShell)"
                );
            }
            (None, None)
        }
    };

    // ── Top processes by power ───────────────────────────────────────────
    let curr_procs = processes::snapshot_processes().ok();
    let curr_totals = processes::snapshot_processor_totals().ok();
    let gpu_map: HashMap<u32, f64> = gpu.as_mut().map(|g| g.sample()).unwrap_or_default();

    let _ = writeln!(out, "\nTOP PROCESSES BY POWER");
    let _ = writeln!(
        out,
        "  {:<6}  {:<32}  {:>10}  {:>10}  {:>10}",
        "PID", "NAME", "CPU (W)", "GPU (W)", "TOTAL (W)"
    );
    let _ = writeln!(out, "  {}", "─".repeat(74));

    if let (Some(prev_p), Some(prev_t), Some(curr_p), Some(curr_t)) = (
        prev_procs.as_ref(),
        prev_cpu_totals.as_ref(),
        curr_procs.as_ref(),
        curr_totals.as_ref(),
    ) {
        let deltas = processes::compute_deltas(prev_p, curr_p, prev_t, curr_t);
        render_processes(&mut out, &deltas, &gpu_map, cpu_pkg_w, gpu_pkg_w);
    } else {
        let _ = writeln!(out, "  sampling…");
    }

    *prev_procs = curr_procs;
    *prev_cpu_totals = curr_totals;

    out
}

// ─── Battery section ──────────────────────────────────────────────────────────

fn render_battery(out: &mut String, snap: &BatterySnapshot) {
    let info = &snap.info;
    let status = &snap.status;
    let full = info.full_charged_capacity.max(1) as f64;
    let pct = if status.capacity != BATTERY_UNKNOWN_CAPACITY {
        Some(status.capacity as f64 / full * 100.0)
    } else {
        None
    };

    let _ = writeln!(out, "\nBATTERY");

    // Gauge bar
    if let Some(p) = pct {
        let bar = gauge(p, 26);
        let _ = writeln!(
            out,
            "  {bar}  {p:5.1}%     {} / {} mWh",
            status.capacity, info.full_charged_capacity
        );
    } else {
        let _ = writeln!(out, "  [capacity unknown]");
    }

    // State + rate line
    let mut state_parts: Vec<&str> = Vec::new();
    if status.power_state & BATTERY_POWER_ON_LINE != 0 {
        state_parts.push("on AC");
    }
    if status.power_state & BATTERY_CHARGING != 0 {
        state_parts.push("charging");
    }
    if status.power_state & BATTERY_DISCHARGING != 0 {
        state_parts.push("discharging");
    }
    if status.power_state & BATTERY_CRITICAL != 0 {
        state_parts.push("CRITICAL");
    }
    let state_str = if state_parts.is_empty() {
        "idle".to_string()
    } else {
        state_parts.join(", ")
    };

    let rate_str = if status.rate == BATTERY_UNKNOWN_RATE || status.rate == 0 {
        "rate not reported".to_string()
    } else if status.rate > 0 {
        let w = status.rate as f64 / 1000.0;
        let remaining_to_full = info.full_charged_capacity.saturating_sub(status.capacity);
        let time_h = remaining_to_full as f64 / status.rate as f64;
        format!(
            "charging at {w:.2} W  ·  ~{} to full",
            crate::format_hours(time_h)
        )
    } else {
        let w = (-status.rate) as f64 / 1000.0;
        let time_h = status.capacity as f64 / (-status.rate) as f64;
        format!(
            "draining at {w:.2} W  ·  ~{} left",
            crate::format_hours(time_h)
        )
    };

    let voltage_str = if status.voltage != BATTERY_UNKNOWN_VOLTAGE {
        format!("{:.2} V", status.voltage as f64 / 1000.0)
    } else {
        "— V".to_string()
    };
    let temp_str = snap
        .temperature_c
        .map(|c| format!("{c:.1}°C"))
        .unwrap_or_else(|| "—".to_string());

    let _ = writeln!(out, "  {state_str}  ·  {rate_str}");
    let _ = writeln!(out, "  {voltage_str}  ·  {temp_str}");

    // Health
    let raw_health = info.full_charged_capacity as f64 / info.designed_capacity.max(1) as f64 * 100.0;
    let wear = (100.0 - raw_health).max(0.0);
    let _ = writeln!(
        out,
        "  health {:.0}%  ·  wear {:.1}%  ·  {} cycles",
        raw_health.min(100.0),
        wear,
        info.cycle_count
    );
}

fn gauge(pct: f64, width: usize) -> String {
    let clamped = pct.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    let mut s = String::with_capacity(width + 2);
    s.push('[');
    for _ in 0..filled {
        s.push('█');
    }
    for _ in 0..empty {
        s.push('░');
    }
    s.push(']');
    s
}

// ─── Power section ────────────────────────────────────────────────────────────

fn render_power(out: &mut String, readings: &[EmiReading]) -> (Option<f64>, Option<f64>) {
    let r = &readings[0];
    let _ = writeln!(
        out,
        "\nPOWER  ({} {})",
        if r.oem.is_empty() { "EMI" } else { r.oem.as_str() },
        r.model
    );

    // Categorize
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
    let sys_total: f64 = system.iter().map(|c| c.watts).sum();
    let input_total: f64 = inputs.iter().map(|c| c.watts).sum();

    // Summary block — the big numbers first
    if input_total > 0.0 {
        let _ = writeln!(out, "  wall input    {:>7.2} W", input_total);
    }
    if sys_total > 0.0 {
        let _ = writeln!(out, "  system draw   {:>7.2} W", sys_total);
    }
    if let Some(w) = cpu_total {
        let label = if pkg_value.is_some() {
            "CPU package  "
        } else {
            "CPU clusters "
        };
        let _ = writeln!(out, "  {label} {:>7.2} W", w);
    }
    if let Some(w) = gpu_total {
        let _ = writeln!(out, "  GPU           {:>7.2} W", w);
    }
    let dram_total: f64 = dram.iter().map(|c| c.watts).sum();
    if dram_total > 0.0 {
        let _ = writeln!(out, "  DRAM          {:>7.2} W", dram_total);
    }

    (cpu_total, gpu_total)
}

// ─── Process section ──────────────────────────────────────────────────────────

fn render_processes(
    out: &mut String,
    deltas: &[processes::ProcessDelta],
    gpu_map: &HashMap<u32, f64>,
    cpu_pkg_w: Option<f64>,
    gpu_pkg_w: Option<f64>,
) {
    // Compute per-process power rows, then sort by total descending.
    struct Row {
        pid: u32,
        name: String,
        cpu_w: f64,
        gpu_w: f64,
        total_w: f64,
    }
    let mut rows: Vec<Row> = deltas
        .iter()
        .map(|d| {
            let cpu_w = cpu_pkg_w.map(|w| d.share * w).unwrap_or(0.0);
            let gpu_pct = gpu_map.get(&d.pid).copied().unwrap_or(0.0);
            let gpu_frac = (gpu_pct / 100.0).clamp(0.0, 1.0);
            let gpu_w = gpu_pkg_w.map(|w| gpu_frac * w).unwrap_or(0.0);
            let total_w = cpu_w + gpu_w;
            Row {
                pid: d.pid,
                name: d.name.clone(),
                cpu_w,
                gpu_w,
                total_w,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.total_w.partial_cmp(&a.total_w).unwrap_or(std::cmp::Ordering::Equal));

    for row in rows.iter().take(12) {
        if row.total_w < 0.001 {
            continue;
        }
        let name = truncate_name(&row.name, 32);
        let _ = writeln!(
            out,
            "  {:<6}  {:<32}  {:>10.3}  {:>10.3}  {:>10.3}",
            row.pid, name, row.cpu_w, row.gpu_w, row.total_w
        );
    }
}

fn truncate_name(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

// ─── Terminal / VT helpers ────────────────────────────────────────────────────

/// Render a frame in-place: move cursor to home, print each line with a
/// "clear to end of line" suffix, then clear anything below. This avoids
/// the flicker you'd get from a full screen-wipe.
fn redraw(frame: &str) {
    let mut out = String::from("\x1B[H");
    for line in frame.lines() {
        out.push_str(line);
        out.push_str("\x1B[K\n");
    }
    out.push_str("\x1B[J");
    print!("{out}");
    let _ = std::io::stdout().flush();
}

fn enable_vt() {
    use windows::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        if let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(handle, &mut mode).is_ok() {
                let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
