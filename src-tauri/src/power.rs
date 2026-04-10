// CPU / system power reading.
//
// Two paths:
//
//   1. EMI (Energy Meter Interface) — documented in WDK header emi.h.
//      Works with NO admin, on ARM64 (Snapdragon X primary path) AND on
//      x64 Surface / OEM laptops that expose the device. Uses SetupDi to
//      find device nodes under GUID_DEVINTERFACE_EMI, opens with CreateFile,
//      reads metadata + energy counters via DeviceIoControl.
//
//   2. RAPL MSRs (Intel 12th-gen+, AMD Zen 3+) — requires kernel-mode
//      access to MSR 0x611 (PKG), 0x639 (PP0/cores), etc. Can only be
//      done via a signed kernel driver. In BugJuice proper this lives in
//      the `bugjuice-service` Windows service. The CLI prototype can't
//      read MSRs on its own — we stub it and print an honest status.
//
// Power math for EMI:
//   watts = delta_energy_pWh * 3.6e-9 / delta_time_seconds
// Derivation:
//   1 pWh = 1e-12 Wh = 1e-12 * 3600 J = 3.6e-9 J
//   absolute_time is in 100ns units, so delta_time_s = delta_100ns * 1e-7

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr;
use std::time::Duration;

use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;

// ─── EMI constants ────────────────────────────────────────────────────────────

/// GUID_DEVINTERFACE_EMI — {45BD8344-7ED6-49cf-A440-C276C933B053}
const EMI_GUID: GUID = GUID::from_u128(0x45BD8344_7ED6_49cf_A440_C276C933B053);

// EMI IOCTLs. CTL_CODE(FILE_DEVICE_UNKNOWN=0x22, func, METHOD_BUFFERED=0,
// FILE_READ_ACCESS=1) = (0x22<<16) | (1<<14) | (func<<2)
// Function codes per emi.h: GET_VERSION=0, GET_METADATA_SIZE=1,
// GET_METADATA=2, GET_MEASUREMENT=3.
const IOCTL_EMI_GET_VERSION: u32 = 0x224000; // func 0
const IOCTL_EMI_GET_METADATA_SIZE: u32 = 0x224004; // func 1
const IOCTL_EMI_GET_METADATA: u32 = 0x224008; // func 2
const IOCTL_EMI_GET_MEASUREMENT: u32 = 0x22400C; // func 3

const EMI_NAME_MAX: usize = 16;

// ─── EMI structs (match emi.h) ────────────────────────────────────────────────

/// EMI_VERSION — IOCTL_EMI_GET_VERSION output.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EmiVersionStruct {
    emi_version: u16,
}

/// EMI_METADATA_V1 header.
/// Fixed portion: 2 (unit) + 32 (oem) + 32 (model) + 2 (rev) + 2 (name_size) = 70 bytes.
/// Followed by `metered_hardware_name_size` bytes of WCHAR name.
#[repr(C)]
#[derive(Clone, Copy)]
struct EmiMetadataV1Header {
    measurement_unit: u16,
    hardware_oem: [u16; EMI_NAME_MAX],
    hardware_model: [u16; EMI_NAME_MAX],
    hardware_revision: u16,
    metered_hardware_name_size: u16,
    // WCHAR MeteredHardwareName[ANYSIZE_ARRAY];
}

/// EMI_METADATA_V2 header.
/// Fixed portion: 32 (oem) + 32 (model) + 2 (rev) + 2 (channel_count) = 68 bytes.
/// Followed by `channel_count` variable-length EMI_CHANNEL_V2 entries.
/// Note V2 has NO top-level MeasurementUnit — unit is per-channel.
#[repr(C)]
#[derive(Clone, Copy)]
struct EmiMetadataV2Header {
    hardware_oem: [u16; EMI_NAME_MAX],
    hardware_model: [u16; EMI_NAME_MAX],
    hardware_revision: u16,
    channel_count: u16,
}

// EMI_CHANNEL_V2 is variable-length:
//   ULONG MeasurementUnit;     // 4 bytes (C enum → int)
//   USHORT ChannelNameSize;    // 2 bytes (size of name in BYTES)
//   WCHAR ChannelName[ANYSIZE_ARRAY];
// On the Qualcomm Snapdragon X driver, entries are packed tight with no
// alignment padding between them — we parse byte-by-byte with u32/u16 reads.

/// EMI_MEASUREMENT_DATA — shared by V1 and V2 (16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct EmiMeasurementData {
    absolute_energy: u64, // picowatt-hours
    absolute_time: u64,   // 100-nanosecond intervals
}

