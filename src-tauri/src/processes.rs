// Per-process CPU time via NtQuerySystemInformation.
//
// Two NT info classes:
//   SystemProcessInformation (5)             — per-process times + names + PIDs
//   SystemProcessorPerformanceInformation (8) — per-CPU idle/kernel/user totals
//
// Both are documented in the Windows SDK header `winternl.h` and have been
// stable for ~25 years. No admin required.
//
// CPU time math:
//   process_cpu_share = process_delta / total_busy_delta
// where total_busy_delta is the sum across all CPUs of (kernel - idle + user)
// over the same time window.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;

// ─── Raw FFI ──────────────────────────────────────────────────────────────────
//
// windows-rs does expose NtQuerySystemInformation via the Wdk feature, but
// it doesn't include the SYSTEM_PROCESS_INFORMATION struct (correctly,
// since the struct's tail varies by Windows version). It's simpler to
// declare both manually.

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        SystemInformationClass: i32,
        SystemInformation: *mut c_void,
        SystemInformationLength: u32,
        ReturnLength: *mut u32,
    ) -> i32;
    fn NtQueryInformationProcess(
        ProcessHandle: *mut c_void,
        ProcessInformationClass: i32,
        ProcessInformation: *mut c_void,
        ProcessInformationLength: u32,
        ReturnLength: *mut u32,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut c_void;
    fn CloseHandle(hObject: *mut c_void) -> i32;
}

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

const SYSTEM_PROCESS_INFORMATION_CLASS: i32 = 5;
const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS: i32 = 8;

const STATUS_SUCCESS: i32 = 0;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC0000004u32 as i32;

/// UNICODE_STRING — buffer is a u16 pointer, length is in BYTES.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    _pad: u32, // 64-bit alignment for buffer pointer
    buffer: *mut u16,
}

/// SYSTEM_PROCESS_INFORMATION (partial — fields beyond unique_process_id
/// are not used and have varied across Windows versions).
#[repr(C)]
struct SystemProcessInfoHeader {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,   // 100-ns units, accumulated since process start
    kernel_time: i64, // 100-ns units, accumulated since process start
    image_name: UnicodeString,
    base_priority: i32,
    _pad_after_priority: u32,
    unique_process_id: *mut c_void, // PID stored as a HANDLE
    // … fields after this point we don't read.
}

/// SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION — one entry per logical CPU.
#[repr(C)]
struct SystemProcessorPerfInfo {
    idle_time: i64,   // 100-ns
    kernel_time: i64, // 100-ns, INCLUDES idle_time
    user_time: i64,   // 100-ns
    reserved1: [i64; 2],
    reserved2: u32,
}

// ─── Public types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct ProcessSample {
    pub pid: u32,
    pub name: String,
    pub user_time_100ns: i64,
    pub kernel_time_100ns: i64,
}

impl ProcessSample {
    fn total_time_100ns(&self) -> i64 {
        self.user_time_100ns + self.kernel_time_100ns
    }
}

#[derive(Default, Clone, Debug)]
pub struct ProcessorTotals {
    pub idle_100ns: i64,
    pub kernel_100ns: i64, // includes idle (Windows convention)
    pub user_100ns: i64,
}

impl ProcessorTotals {
    /// Time spent doing useful work across all CPUs, summed.
    pub fn busy_100ns(&self) -> i64 {
        // kernel_time INCLUDES idle, so subtract it back out
        (self.kernel_100ns - self.idle_100ns) + self.user_100ns
    }
}

#[derive(Debug, Clone)]
pub struct ProcessDelta {
    pub pid: u32,
    pub name: String,
    /// CPU 100ns this process used during the window.
    pub cpu_100ns: i64,
    /// Fraction of total system busy time during the window. 0.0..~1.0.
    pub share: f64,
}

// ─── Snapshots ────────────────────────────────────────────────────────────────

