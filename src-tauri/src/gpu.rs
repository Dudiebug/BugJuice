// Per-process GPU utilization via PDH "GPU Engine" counters.
//
// This is the exact data source Task Manager uses. No admin needed, no
// kernel driver needed, works on integrated and discrete GPUs (NVIDIA,
// AMD, Intel, Qualcomm Adreno).
//
// The counter path we query is `\GPU Engine(*)\Utilization Percentage`.
// PDH expands the wildcard into one counter per engine per process with
// instance names like:
//   pid_1234_luid_0x00000000_0x000078AF_phys_0_eng_0_engtype_3D
//
// The PID is encoded in the instance name. We sum across all engine
// types (3D, Compute, VideoEncode, VideoDecode, Copy) per PID.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;

// PDH returns LONG status codes. 0 = success.
type PdhStatus = i32;
type PdhQueryHandle = *mut c_void;
type PdhCounterHandle = *mut c_void;

const ERROR_SUCCESS: i32 = 0;
const PDH_MORE_DATA: i32 = 0x800007D2u32 as i32;
const PDH_FMT_DOUBLE: u32 = 0x00000200;

#[link(name = "pdh")]
unsafe extern "system" {
    fn PdhOpenQueryW(
        szDataSource: *const u16,
        dwUserData: usize,
        phQuery: *mut PdhQueryHandle,
    ) -> PdhStatus;
    fn PdhCloseQuery(hQuery: PdhQueryHandle) -> PdhStatus;
    fn PdhAddCounterW(
        hQuery: PdhQueryHandle,
        szFullCounterPath: *const u16,
        dwUserData: usize,
        phCounter: *mut PdhCounterHandle,
    ) -> PdhStatus;
    fn PdhCollectQueryData(hQuery: PdhQueryHandle) -> PdhStatus;
    fn PdhGetFormattedCounterArrayW(
        hCounter: PdhCounterHandle,
        dwFormat: u32,
        lpdwBufferSize: *mut u32,
        lpdwItemCount: *mut u32,
        ItemBuffer: *mut c_void,
    ) -> PdhStatus;
}

/// PDH_FMT_COUNTERVALUE — { CStatus: DWORD, <pad>, Value: union (8 bytes) }
#[repr(C)]
struct PdhFmtCounterValue {
    status: u32,
    _pad: u32,
    double_value: f64,
}

/// PDH_FMT_COUNTERVALUE_ITEM_W — { szName: LPWSTR, FmtValue: PDH_FMT_COUNTERVALUE }
#[repr(C)]
struct PdhFmtCounterValueItemW {
    name: *mut u16,
    value: PdhFmtCounterValue,
}

pub struct GpuQuery {
    query: PdhQueryHandle,
    counter: PdhCounterHandle,
    primed: bool,
}

// SAFETY: we only use the query from the polling thread.
unsafe impl Send for GpuQuery {}

impl GpuQuery {
    /// Open a PDH query for `\GPU Engine(*)\Utilization Percentage`.
    /// Returns None if PDH or the GPU Engine counter set isn't available
    /// (headless Windows servers, very old versions).
    pub fn new() -> Option<Self> {
        unsafe {
            let mut query: PdhQueryHandle = ptr::null_mut();
            if PdhOpenQueryW(ptr::null(), 0, &mut query) != ERROR_SUCCESS {
                return None;
            }
            let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0"
                .encode_utf16()
                .collect();
            let mut counter: PdhCounterHandle = ptr::null_mut();
            if PdhAddCounterW(query, path.as_ptr(), 0, &mut counter) != ERROR_SUCCESS {
                let _ = PdhCloseQuery(query);
                return None;
            }
            Some(GpuQuery {
                query,
                counter,
                primed: false,
            })
        }
    }

    /// Take a sample. Returns a PID → utilization-percent map, summed
    /// across engine types. Can exceed 100% on systems with parallel
    /// engines. First call primes the baseline and returns an empty map.
    pub fn sample(&mut self) -> HashMap<u32, f64> {
        unsafe {
            if PdhCollectQueryData(self.query) != ERROR_SUCCESS {
                return HashMap::new();
            }
            if !self.primed {
                self.primed = true;
                return HashMap::new();
            }

            // First call with null buffer to discover required size.
            let mut buf_size: u32 = 0;
            let mut item_count: u32 = 0;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut buf_size,
                &mut item_count,
                ptr::null_mut(),
            );
            if status != PDH_MORE_DATA || buf_size == 0 {
                return HashMap::new();
            }

            // Allocate and fill.
            let mut buf: Vec<u8> = vec![0u8; buf_size as usize];
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut buf_size,
                &mut item_count,
                buf.as_mut_ptr() as *mut c_void,
            );
            if status != ERROR_SUCCESS {
                return HashMap::new();
            }

