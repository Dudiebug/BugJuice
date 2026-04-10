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
}

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