/// Take a snapshot of every process on the system, with cumulative CPU
/// times. Cost: ~1-5 ms on a typical desktop, no admin needed.
pub fn snapshot_processes() -> Result<Vec<ProcessSample>, String> {
    unsafe {
        let mut size: u32 = 256 * 1024; // 256 KB; will grow if needed
        let mut buf: Vec<u8> = vec![0u8; size as usize];

        loop {
            let mut returned: u32 = 0;
            let status = NtQuerySystemInformation(
                SYSTEM_PROCESS_INFORMATION_CLASS,
                buf.as_mut_ptr() as *mut c_void,
                size,
                &mut returned,
            );

            if status == STATUS_SUCCESS {
                break;
            }
            if status == STATUS_INFO_LENGTH_MISMATCH {
                size = size.saturating_mul(2);
                if size > 16 * 1024 * 1024 {
                    return Err("process info buffer exceeded 16 MB".into());
                }
                buf.resize(size as usize, 0);
                continue;
            }
            return Err(format!(
                "NtQuerySystemInformation(processes) failed: 0x{:08X}",
                status
            ));
        }

        // Walk the linked list. Each entry's NextEntryOffset points from
        // the start of THIS entry to the start of the next; 0 = end.
        let mut out = Vec::with_capacity(256);
        let mut offset: usize = 0;
        loop {
            let entry_ptr = buf.as_ptr().add(offset) as *const SystemProcessInfoHeader;
            let info = ptr::read_unaligned(entry_ptr);

            let pid = info.unique_process_id as usize as u32;
            let name = if !info.image_name.buffer.is_null() && info.image_name.length > 0 {
                let len_chars = (info.image_name.length / 2) as usize;
                let slice = std::slice::from_raw_parts(info.image_name.buffer, len_chars);
                String::from_utf16_lossy(slice)
            } else if pid == 0 {
                "Idle".to_string()
            } else if pid == 4 {
                "System".to_string()
            } else {
                format!("pid_{pid}")
            };

            out.push(ProcessSample {
                pid,
                name,
                user_time_100ns: info.user_time,
                kernel_time_100ns: info.kernel_time,
            });

            if info.next_entry_offset == 0 {
                break;
            }
            offset += info.next_entry_offset as usize;
        }

        Ok(out)
    }
}

/// Take a snapshot of total CPU time across all logical processors.
pub fn snapshot_processor_totals() -> Result<ProcessorTotals, String> {
    unsafe {
        // Allocate generously — a 256-CPU machine still fits in 12 KB.
        let n_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(64);
        let needed = (n_cpus * size_of::<SystemProcessorPerfInfo>()) as u32;
        let mut buf = vec![0u8; needed as usize];
        let mut returned: u32 = 0;

        let status = NtQuerySystemInformation(
            SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION_CLASS,
            buf.as_mut_ptr() as *mut c_void,
            needed,
            &mut returned,
        );
        if status != STATUS_SUCCESS {
            return Err(format!(
                "NtQuerySystemInformation(cpu) failed: 0x{:08X}",
                status
            ));
        }

        let actual_count = returned as usize / size_of::<SystemProcessorPerfInfo>();
        let mut totals = ProcessorTotals::default();
        let arr = buf.as_ptr() as *const SystemProcessorPerfInfo;
        for i in 0..actual_count {
            let p = ptr::read_unaligned(arr.add(i));
            totals.idle_100ns += p.idle_time;
            totals.kernel_100ns += p.kernel_time;
            totals.user_100ns += p.user_time;
        }
        Ok(totals)
    }
}

// ─── Delta computation ────────────────────────────────────────────────────────

