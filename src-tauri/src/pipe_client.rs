// Named pipe client — connects to the bugjuice-service to read EMI data.
//
// The service runs as SYSTEM and handles privileged EMI reads. This client
// connects to \\.\pipe\bugjuice, sends a command, and reads the JSON response.
// If the service isn't running, we gracefully return an empty vec — the app
// still works for battery, process CPU, and GPU data without the service.

use std::io::{Read, Write};
use std::time::Duration;

use crate::power::{EmiReading, PowerChannel};
use serde::Deserialize;

const PIPE_PATH: &str = r"\\.\pipe\bugjuice";
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
struct PipeResponse {
    ok: bool,
    readings: Option<Vec<EmiReadingWire>>,
    #[allow(dead_code)]
    error: Option<String>,
}

/// Wire format matches the service's serde output.
#[derive(Deserialize)]
struct EmiReadingWire {
    version: u16,
    oem: String,
    model: String,
    channels: Vec<PowerChannelWire>,
}

#[derive(Deserialize)]
struct PowerChannelWire {
    name: String,
    watts: f64,
}

/// Read the latest EMI data from the service. Returns an empty vec if the
/// service isn't running or the read fails — this is a graceful fallback,
/// not an error.
pub fn read_emi() -> Result<Vec<EmiReading>, String> {
    // On Windows, named pipes can be opened with std::fs::OpenOptions.
    // We use a short timeout to avoid blocking the polling thread.
    use std::os::windows::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(0) // FILE_ATTRIBUTE_NORMAL
        .open(PIPE_PATH);

    let mut pipe = match file {
        Ok(f) => f,
        Err(_) => {
            // Service not running or pipe doesn't exist — graceful fallback.
            return Ok(Vec::new());
        }
    };

    // Set a 2-second read timeout on the pipe handle so a stalled service
    // can't block the polling thread forever.
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::System::Pipes::SetNamedPipeHandleState;
        use windows::Win32::Foundation::HANDLE;
        let raw = pipe.as_raw_handle();
        let mut timeout_ms: u32 = TIMEOUT.as_millis() as u32;
        unsafe {
            let _ = SetNamedPipeHandleState(
                HANDLE(raw),
                None,
                None,
                Some(&mut timeout_ms),
            );
        }
    }

    // Send command.
    let cmd = b"{\"cmd\":\"read_emi\"}\n";
    if pipe.write_all(cmd).is_err() {
        return Ok(Vec::new());
    }
    if pipe.flush().is_err() {
        return Ok(Vec::new());
    }

    // Read response. The service writes JSON and disconnects, so we read
    // until EOF. Limit to 64KB to avoid unbounded allocation.
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let deadline = std::time::Instant::now() + TIMEOUT;

    loop {
        if std::time::Instant::now() > deadline {
            break;
        }
        match pipe.read(&mut chunk) {
            Ok(0) => break,        // EOF — server disconnected
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
        if buf.len() > 65536 {
            break;
        }
    }

    if buf.is_empty() {
        return Ok(Vec::new());
    }

    // Parse JSON response.
    let resp: PipeResponse = match serde_json::from_slice(&buf) {
        Ok(r) => r,
        Err(_) => return Ok(Vec::new()),
    };

    if !resp.ok {
        return Ok(Vec::new());
    }

    // Convert wire types to the app's internal types.
    let readings = resp
        .readings
        .unwrap_or_default()
        .into_iter()
        .map(|r| EmiReading {
            version: r.version,
            oem: r.oem,
            model: r.model,
            channels: r
                .channels
                .into_iter()
                .map(|c| PowerChannel {
                    name: c.name,
                    watts: c.watts,
                })
                .collect(),
        })
        .collect();

    Ok(readings)
}
