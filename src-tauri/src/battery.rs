// Battery IOCTL interface.
//
// All of this works without admin. Uses SetupDi to enumerate devices
// under GUID_DEVCLASS_BATTERY, opens them with CreateFile, and queries
// via DeviceIoControl.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr;

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

// ─── Constants ────────────────────────────────────────────────────────────────

/// GUID_DEVCLASS_BATTERY — {72631e54-78a4-11d0-bcf7-00aa00b7b32a}
pub const GUID_DEVCLASS_BATTERY: GUID =
    GUID::from_u128(0x72631e54_78a4_11d0_bcf7_00aa00b7b32a);

// IOCTL codes (from poclass.h)
const IOCTL_BATTERY_QUERY_TAG: u32 = 0x294040;
const IOCTL_BATTERY_QUERY_INFORMATION: u32 = 0x294044;
const IOCTL_BATTERY_QUERY_STATUS: u32 = 0x29404C;

// BATTERY_QUERY_INFORMATION_LEVEL values
const BATTERY_INFO_LEVEL: u32 = 0;
const BATTERY_TEMPERATURE_LEVEL: u32 = 2;
const BATTERY_ESTIMATED_TIME_LEVEL: u32 = 3;
const BATTERY_DEVICE_NAME_LEVEL: u32 = 4;
const BATTERY_MANUFACTURE_DATE_LEVEL: u32 = 5;
const BATTERY_MANUFACTURE_NAME_LEVEL: u32 = 6;
const BATTERY_SERIAL_NUMBER_LEVEL: u32 = 8;

// Power state bits
pub const BATTERY_POWER_ON_LINE: u32 = 0x00000001;
pub const BATTERY_DISCHARGING: u32 = 0x00000002;
pub const BATTERY_CHARGING: u32 = 0x00000004;
pub const BATTERY_CRITICAL: u32 = 0x00000008;

// Sentinel values
pub const BATTERY_UNKNOWN_RATE: i32 = i32::MIN; // 0x80000000
pub const BATTERY_UNKNOWN_CAPACITY: u32 = 0xFFFFFFFF;
pub const BATTERY_UNKNOWN_VOLTAGE: u32 = 0xFFFFFFFF;
pub const BATTERY_UNKNOWN_TIME: u32 = 0xFFFFFFFF;

// CreateFile access rights
const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;