/// Diff two snapshots and return processes sorted by CPU consumption,
/// highest first. Skips processes that didn't accumulate any CPU during
/// the window and processes that didn't exist in both snapshots.
pub fn compute_deltas(
    prev: &[ProcessSample],
    curr: &[ProcessSample],
    prev_totals: &ProcessorTotals,
    curr_totals: &ProcessorTotals,
) -> Vec<ProcessDelta> {
    let prev_map: HashMap<u32, &ProcessSample> = prev.iter().map(|p| (p.pid, p)).collect();
    let total_busy = (curr_totals.busy_100ns() - prev_totals.busy_100ns()).max(1);

    let mut out: Vec<ProcessDelta> = curr
        .iter()
        .filter_map(|p| {
            // Skip the System Idle pseudo-process (PID 0). Its kernel_time
            // accumulates per-CPU idle time, which is not real work and
            // would otherwise dominate the rankings.
            if p.pid == 0 {
                return None;
            }
            let prev_p = prev_map.get(&p.pid)?;
            let cpu = p.total_time_100ns() - prev_p.total_time_100ns();
            if cpu <= 0 {
                return None;
            }
            Some(ProcessDelta {
                pid: p.pid,
                name: p.name.clone(),
                cpu_100ns: cpu,
                share: cpu as f64 / total_busy as f64,
            })
        })
        .collect();
    out.sort_by(|a, b| b.cpu_100ns.cmp(&a.cpu_100ns));
    out
}

/// System-wide idle fraction over a window: 0.0..1.0.
pub fn idle_fraction(prev: &ProcessorTotals, curr: &ProcessorTotals) -> f64 {
    let idle_delta = (curr.idle_100ns - prev.idle_100ns).max(0) as f64;
    let total_delta =
        ((curr.kernel_100ns + curr.user_100ns) - (prev.kernel_100ns + prev.user_100ns)).max(1) as f64;
    (idle_delta / total_delta).clamp(0.0, 1.0)
}

// ─── ProcessEnergyValues ─────────────────────────────────────────────────────
//
// Windows E3 tracks per-process energy internally. We read it via the
// undocumented NtQueryInformationProcess(ProcessEnergyValues) info class.
// Struct layout from phnt headers (github.com/winsiderss/phnt).
//
// These counters are cumulative — snapshot twice and subtract for a delta.
// No admin required (PROCESS_QUERY_LIMITED_INFORMATION access).

const PROCESS_ENERGY_VALUES_INFO_CLASS: i32 = 77; // ProcessEnergyValues

/// Raw struct returned by NtQueryInformationProcess(ProcessEnergyValues).
/// Layout from phnt/ntapi: total size = 0x104 bytes (260).
#[repr(C)]
#[derive(Clone, Copy)]
struct RawProcessEnergyValues {
    /// CPU cycles: [2 groups][4 entries] — accumulated cycles per processor set.
    cycles: [[u64; 4]; 2],
    disk_energy: u64,
    network_tail_energy: u64,
    mbb_tail_energy: u64,
    network_tx_rx_bytes: u64,
    mbb_tx_rx_bytes: u64,
    foreground_duration: u32,
    desktop_visible_duration: u32,
    psm_foreground_duration: u32,
    composition_rendered: u32,
    composition_dirty_generated: u32,
    composition_dirty_propagated: u32,
    reserved1: u32,
    /// Attributed CPU cycles: [4 groups][2 entries].
    attributed_cycles: [[u64; 2]; 4],
    /// Work-on-behalf cycles: [4 groups][2 entries].
    work_on_behalf_cycles: [[u64; 2]; 4],
}

/// Simplified snapshot of per-process energy counters.
#[derive(Clone, Debug)]
pub struct ProcessEnergySnapshot {
    pub pid: u32,
    /// Sum of all CPU cycle groups (cumulative).
    pub cpu_cycles: u64,
    /// Disk energy counter (cumulative, arbitrary units).
    pub disk_energy: u64,
    /// Network tail energy counter (cumulative, arbitrary units).
    pub network_energy: u64,
}

/// Per-process energy delta between two snapshots.
#[derive(Clone, Debug)]
pub struct ProcessEnergyDelta {
    pub pid: u32,
    pub name: String,
    pub cpu_cycles_delta: u64,
    pub disk_energy_delta: u64,
    pub network_energy_delta: u64,
}

