// BugJuice service — reads EMI power data and serves it over a named pipe.
//
// Usage:
//   bugjuice-svc.exe install    — register as a Windows service
//   bugjuice-svc.exe uninstall  — stop and remove the service
//   (no args)                   — run as service (called by SCM)

#![allow(unsafe_op_in_unsafe_fn)]

mod emi;

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use emi::EmiReading;
use serde::Serialize;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, ERROR_PIPE_CONNECTED};
use windows::Win32::Security::{
    InitializeSecurityDescriptor, SetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR,
    SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
};
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;
use windows::Win32::Storage::FileSystem::{
    FILE_FLAGS_AND_ATTRIBUTES, FlushFileBuffers, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
    PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
const PIPE_ACCESS_DUPLEX: u32 = 0x00000003;
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, RegisterServiceCtrlHandlerW,
    SC_MANAGER_ALL_ACCESS, SERVICE_ACCEPT_STOP, SERVICE_ALL_ACCESS,
    SERVICE_AUTO_START, SERVICE_CONTROL_STOP as SVC_CTRL_STOP,
    SERVICE_ERROR_NORMAL, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP_PENDING, SERVICE_STOPPED,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceStatus,
    StartServiceCtrlDispatcherW, StartServiceW,
};

// ─── Globals ─────────────────────────────────────────────────────────────────

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static mut STATUS_HANDLE: isize = 0;

const PIPE_NAME: PCWSTR = windows::core::w!("\\\\.\\pipe\\bugjuice");
const SERVICE_NAME: PCWSTR = windows::core::w!("BugJuice");

// ─── IPC types ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PipeResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    readings: Option<Vec<EmiReading>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ─── Entry point ─────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("install") => install_service(),
        Some("uninstall") => uninstall_service(),
        _ => run_as_service(),
    }
}

// ─── Install / Uninstall ─────────────────────────────────────────────────────

fn install_service() {
    // Use the Win32 SCM API directly — sc.exe argument parsing is unreliable.
    let exe = std::env::current_exe().expect("failed to get exe path");
    let exe_wide: Vec<u16> = exe
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[bugjuice-svc] OpenSCManager failed: {e}");
                eprintln!("[bugjuice-svc] (are you running as admin?)");
                return;
            }
        };

        let svc = match CreateServiceW(
            scm,
            windows::core::w!("BugJuice"),
            windows::core::w!("BugJuice Power Monitor"),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(exe_wide.as_ptr()),
            None, None, None, None, None,
        ) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[bugjuice-svc] CreateService failed: {e}");
                let _ = CloseServiceHandle(scm);
                return;
            }
        };

        println!("[bugjuice-svc] Service installed successfully.");

        // Start the service.
        match StartServiceW(svc, None) {
            Ok(_) => println!("[bugjuice-svc] Service started."),
            Err(e) => eprintln!("[bugjuice-svc] StartService failed: {e}"),
        }

        let _ = CloseServiceHandle(svc);
        let _ = CloseServiceHandle(scm);
    }
}

fn uninstall_service() {
    unsafe {
        let scm = match OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[bugjuice-svc] OpenSCManager failed: {e}");
                return;
            }
        };

        let svc = match OpenServiceW(scm, windows::core::w!("BugJuice"), SERVICE_ALL_ACCESS) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[bugjuice-svc] OpenService failed: {e}");
                let _ = CloseServiceHandle(scm);
                return;
            }
        };

        // Stop the service (ignore errors — it might already be stopped).
        let mut status: SERVICE_STATUS = zeroed();
        let _ = ControlService(svc, SVC_CTRL_STOP, &mut status);
        std::thread::sleep(Duration::from_secs(2));

        // Delete it.
        match DeleteService(svc) {
            Ok(_) => println!("[bugjuice-svc] Service removed."),
            Err(e) => eprintln!("[bugjuice-svc] DeleteService failed: {e}"),
        }

        let _ = CloseServiceHandle(svc);
        let _ = CloseServiceHandle(scm);
    }
}

// ─── Service runtime ─────────────────────────────────────────────────────────

fn run_as_service() {
    unsafe {
        let table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: windows::core::PWSTR(SERVICE_NAME.as_ptr() as *mut u16),
                lpServiceProc: Some(service_main),
            },
            zeroed(),
        ];
        let _ = StartServiceCtrlDispatcherW(table.as_ptr());
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows::core::PWSTR) {
    let handle = RegisterServiceCtrlHandlerW(SERVICE_NAME, Some(control_handler));
    match handle {
        Ok(h) => STATUS_HANDLE = h.0 as isize,
        Err(_) => return,
    }

    report_status(SERVICE_START_PENDING, 0);

    // Shared EMI data: polled by the EMI thread, read by the pipe thread.
    let emi_data: Arc<Mutex<Vec<EmiReading>>> = Arc::new(Mutex::new(Vec::new()));

    // EMI polling thread — reads every 2 seconds.
    let emi_clone = emi_data.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("emi-poll".into())
        .spawn(move || emi_poll_loop(emi_clone))
    {
        eprintln!("[service] failed to spawn EMI poll thread: {e}");
    }

    // Named pipe server thread.
    let pipe_clone = emi_data.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("pipe-server".into())
        .spawn(move || pipe_server_loop(pipe_clone))
    {
        eprintln!("[service] failed to spawn pipe server thread: {e}");
    }

    report_status(SERVICE_RUNNING, 0);

    // Block until shutdown signal.
    while !SHUTDOWN.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(500));
    }

    report_status(SERVICE_STOPPED, 0);
}