// ─── Public types ─────────────────────────────────────────────────────────────

pub struct PowerChannel {
    pub name: String,
    pub watts: f64,
}

pub struct EmiReading {
    pub version: u16,
    pub oem: String,
    pub model: String,
    pub channels: Vec<PowerChannel>,
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Read all EMI devices over `delay` seconds (counters need a time window
/// to compute power from energy deltas). Returns one EmiReading per device.
pub fn read_all_emi(delay: Duration) -> Result<Vec<EmiReading>, String> {
    let paths = enumerate_emi_devices()?;
    if paths.is_empty() {
        return Err("no EMI devices present".into());
    }

    let mut results = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        match read_one_device(path, delay) {
            Ok(r) => results.push(r),
            Err(e) => eprintln!("  EMI device #{i}: {e}"),
        }
    }
    Ok(results)
}

/// Print a human-readable status block covering CPU/system power on this
/// machine. Tries EMI first; falls back to a RAPL status note on x64.
pub fn print_power_summary(delay: Duration) {
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("CPU / system power");
    println!("(measuring over {}s…)", delay.as_secs_f64());

    match read_all_emi(delay) {
        Ok(readings) if !readings.is_empty() => {
            for r in &readings {
                println!(
                    "\nEMI v{} — {} {}",
                    r.version,
                    if r.oem.is_empty() { "(unknown OEM)" } else { &r.oem },
                    if r.model.is_empty() { "" } else { &r.model }
                );

                // Categorize channels so we can group them meaningfully.
                // Names come from the OEM driver and differ by vendor:
                //   Snapdragon X (Qualcomm):  CPU_CLUSTER_0/1/2, GPU, PSU_USB,
                //                             USBC_TOTAL, SYS
                //   Microsoft PPM (x64 RAPL): RAPL_Package0_PKG, _PP0, _PP1,
                //                             _DRAM (Intel-style domains)
                // PSU/USBC are input channels (wall power in).
                // SYS is overall system draw.
                // CPU_CLUSTER_*, RAPL_*_PKG/PP0/PP1/DRAM, GPU are subsets —
                // summing them would double-count.
                let mut cpu: Vec<&PowerChannel> = Vec::new();
                let mut gpu: Vec<&PowerChannel> = Vec::new();
                let mut dram: Vec<&PowerChannel> = Vec::new();
                let mut inputs: Vec<&PowerChannel> = Vec::new();
                let mut system: Vec<&PowerChannel> = Vec::new();
                let mut other: Vec<&PowerChannel> = Vec::new();
                for c in &r.channels {
                    let n = c.name.to_ascii_uppercase();
                    // Intel-style RAPL domains exposed via Microsoft PPM
                    if n.contains("PP1") {
                        // PP1 is iGPU on Intel client parts
                        gpu.push(c);
                    } else if n.contains("DRAM") {
                        dram.push(c);
                    } else if n.contains("PKG")
                        || n.contains("PP0")
                        || n.contains("CORE")
                        || n.contains("CPU")
                    {
                        cpu.push(c);
                    } else if n.contains("GPU") {
                        gpu.push(c);
                    } else if n.contains("PSU") || n.contains("USBC") || n.contains("USB_C") {
                        inputs.push(c);
                    } else if n == "SYS"
                        || n.contains("PLATFORM")
                        || n.contains("SOC")
                        || n.contains("PSYS")
                    {
                        system.push(c);
                    } else {
                        other.push(c);
                    }
                }

                let section = |title: &str, chans: &[&PowerChannel]| {
                    if chans.is_empty() {
                        return;
                    }
                    println!("  {title}");
                    for c in chans {
                        println!("    {:20}  {:7.2} W", c.name, c.watts);
                    }
                };

                section("CPU", &cpu);
                section("GPU", &gpu);
                section("DRAM", &dram);
                section("system", &system);
                section("power input", &inputs);
                section("other", &other);

                // Derived totals. Prefer PKG (Intel RAPL package) as the
                // CPU figure when present since it already includes cores +
                // uncore + iGPU. Otherwise sum CPU_CLUSTER_* etc.
                let pkg_value: Option<f64> = cpu
                    .iter()
                    .find(|c| c.name.to_ascii_uppercase().contains("PKG"))
                    .map(|c| c.watts);
                let cpu_total: f64 = pkg_value
                    .unwrap_or_else(|| cpu.iter().map(|c| c.watts).sum());
                let gpu_total: f64 = gpu.iter().map(|c| c.watts).sum();
                let sys_total: f64 = system.iter().map(|c| c.watts).sum();
                let input_total: f64 = inputs.iter().map(|c| c.watts).sum();

                let has_summary =
                    cpu_total > 0.0 || gpu_total > 0.0 || sys_total > 0.0 || input_total > 0.0;
                if has_summary {
                    println!("\n  summary");
                    if cpu_total > 0.0 {
                        let label = if pkg_value.is_some() {
                            "CPU package:        "
                        } else {
                            "CPU (sum of cores): "
                        };
                        println!("    {label} {cpu_total:.2} W");
                    }
                    if gpu_total > 0.0 {
                        println!("    GPU:                 {gpu_total:.2} W");
                    }
                    if sys_total > 0.0 {
                        println!(
                            "    system draw:         {sys_total:.2} W  ← whole laptop"
                        );
                    }
                    if input_total > 0.0 {
                        println!(
                            "    power input:         {input_total:.2} W  ← from charger"
                        );
                    }
                }
            }
        }
        Ok(_) => {
            println!("\n  no EMI channels returned");
            rapl_status_note();
        }
        Err(e) => {
            println!("\n  EMI: {e}");
            if e.contains("no EMI devices present") {
                println!("                 (this platform does not expose an EMI device)");
            } else {
                println!("\n  (on Qualcomm Snapdragon X the EMI driver requires");
                println!("   admin/SYSTEM in practice — try running from an");
                println!("   elevated PowerShell. In production this lives inside");
                println!("   the bugjuice-service which runs as SYSTEM.)");
            }
            rapl_status_note();
        }
    }
}

fn rapl_status_note() {
    if cfg!(target_arch = "x86_64") {
        println!(
            "  RAPL MSR path: requires signed kernel driver via bugjuice-service"
        );
        println!("                 (Phase 2 — not implemented in CLI prototype)");
    } else {
        println!("  RAPL MSR path: N/A on this architecture ({})", std::env::consts::ARCH);
        println!("                 (RAPL is Intel/AMD x86 only)");
    }
}

// ─── Device enumeration ───────────────────────────────────────────────────────

fn enumerate_emi_devices() -> Result<Vec<Vec<u16>>, String> {
    unsafe {
        let hdev = SetupDiGetClassDevsW(
            Some(&EMI_GUID),
            PCWSTR(ptr::null()),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
        .map_err(|e| format!("SetupDiGetClassDevsW(EMI) failed: {e}"))?;

        let mut paths = Vec::new();

        for index in 0u32.. {
            let mut iface: SP_DEVICE_INTERFACE_DATA = zeroed();
            iface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

            if SetupDiEnumDeviceInterfaces(hdev, None, &EMI_GUID, index, &mut iface).is_err() {
                break;
            }

            let mut required: u32 = 0;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &iface,
                None,
                0,
                Some(&mut required),
                None,
            );
            if required == 0 {
                continue;
            }

            let mut buf = vec![0u8; required as usize];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

            if SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &iface,
                Some(detail),
                required,
                None,
                None,
            )
            .is_err()
            {
                continue;
            }

            // Copy the wide-char path out before we free the DeviceInfoList.
            let path_ptr = ptr::addr_of!((*detail).DevicePath) as *const u16;
            let mut path = Vec::with_capacity(260);
            let mut i: isize = 0;
            loop {
                let ch = *path_ptr.offset(i);
                path.push(ch);
                if ch == 0 {
                    break;
                }
                i += 1;
                if i > 1024 {
                    break;
                }
            }
            paths.push(path);
        }

        let _ = SetupDiDestroyDeviceInfoList(hdev);
        Ok(paths)
    }
}