/// Whether ProcessEnergyValues is available on this Windows build.
/// Cached after first attempt so we don't retry on every tick.
static ENERGY_API_AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn energy_api_available() -> bool {
    *ENERGY_API_AVAILABLE.get().unwrap_or(&true)
}

/// Query ProcessEnergyValues for a set of PIDs. Returns a map of
/// PID → snapshot. PIDs that can't be opened are silently skipped.
pub fn snapshot_energy_values(pids: &[u32]) -> HashMap<u32, ProcessEnergySnapshot> {
    let mut out = HashMap::with_capacity(pids.len());

    // Early exit if we already know the API isn't available.
    if !energy_api_available() {
        return out;
    }

    for &pid in pids {
        if pid == 0 || pid == 4 {
            continue; // skip Idle and System
        }

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                continue;
            }

            let mut raw: RawProcessEnergyValues = std::mem::zeroed();
            let mut ret_len: u32 = 0;
            let status = NtQueryInformationProcess(
                handle,
                PROCESS_ENERGY_VALUES_INFO_CLASS,
                &mut raw as *mut _ as *mut c_void,
                size_of::<RawProcessEnergyValues>() as u32,
                &mut ret_len,
            );

            CloseHandle(handle);

            if status == STATUS_SUCCESS {
                // Sum all cycle groups into one value.
                let mut total_cycles: u64 = 0;
                for group in &raw.cycles {
                    for &val in group {
                        total_cycles = total_cycles.wrapping_add(val);
                    }
                }

                out.insert(
                    pid,
                    ProcessEnergySnapshot {
                        pid,
                        cpu_cycles: total_cycles,
                        disk_energy: raw.disk_energy,
                        network_energy: raw.network_tail_energy,
                    },
                );
            } else if status == 0xC0000003u32 as i32 {
                // STATUS_INVALID_INFO_CLASS — API not available on this build.
                ENERGY_API_AVAILABLE.set(false).ok();
                log::warn!(
                    "ProcessEnergyValues (info class 77) not available; \
                     falling back to CPU-time attribution"
                );
                break;
            }
            // Other errors (access denied, etc.) → skip this PID silently.
        }
    }
    out
}

/// Compute per-process energy deltas between two snapshots.
/// Processes that are in `curr` but not `prev` are skipped (first tick).
/// Processes where any counter went backwards (restarted) are skipped.
pub fn compute_energy_deltas(
    prev: &HashMap<u32, ProcessEnergySnapshot>,
    curr: &HashMap<u32, ProcessEnergySnapshot>,
    names: &HashMap<u32, String>,
) -> Vec<ProcessEnergyDelta> {
    let mut out = Vec::new();
    for (pid, c) in curr {
        let Some(p) = prev.get(pid) else { continue };
        // Wrapping subtraction handles counter overflow gracefully.
        let cpu_delta = c.cpu_cycles.wrapping_sub(p.cpu_cycles);
        let disk_delta = c.disk_energy.wrapping_sub(p.disk_energy);
        let net_delta = c.network_energy.wrapping_sub(p.network_energy);
        // Skip if all zeros (process did nothing) or absurdly large (restart).
        if cpu_delta == 0 && disk_delta == 0 && net_delta == 0 {
            continue;
        }
        if cpu_delta > u64::MAX / 2 {
            continue; // counter wrapped or process restarted
        }
        let name = names
            .get(pid)
            .cloned()
            .unwrap_or_else(|| format!("pid_{pid}"));
        out.push(ProcessEnergyDelta {
            pid: *pid,
            name,
            cpu_cycles_delta: cpu_delta,
            disk_energy_delta: disk_delta,
            network_energy_delta: net_delta,
        });
    }
    out.sort_by(|a, b| b.cpu_cycles_delta.cmp(&a.cpu_cycles_delta));
    out
}