unsafe extern "system" fn control_handler(control: u32) {
    // SERVICE_CONTROL_STOP = 1
    if control == 1 {
        report_status(SERVICE_STOP_PENDING, 0);
        SHUTDOWN.store(true, Ordering::Relaxed);
    }
}

unsafe fn report_status(state: SERVICE_STATUS_CURRENT_STATE, exit_code: u32) {
    let accept: u32 = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP
    } else {
        0
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accept,
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING {
            3000
        } else {
            0
        },
    };
    let handle = windows::Win32::System::Services::SERVICE_STATUS_HANDLE(
        STATUS_HANDLE as *mut c_void,
    );
    let _ = SetServiceStatus(handle, &status);
}

// ─── EMI polling loop ────────────────────────────────────────────────────────

fn emi_poll_loop(data: Arc<Mutex<Vec<EmiReading>>>) {
    while !SHUTDOWN.load(Ordering::Relaxed) {
        match emi::read_all_emi(Duration::from_secs(1)) {
            Ok(readings) => {
                if let Ok(mut lock) = data.lock() {
                    *lock = readings;
                }
            }
            Err(_) => {
                // EMI not available — clear stale data.
                if let Ok(mut lock) = data.lock() {
                    lock.clear();
                }
            }
        }
        // Wait before next poll (the read itself takes ~1s due to the delay).
        for _ in 0..10 {
            if SHUTDOWN.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// ─── Named pipe server ──────────────────────────────────────────────────────

fn pipe_server_loop(data: Arc<Mutex<Vec<EmiReading>>>) {
    while !SHUTDOWN.load(Ordering::Relaxed) {
        // Create a new pipe instance with NULL DACL so non-admin clients
        // can connect. This is CRITICAL — without it, the Tauri app
        // (running as normal user) gets ACCESS_DENIED.
        let pipe = match create_pipe() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("[pipe] create failed: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        // Wait for a client to connect.
        unsafe {
            let result = ConnectNamedPipe(pipe, None);
            let connected = result.is_ok()
                || windows::Win32::Foundation::GetLastError() == ERROR_PIPE_CONNECTED;
            if !connected {
                let _ = CloseHandle(pipe);
                continue;
            }
        }

        // Read command from client (up to 256 bytes).
        let mut cmd_buf = [0u8; 256];
        let mut cmd_len: u32 = 0;
        unsafe {
            let _ = ReadFile(pipe, Some(&mut cmd_buf), Some(&mut cmd_len), None);
        }

        // Parse command and build response.
        let cmd = std::str::from_utf8(&cmd_buf[..cmd_len as usize])
            .unwrap_or("")
            .trim();

        let response = if cmd.contains("read_emi") {
            let readings = data.lock().map(|d| d.clone()).unwrap_or_default();
            PipeResponse {
                ok: true,
                readings: Some(readings),
                error: None,
            }
        } else {
            PipeResponse {
                ok: false,
                readings: None,
                error: Some(format!("unknown command: {cmd}")),
            }
        };

        // Write JSON response.
        let json = serde_json::to_vec(&response).unwrap_or_default();
        unsafe {
            let mut written: u32 = 0;
            let _ = WriteFile(pipe, Some(&json), Some(&mut written), None);
            let _ = FlushFileBuffers(pipe);
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
    }
}

fn create_pipe() -> Result<HANDLE, String> {
    unsafe {
        // NULL DACL = allow everyone to connect. CRITICAL — without this,
        // the Tauri app (running as normal user) gets ACCESS_DENIED.
        let mut sd: SECURITY_DESCRIPTOR = zeroed();
        let sd_ptr = PSECURITY_DESCRIPTOR(&mut sd as *mut _ as *mut c_void);
        InitializeSecurityDescriptor(sd_ptr, SECURITY_DESCRIPTOR_REVISION)
            .map_err(|e| format!("InitializeSecurityDescriptor: {e}"))?;

        SetSecurityDescriptorDacl(sd_ptr, true, None, false)
            .map_err(|e| format!("SetSecurityDescriptorDacl: {e}"))?;

        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd_ptr.0,
            bInheritHandle: false.into(),
        };

        let handle = CreateNamedPipeW(
            PIPE_NAME,
            FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX),
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            4096, // out buffer
            4096, // in buffer
            0,    // default timeout
            Some(&sa),
        );

        if handle == INVALID_HANDLE_VALUE {
            return Err("CreateNamedPipeW returned INVALID_HANDLE_VALUE".into());
        }

        Ok(handle)
    }
}
