// EMI (Energy Meter Interface) reading — adapted from src-tauri/src/power.rs.
//
// Reads per-channel power data from EMI devices. On Snapdragon X this
// requires SYSTEM-level access, which is why this code lives in the
// service rather than the main app.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

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

// ─── Constants ───────────────────────────────────────────────────────────────

const EMI_GUID: GUID = GUID::from_u128(0x45BD8344_7ED6_49cf_A440_C276C933B053);

const IOCTL_EMI_GET_VERSION: u32 = 0x224000;
const IOCTL_EMI_GET_METADATA_SIZE: u32 = 0x224004;
const IOCTL_EMI_GET_METADATA: u32 = 0x224008;
const IOCTL_EMI_GET_MEASUREMENT: u32 = 0x22400C;

const EMI_NAME_MAX: usize = 16;

// ─── Internal structs ────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EmiVersionStruct {
    emi_version: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EmiMetadataV1Header {
    measurement_unit: u16,
    hardware_oem: [u16; EMI_NAME_MAX],
    hardware_model: [u16; EMI_NAME_MAX],
    hardware_revision: u16,
    metered_hardware_name_size: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EmiMetadataV2Header {
    hardware_oem: [u16; EMI_NAME_MAX],
    hardware_model: [u16; EMI_NAME_MAX],
    hardware_revision: u16,
    channel_count: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct EmiMeasurementData {
    absolute_energy: u64,
    absolute_time: u64,
}

// ─── Public types ────────────────────────────────────────────────────────────

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PowerChannel {
    pub name: String,
    pub watts: f64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct EmiReading {
    pub version: u16,
    pub oem: String,
    pub model: String,
    pub channels: Vec<PowerChannel>,
}

// ─── Public API ──────────────────────────────────────────────────────────────

pub fn read_all_emi(delay: Duration) -> Result<Vec<EmiReading>, String> {
    let paths =
        enumerate_emi_devices().map_err(|e| format!("enumerate_emi_devices: {e}"))?;
    if paths.is_empty() {
        return Err("no EMI devices present".into());
    }

    let mut results = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        match read_one_device(path, delay) {
            Ok(r) => results.push(r),
            Err(e) => errors.push(format!("dev{i}: {e}")),
        }
    }
    if results.is_empty() && !errors.is_empty() {
        return Err(format!(
            "found {} device(s) but all reads failed — {}",
            paths.len(),
            errors.join(" | ")
        ));
    }
    Ok(results)
}

// ─── Device enumeration ─────────────────────────────────────────────────────

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
            if SetupDiEnumDeviceInterfaces(hdev, None, &EMI_GUID, index, &mut iface)
                .is_err()
            {
                break;
            }

            let mut required: u32 = 0;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                hdev, &iface, None, 0, Some(&mut required), None,
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

// ─── Per-device read ────────────────────────────────────────────────────────

fn read_one_device(path: &[u16], delay: Duration) -> Result<EmiReading, String> {
    unsafe {
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
            format!("CreateFileW(EMI) failed (tried 4 access masks, last: {last_err})")
        })?;

        let result = read_one_inner(handle, delay);
        let _ = CloseHandle(handle);
        result
    }
}

