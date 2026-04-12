// LibreHardwareMonitor WMI integration (x64 only).
//
// When LHM is running, it exposes all sensor data via the WMI namespace
// `root\LibreHardwareMonitor`. We query for Power-type sensors to get
// RAPL readings (CPU package, CPU cores, GPU, DRAM) that would otherwise
// require a kernel driver.
//
// If LHM isn't running, the WMI query returns an empty set and we
// gracefully fall back to EMI + ProcessEnergyValues — no errors, no logs.

use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Cached "is LHM available" flag so we don't query WMI every tick.
static LHM_CHECKED: AtomicBool = AtomicBool::new(false);
static LHM_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Power data extracted from LHM's WMI sensors.
#[derive(Debug, Clone, Default)]
pub struct LhmPowerData {
    /// CPU Package power (Intel PKG / AMD Package) in watts.
    pub cpu_package_w: Option<f64>,
    /// CPU Cores power (Intel PP0 / AMD sum-of-cores) in watts.
    pub cpu_cores_w: Option<f64>,
    /// Discrete GPU power in watts.
    pub gpu_power_w: Option<f64>,
    /// DRAM power in watts (server/HEDT only).
    pub dram_w: Option<f64>,
}

/// WMI row shape for LibreHardwareMonitor sensors.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct LhmSensor {
    name: String,
    sensor_type: String,
    value: f32,
}

pub fn is_available() -> bool {
    LHM_AVAILABLE.load(Ordering::Relaxed)
}

/// Direct WMI availability check, bypassing AND updating the cache.
/// Updates LHM_AVAILABLE so the polling thread picks up LHM data
/// on the very next tick instead of waiting up to 60 seconds.
pub fn check_now() -> bool {
    if cfg!(target_arch = "aarch64") {
        return false;
    }
    let available = query_lhm_wmi().is_some();
    LHM_AVAILABLE.store(available, Ordering::Relaxed);
    LHM_CHECKED.store(true, Ordering::Relaxed);
    available
}

/// State held across ticks for retry logic.
pub struct LhmState {
    last_check: Instant,
}

impl LhmState {
    pub fn new() -> Self {
        Self {
            // Set far in the past so first tick triggers a check.
            last_check: Instant::now()
                .checked_sub(std::time::Duration::from_secs(120))
                .unwrap_or(Instant::now()),
        }
    }
}

/// Read power sensors from LHM via WMI. Returns None if LHM isn't running
/// or the query fails. Checks availability every 60 seconds.
pub fn read_power(state: &mut LhmState) -> Option<LhmPowerData> {
    // On ARM64, skip entirely — EMI provides all we need.
    if cfg!(target_arch = "aarch64") {
        return None;
    }

    // Rate-limit WMI checks: retry every 60s if LHM wasn't found.
    if LHM_CHECKED.load(Ordering::Relaxed)
        && !LHM_AVAILABLE.load(Ordering::Relaxed)
        && state.last_check.elapsed().as_secs() < 60
    {
        return None;
    }

    state.last_check = Instant::now();

    let result = query_lhm_wmi();
    let available = result.is_some();

    if !LHM_CHECKED.swap(true, Ordering::Relaxed) && available {
        log::info!("LibreHardwareMonitor detected — enhanced power monitoring enabled");
    }
    LHM_AVAILABLE.store(available, Ordering::Relaxed);

    result
}

fn query_lhm_wmi() -> Option<LhmPowerData> {
    // Connect to the LHM WMI namespace. If LHM isn't running, the
    // namespace doesn't exist and COMLibrary/WMIConnection will fail.
    let com = wmi::COMLibrary::without_security().ok()?;
    let wmi_conn = wmi::WMIConnection::with_namespace_path(
        "root\\LibreHardwareMonitor",
        com,
    )
    .ok()?;

    // Query all power sensors.
    let sensors: Vec<LhmSensor> = wmi_conn
        .raw_query("SELECT Name, SensorType, Value FROM Sensor WHERE SensorType = 'Power'")
        .ok()?;

    if sensors.is_empty() {
        return None;
    }

    let mut data = LhmPowerData::default();

    for s in &sensors {
        if s.sensor_type != "Power" {
            continue;
        }
        let w = s.value as f64;
        if !w.is_finite() || w < 0.0 {
            continue;
        }
        let name = s.name.to_ascii_lowercase();

        if name.contains("cpu package") || name == "package" {
            data.cpu_package_w = Some(w);
        } else if name.contains("cpu cores") || name == "cores" {
            data.cpu_cores_w = Some(w);
        } else if name.contains("gpu power") || name.contains("gpu package") {
            data.gpu_power_w = Some(w);
        } else if name.contains("dram") || name.contains("memory") {
            data.dram_w = Some(w);
        }
        // Ignore per-core entries ("CPU Core #1 Power" etc.) — we use
        // the aggregate "CPU Cores" value for attribution.
    }

    // Only return if we got at least one useful reading.
    if data.cpu_package_w.is_some() || data.cpu_cores_w.is_some() || data.gpu_power_w.is_some() {
        Some(data)
    } else {
        None
    }
}