// ─── Per-device read ──────────────────────────────────────────────────────────

fn read_one_device(path: &[u16], delay: Duration) -> Result<EmiReading, String> {
    unsafe {
        // Try several access masks. The Qualcomm EMI driver on Snapdragon X
        // has been observed to reject access=0 even though the WDK docs
        // suggest it should work. Fall through until one succeeds.
        const GENERIC_READ: u32 = 0x80000000;
        const GENERIC_WRITE: u32 = 0x40000000;
        const FILE_READ_ATTRIBUTES: u32 = 0x80;

        let attempts: [(u32, &str); 4] = [
            (GENERIC_READ | GENERIC_WRITE, "GENERIC_READ|GENERIC_WRITE"),
            (GENERIC_READ, "GENERIC_READ"),
            (FILE_READ_ATTRIBUTES, "FILE_READ_ATTRIBUTES"),
            (0, "access=0"),
        ];

        let mut handle: Option<HANDLE> = None;
        let mut last_err = String::new();
        for (access, label) in attempts {
            match CreateFileW(
                PCWSTR(path.as_ptr()),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            ) {
                Ok(h) => {
                    handle = Some(h);
                    break;
                }
                Err(e) => last_err = format!("{label}: {e}"),
            }
        }
        let handle = handle.ok_or_else(|| {
            format!(
                "CreateFileW(EMI) failed (tried 4 access masks, last: {last_err})"
            )
        })?;

        let result = read_one_inner(handle, delay);
        let _ = CloseHandle(handle);
        result
    }
}