unsafe fn read_one_inner(handle: HANDLE, delay: Duration) -> Result<EmiReading, String> {
    let mut version = EmiVersionStruct::default();
    let mut bytes: u32 = 0;

    DeviceIoControl(
        handle, IOCTL_EMI_GET_VERSION, None, 0,
        Some(&mut version as *mut _ as *mut c_void),
        size_of::<EmiVersionStruct>() as u32,
        Some(&mut bytes), None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_VERSION failed: {e}"))?;

    let mut meta_size: u32 = 0;
    DeviceIoControl(
        handle, IOCTL_EMI_GET_METADATA_SIZE, None, 0,
        Some(&mut meta_size as *mut _ as *mut c_void),
        size_of::<u32>() as u32,
        Some(&mut bytes), None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_METADATA_SIZE failed: {e}"))?;

    if meta_size == 0 || meta_size > 1_048_576 {
        return Err(format!("suspicious metadata size: {meta_size}"));
    }

    let mut meta_buf = vec![0u8; meta_size as usize];
    DeviceIoControl(
        handle, IOCTL_EMI_GET_METADATA, None, 0,
        Some(meta_buf.as_mut_ptr() as *mut c_void),
        meta_size, Some(&mut bytes), None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_METADATA failed: {e}"))?;

    match version.emi_version {
        1 => parse_and_read_v1(handle, delay, &meta_buf),
        2 => parse_and_read_v2(handle, delay, &meta_buf),
        v => Err(format!("unsupported EMI version {v}")),
    }
}

// ─── V1 ──────────────────────────────────────────────────────────────────────

unsafe fn parse_and_read_v1(
    handle: HANDLE, delay: Duration, meta: &[u8],
) -> Result<EmiReading, String> {
    let header_size = size_of::<EmiMetadataV1Header>();
    if meta.len() < header_size {
        return Err(format!("V1 metadata too small: {} < {header_size}", meta.len()));
    }
    let hdr = ptr::read_unaligned(meta.as_ptr() as *const EmiMetadataV1Header);
    let oem = wide_to_string(&hdr.hardware_oem);
    let model = wide_to_string(&hdr.hardware_model);

    let name_bytes = hdr.metered_hardware_name_size as usize;
    let channel_name = if meta.len() >= header_size + name_bytes && name_bytes > 0 {
        let name_slice = &meta[header_size..header_size + name_bytes];
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
        version: 1, oem, model,
        channels: vec![PowerChannel { name: channel_name, watts: compute_watts(t1, t2) }],
    })
}

unsafe fn read_measurement_v1(handle: HANDLE) -> Result<EmiMeasurementData, String> {
    let mut out = EmiMeasurementData::default();
    let mut bytes: u32 = 0;
    DeviceIoControl(
        handle, IOCTL_EMI_GET_MEASUREMENT, None, 0,
        Some(&mut out as *mut _ as *mut c_void),
        size_of::<EmiMeasurementData>() as u32,
        Some(&mut bytes), None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_MEASUREMENT(V1) failed: {e}"))?;
    Ok(out)
}

// ─── V2 ──────────────────────────────────────────────────────────────────────

unsafe fn parse_and_read_v2(
    handle: HANDLE, delay: Duration, meta: &[u8],
) -> Result<EmiReading, String> {
    let header_size = size_of::<EmiMetadataV2Header>();
    if meta.len() < header_size {
        return Err(format!("V2 metadata too small: {} < {header_size}", meta.len()));
    }
    let hdr = ptr::read_unaligned(meta.as_ptr() as *const EmiMetadataV2Header);
    let oem = wide_to_string(&hdr.hardware_oem);
    let model = wide_to_string(&hdr.hardware_model);
    let channel_count = hdr.channel_count as usize;

    let mut channel_names: Vec<String> = Vec::with_capacity(channel_count);
    let mut offset = header_size;

    for i in 0..channel_count {
        if offset + 6 > meta.len() {
            return Err(format!(
                "V2 channel #{i} header past buffer (offset={offset}, len={})",
                meta.len()
            ));
        }
        let _measurement_unit = u32::from_le_bytes([
            meta[offset], meta[offset + 1], meta[offset + 2], meta[offset + 3],
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
        channel_names.push(wide_to_string(&wchars));
        offset = name_start + name_bytes;
    }

    let t1 = read_measurements_v2(handle, channel_count)?;
    std::thread::sleep(delay);
    let t2 = read_measurements_v2(handle, channel_count)?;

    let mut channels = Vec::with_capacity(channel_count);
    for (i, name) in channel_names.into_iter().enumerate() {
        channels.push(PowerChannel { name, watts: compute_watts(t1[i], t2[i]) });
    }

    Ok(EmiReading { version: 2, oem, model, channels })
}

unsafe fn read_measurements_v2(
    handle: HANDLE, channel_count: usize,
) -> Result<Vec<EmiMeasurementData>, String> {
    let mut out = vec![EmiMeasurementData::default(); channel_count];
    let mut bytes: u32 = 0;
    let buf_size = (channel_count * size_of::<EmiMeasurementData>()) as u32;
    DeviceIoControl(
        handle, IOCTL_EMI_GET_MEASUREMENT, None, 0,
        Some(out.as_mut_ptr() as *mut c_void),
        buf_size, Some(&mut bytes), None,
    )
    .map_err(|e| format!("IOCTL_EMI_GET_MEASUREMENT(V2) failed: {e}"))?;
    Ok(out)
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn compute_watts(t1: EmiMeasurementData, t2: EmiMeasurementData) -> f64 {
    let delta_energy_pwh = t2.absolute_energy.saturating_sub(t1.absolute_energy) as f64;
    let delta_time_100ns = t2.absolute_time.saturating_sub(t1.absolute_time) as f64;
    if delta_time_100ns <= 0.0 {
        return 0.0;
    }
    let delta_time_s = delta_time_100ns * 1e-7;
    let delta_energy_j = delta_energy_pwh * 3.6e-9;
    delta_energy_j / delta_time_s
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end]).trim().to_string()
}