// ─── Raw Windows structs ──────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
struct BatteryQueryInformation {
    battery_tag: u32,
    information_level: u32,
    at_rate: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BatteryInformation {
    pub capabilities: u32,
    pub technology: u8,
    pub reserved: [u8; 3],
    pub chemistry: [u8; 4],
    pub designed_capacity: u32,
    pub full_charged_capacity: u32,
    pub default_alert1: u32,
    pub default_alert2: u32,
    pub critical_bias: u32,
    pub cycle_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BatteryWaitStatus {
    battery_tag: u32,
    timeout: u32,
    power_state: u32,
    low_capacity: u32,
    high_capacity: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BatteryStatus {
    pub power_state: u32,
    pub capacity: u32,
    pub voltage: u32,
    pub rate: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct BatteryManufactureDate {
    pub day: u8,
    pub month: u8,
    pub year: u16,
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Full snapshot of one battery for high-level callers.
pub struct BatterySnapshot {
    pub tag: u32,
    pub info: BatteryInformation,
    pub status: BatteryStatus,
    pub manufacturer: Option<String>,
    pub device_name: Option<String>,
    pub serial: Option<String>,
    pub manufacture_date: Option<BatteryManufactureDate>,
    pub temperature_c: Option<f64>,
    pub estimated_seconds: Option<u32>,
}

/// Enumerate all batteries and return a snapshot of each.
pub fn snapshot_all() -> Result<Vec<BatterySnapshot>, String> {
    unsafe {
        let hdev = SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_BATTERY),
            PCWSTR(ptr::null()),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
        .map_err(|e| format!("SetupDiGetClassDevsW failed: {e}"))?;

        let mut out = Vec::new();

        for index in 0u32.. {
            let mut iface_data: SP_DEVICE_INTERFACE_DATA = zeroed();
            iface_data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

            if SetupDiEnumDeviceInterfaces(
                hdev,
                None,
                &GUID_DEVCLASS_BATTERY,
                index,
                &mut iface_data,
            )
            .is_err()
            {
                break;
            }

            let mut required: u32 = 0;
            let _ = SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &iface_data,
                None,
                0,
                Some(&mut required),
                None,
            );
            if required == 0 {
                continue;
            }

            let mut buf: Vec<u8> = vec![0u8; required as usize];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

            if SetupDiGetDeviceInterfaceDetailW(
                hdev,
                &iface_data,
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
            match probe_single(PCWSTR(path_ptr)) {
                Ok(snap) => out.push(snap),
                Err(e) => eprintln!("battery #{index}: {e}"),
            }
        }

        let _ = SetupDiDestroyDeviceInfoList(hdev);
        Ok(out)
    }
}

/// Fast path for the sleep-drain case: read current capacity in mWh from
/// the first available battery. Opens and closes the handle inline.
pub fn quick_capacity_mwh() -> Result<u32, String> {
    let snaps = snapshot_all()?;
    let first = snaps
        .into_iter()
        .next()
        .ok_or_else(|| "no batteries present".to_string())?;
    if first.status.capacity == BATTERY_UNKNOWN_CAPACITY {
        return Err("capacity unknown".into());
    }
    Ok(first.status.capacity)
}

// ─── Internals ────────────────────────────────────────────────────────────────

unsafe fn probe_single(device_path: PCWSTR) -> Result<BatterySnapshot, String> {
    let handle = CreateFileW(
        device_path,
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        None,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        None,
    )
    .map_err(|e| format!("CreateFileW failed: {e}"))?;

    let result = read_all(handle);
    let _ = CloseHandle(handle);
    result
}

unsafe fn read_all(handle: HANDLE) -> Result<BatterySnapshot, String> {
    // Query tag
    let wait_timeout: u32 = 0;
    let mut tag: u32 = 0;
    let mut bytes: u32 = 0;

    DeviceIoControl(
        handle,
        IOCTL_BATTERY_QUERY_TAG,
        Some(&wait_timeout as *const _ as *const c_void),
        size_of::<u32>() as u32,
        Some(&mut tag as *mut _ as *mut c_void),
        size_of::<u32>() as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_BATTERY_QUERY_TAG failed: {e}"))?;

    // Static info
    let info: BatteryInformation = query_information(handle, tag, BATTERY_INFO_LEVEL)?;

    let manufacturer = query_string(handle, tag, BATTERY_MANUFACTURE_NAME_LEVEL).ok();
    let device_name = query_string(handle, tag, BATTERY_DEVICE_NAME_LEVEL).ok();
    let serial = query_string(handle, tag, BATTERY_SERIAL_NUMBER_LEVEL).ok();

    let manufacture_date = query_information::<BatteryManufactureDate>(
        handle,
        tag,
        BATTERY_MANUFACTURE_DATE_LEVEL,
    )
    .ok()
    .filter(|d| d.year != 0);

    let temperature_c = query_information::<u32>(handle, tag, BATTERY_TEMPERATURE_LEVEL)
        .ok()
        .map(|t| (t as f64 / 10.0) - 273.15)
        .filter(|c| c.is_finite() && (-40.0..=120.0).contains(c));

    // Live status
    let wait_in = BatteryWaitStatus {
        battery_tag: tag,
        timeout: 0,
        power_state: 0,
        low_capacity: 0,
        high_capacity: 0,
    };
    let mut status = BatteryStatus::default();
    let mut bytes: u32 = 0;

    DeviceIoControl(
        handle,
        IOCTL_BATTERY_QUERY_STATUS,
        Some(&wait_in as *const _ as *const c_void),
        size_of::<BatteryWaitStatus>() as u32,
        Some(&mut status as *mut _ as *mut c_void),
        size_of::<BatteryStatus>() as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_BATTERY_QUERY_STATUS failed: {e}"))?;

    let estimated_seconds = query_information::<u32>(handle, tag, BATTERY_ESTIMATED_TIME_LEVEL)
        .ok()
        .filter(|&s| s != BATTERY_UNKNOWN_TIME && s != 0);

    Ok(BatterySnapshot {
        tag,
        info,
        status,
        manufacturer,
        device_name,
        serial,
        manufacture_date,
        temperature_c,
        estimated_seconds,
    })
}

unsafe fn query_information<T: Copy + Default>(
    handle: HANDLE,
    battery_tag: u32,
    level: u32,
) -> Result<T, String> {
    let bqi = BatteryQueryInformation {
        battery_tag,
        information_level: level,
        at_rate: 0,
    };
    let mut out: T = T::default();
    let mut bytes: u32 = 0;

    DeviceIoControl(
        handle,
        IOCTL_BATTERY_QUERY_INFORMATION,
        Some(&bqi as *const _ as *const c_void),
        size_of::<BatteryQueryInformation>() as u32,
        Some(&mut out as *mut _ as *mut c_void),
        size_of::<T>() as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL_BATTERY_QUERY_INFORMATION (level {level}) failed: {e}"))?;

    Ok(out)
}

unsafe fn query_string(
    handle: HANDLE,
    battery_tag: u32,
    level: u32,
) -> Result<String, String> {
    let bqi = BatteryQueryInformation {
        battery_tag,
        information_level: level,
        at_rate: 0,
    };
    let mut buf = vec![0u16; 256];
    let mut bytes: u32 = 0;

    DeviceIoControl(
        handle,
        IOCTL_BATTERY_QUERY_INFORMATION,
        Some(&bqi as *const _ as *const c_void),
        size_of::<BatteryQueryInformation>() as u32,
        Some(buf.as_mut_ptr() as *mut c_void),
        (buf.len() * 2) as u32,
        Some(&mut bytes),
        None,
    )
    .map_err(|e| format!("IOCTL string query (level {level}) failed: {e}"))?;

    let wchars = (bytes as usize / 2).min(buf.len());
    let slice = &buf[..wchars];
    let end = slice.iter().position(|&c| c == 0).unwrap_or(wchars);
    Ok(String::from_utf16_lossy(&slice[..end]).trim().to_string())
}