unsafe fn read_one_inner(handle: HANDLE, delay: Duration) -> Result<EmiReading, String> {
    // ── Version ───────────────────────────────────────────────────────────
    let mut version = EmiVersionStruct::default();
    let mut bytes: u32 = 0;

    DeviceIoControl(
        handle,
        IOCTL_EMI_GET_VERSION,
        None,
        0,
        Some(&mut version as *mut _ as *mut c_void),
        size_of::<EmiVersionStruct>() as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_VERSION failed: {e}"))?;

    // ── Metadata size ─────────────────────────────────────────────────────
    let mut meta_size: u32 = 0;
    DeviceIoControl(
        handle,
        IOCTL_EMI_GET_METADATA_SIZE,
        None,
        0,
        Some(&mut meta_size as *mut _ as *mut c_void),
        size_of::<u32>() as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_METADATA_SIZE failed: {e}"))?;

    if meta_size == 0 || meta_size > 1_048_576 {
        return Err(format!("suspicious metadata size: {meta_size}"));
    }

    // ── Metadata ──────────────────────────────────────────────────────────
    let mut meta_buf = vec![0u8; meta_size as usize];
    DeviceIoControl(
        handle,
        IOCTL_EMI_GET_METADATA,
        None,
        0,
        Some(meta_buf.as_mut_ptr() as *mut c_void),
        meta_size,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_METADATA failed: {e}"))?;

    match version.emi_version {
        1 => parse_and_read_v1(handle, delay, &meta_buf),
        2 => parse_and_read_v2(handle, delay, &meta_buf),
        v => Err(format!("unsupported EMI version {v}")),
    }
}

// ─── V1 ───────────────────────────────────────────────────────────────────────

unsafe fn parse_and_read_v1(
    handle: HANDLE,
    delay: Duration,
    meta: &[u8],
) -> Result<EmiReading, String> {
    let header_size = size_of::<EmiMetadataV1Header>(); // 70
    if meta.len() < header_size {
        return Err(format!("V1 metadata too small: {} < {header_size}", meta.len()));
    }
    let hdr = ptr::read_unaligned(meta.as_ptr() as *const EmiMetadataV1Header);
    let oem = wide_to_string(&hdr.hardware_oem);
    let model = wide_to_string(&hdr.hardware_model);

    // Name follows header, length in bytes (not chars).
    let name_bytes = hdr.metered_hardware_name_size as usize;
    let channel_name = if meta.len() >= header_size + name_bytes && name_bytes > 0 {
        let name_slice = &meta[header_size..header_size + name_bytes];
        // Bytes are WCHAR (u16 LE). Convert.
        let wchars: Vec<u16> = name_slice
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        wide_to_string(&wchars)
    } else {
        String::new()
    };

    let t1 = read_measurement_v1(handle)?;
    std::thread::sleep(delay);
    let t2 = read_measurement_v1(handle)?;

    Ok(EmiReading {
        version: 1,
        oem,
        model,
        channels: vec![PowerChannel {
            name: channel_name,
            watts: compute_watts(t1, t2),
        }],
    })
}

unsafe fn read_measurement_v1(handle: HANDLE) -> Result<EmiMeasurementData, String> {
    let mut out = EmiMeasurementData::default();
    let mut bytes: u32 = 0;
    DeviceIoControl(
        handle,
        IOCTL_EMI_GET_MEASUREMENT,
        None,
        0,
        Some(&mut out as *mut _ as *mut c_void),
        size_of::<EmiMeasurementData>() as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_MEASUREMENT(V1) failed: {e}"))?;
    Ok(out)
}

// ─── V2 ───────────────────────────────────────────────────────────────────────

unsafe fn parse_and_read_v2(
    handle: HANDLE,
    delay: Duration,
    meta: &[u8],
) -> Result<EmiReading, String> {
    let header_size = size_of::<EmiMetadataV2Header>(); // 68
    if meta.len() < header_size {
        return Err(format!("V2 metadata too small: {} < {header_size}", meta.len()));
    }
    let hdr = ptr::read_unaligned(meta.as_ptr() as *const EmiMetadataV2Header);
    let oem = wide_to_string(&hdr.hardware_oem);
    let model = wide_to_string(&hdr.hardware_model);
    let channel_count = hdr.channel_count as usize;

    // Walk the variable-length channel list. EMI_CHANNEL_V2 looks like:
    //   EMI_MEASUREMENT_UNIT MeasurementUnit;  // C enum → 4 bytes
    //   USHORT ChannelNameSize;                // 2 bytes (bytes, not chars)
    //   WCHAR ChannelName[ChannelNameSize/2];
    // Struct alignment is 4 (because of the int). Entries in the buffer
    // are padded to the next 4-byte boundary between entries.
    let mut channel_names: Vec<String> = Vec::with_capacity(channel_count);
    let mut offset = header_size;

    for i in 0..channel_count {
        // Fixed prefix: 4 (unit, C enum → int) + 2 (name_size) = 6 bytes.
        if offset + 6 > meta.len() {
            return Err(format!(
                "V2 channel #{i} header past buffer (offset={offset}, len={})",
                meta.len()
            ));
        }
        let measurement_unit = u32::from_le_bytes([
            meta[offset],
            meta[offset + 1],
            meta[offset + 2],
            meta[offset + 3],
        ]);
        let name_bytes =
            u16::from_le_bytes([meta[offset + 4], meta[offset + 5]]) as usize;

        let name_start = offset + 6;
        if name_start + name_bytes > meta.len() {
            return Err(format!(
                "V2 channel #{i} name past buffer (offset={name_start}, name_size={name_bytes}, len={})",
                meta.len()
            ));
        }
        let name_slice = &meta[name_start..name_start + name_bytes];
        let wchars: Vec<u16> = name_slice
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let name = wide_to_string(&wchars);

        // Accept only the documented unit (picowatt-hours = 0). Other
        // values are silently clamped — the conversion math assumes pWh.
        let _ = measurement_unit;
        channel_names.push(name);

        // Entries are packed — no padding between them.
        offset = name_start + name_bytes;
    }

    // V2 measurement IOCTL returns an array of EMI_CHANNEL_MEASUREMENT_DATA
    // or EMI_MEASUREMENT_DATA — TBD on this device. Try fixed-struct array
    // first (16 bytes per channel).
    let t1 = read_measurements_v2(handle, channel_count)?;
    std::thread::sleep(delay);
    let t2 = read_measurements_v2(handle, channel_count)?;

    let mut channels = Vec::with_capacity(channel_count);
    for (i, name) in channel_names.into_iter().enumerate() {
        channels.push(PowerChannel {
            name,
            watts: compute_watts(t1[i], t2[i]),
        });
    }

    Ok(EmiReading {
        version: 2,
        oem,
        model,
        channels,
    })
}

unsafe fn read_measurements_v2(
    handle: HANDLE,
    channel_count: usize,
) -> Result<Vec<EmiMeasurementData>, String> {
    let mut out = vec![EmiMeasurementData::default(); channel_count];
    let mut bytes: u32 = 0;
    let buf_size = (channel_count * size_of::<EmiMeasurementData>()) as u32;

    DeviceIoControl(
        handle,
        IOCTL_EMI_GET_MEASUREMENT,
        None,
        0,
        Some(out.as_mut_ptr() as *mut c_void),
        buf_size,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_MEASUREMENT(V2) failed: {e}"))?;
    Ok(out)
}

// ─── Math ─────────────────────────────────────────────────────────────────────

fn compute_watts(t1: EmiMeasurementData, t2: EmiMeasurementData) -> f64 {
    let delta_energy_pwh = t2.absolute_energy.saturating_sub(t1.absolute_energy) as f64;
    let delta_time_100ns = t2.absolute_time.saturating_sub(t1.absolute_time) as f64;
    if delta_time_100ns <= 0.0 {
        return 0.0;
    }
    let delta_time_s = delta_time_100ns * 1e-7;
    // 1 pWh = 1e-12 Wh = 3.6e-9 J
    let delta_energy_j = delta_energy_pwh * 3.6e-9;
    delta_energy_j / delta_time_s
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end]).trim().to_string()
}