            // The buffer is laid out as [Items...][string pool], with each
            // item's `name` pointer pointing into the string pool.
            let items_ptr = buf.as_ptr() as *const PdhFmtCounterValueItemW;
            let mut out: HashMap<u32, f64> = HashMap::new();
            for i in 0..item_count as usize {
                let item = ptr::read_unaligned(items_ptr.add(i));
                if item.value.status != ERROR_SUCCESS as u32 {
                    continue;
                }
                let name = read_wide_string(item.name);
                if let Some(pid) = parse_pid_from_instance(&name) {
                    if pid == 0 {
                        continue;
                    }
                    let v = item.value.double_value;
                    if v.is_finite() && v >= 0.0 {
                        *out.entry(pid).or_insert(0.0) += v;
                    }
                }
            }
            out
        }
    }
}

impl Drop for GpuQuery {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

fn parse_pid_from_instance(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("pid_")?;
    let end = rest.find('_').unwrap_or(rest.len());
    rest[..end].parse().ok()
}

unsafe fn read_wide_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
        if len > 2048 {
            break;
        }
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    String::from_utf16_lossy(slice)
}

// ─── NVML GPU Power ─────────────────────────────────────────────────────────
//
// On NVIDIA systems, nvml.dll ships with the driver and exposes actual GPU
// wattage via on-board current shunt resistors. We load it dynamically so
// the app works fine on systems without NVIDIA GPUs.

const NVML_SUCCESS: u32 = 0;

// Function pointer types matching the NVML C API.
type NvmlInit = unsafe extern "C" fn() -> u32;
type NvmlShutdown = unsafe extern "C" fn() -> u32;
type NvmlDeviceGetCount = unsafe extern "C" fn(*mut u32) -> u32;
type NvmlDeviceGetHandleByIndex = unsafe extern "C" fn(u32, *mut *mut c_void) -> u32;
type NvmlDeviceGetPowerUsage = unsafe extern "C" fn(*mut c_void, *mut u32) -> u32;

pub struct NvmlPower {
    _lib: libloading::Library,
    handles: Vec<*mut c_void>,
    fn_get_power: NvmlDeviceGetPowerUsage,
    fn_shutdown: NvmlShutdown,
}

// SAFETY: we only use NvmlPower from the polling thread.
unsafe impl Send for NvmlPower {}

impl NvmlPower {
    /// Try to load NVML and enumerate GPU devices. Returns None if NVML
    /// isn't available (no NVIDIA GPU or driver not installed).
    pub fn new() -> Option<Self> {
        // Try the standard system path first, then the NVSMI folder.
        let lib = unsafe {
            libloading::Library::new("nvml.dll")
                .or_else(|_| {
                    libloading::Library::new(
                        r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll",
                    )
                })
                .ok()?
        };

        unsafe {
            let init: libloading::Symbol<NvmlInit> = lib.get(b"nvmlInit_v2\0").ok()?;
            if init() != NVML_SUCCESS {
                return None;
            }

            let get_count: libloading::Symbol<NvmlDeviceGetCount> =
                lib.get(b"nvmlDeviceGetCount_v2\0").ok()?;
            let mut count: u32 = 0;
            if get_count(&mut count) != NVML_SUCCESS || count == 0 {
                let shutdown: libloading::Symbol<NvmlShutdown> =
                    lib.get(b"nvmlShutdown\0").ok()?;
                shutdown();
                return None;
            }

            let get_handle: libloading::Symbol<NvmlDeviceGetHandleByIndex> =
                lib.get(b"nvmlDeviceGetHandleByIndex_v2\0").ok()?;
            let mut handles = Vec::with_capacity(count as usize);
            for i in 0..count {
                let mut handle: *mut c_void = ptr::null_mut();
                if get_handle(i, &mut handle) == NVML_SUCCESS {
                    handles.push(handle);
                }
            }
            if handles.is_empty() {
                let shutdown: libloading::Symbol<NvmlShutdown> =
                    lib.get(b"nvmlShutdown\0").ok()?;
                shutdown();
                return None;
            }

            let fn_get_power: NvmlDeviceGetPowerUsage =
                *lib.get::<NvmlDeviceGetPowerUsage>(b"nvmlDeviceGetPowerUsage\0").ok()?;
            let fn_shutdown: NvmlShutdown =
                *lib.get::<NvmlShutdown>(b"nvmlShutdown\0").ok()?;

            log::info!(
                "NVML initialized: {} GPU device(s)",
                handles.len()
            );

            Some(NvmlPower {
                _lib: lib,
                handles,
                fn_get_power,
                fn_shutdown,
            })
        }
    }

    /// Read the total GPU power across all NVIDIA devices. Returns watts.
    pub fn read_total_watts(&self) -> Option<f64> {
        let mut total_mw: u64 = 0;
        let mut ok = false;
        for &handle in &self.handles {
            let mut power_mw: u32 = 0;
            if unsafe { (self.fn_get_power)(handle, &mut power_mw) } == NVML_SUCCESS {
                total_mw += power_mw as u64;
                ok = true;
            }
        }
        if ok {
            Some(total_mw as f64 / 1000.0)
        } else {
            None
        }
    }
}

impl Drop for NvmlPower {
    fn drop(&mut self) {
        unsafe {
            (self.fn_shutdown)();
        }
    }
}
