# BugJuice

**Battery monitoring and power analytics for Windows laptops.** Real-time wattage, per-app power attribution, component breakdown, sleep drain analysis, battery health trending, and charge habit scoring — all in plain English.

Built by [DudieBug](https://dudiebug.net). Free and open source (MIT).

<!-- ![BugJuice Dashboard](docs/screenshot-dashboard.png) -->

## Features

| Category | What you get |
|----------|-------------|
| **Dashboard** | Battery %, charge/discharge rate in watts, time remaining, voltage, live 1-hour history chart. Power cards hide automatically when data isn't available. |
| **Per-app power** | Ranked process list by estimated watts — CPU and GPU breakdown per process, updated every 2 seconds |
| **Components** | CPU, GPU, DRAM, modem, NPU power via pie chart. Shows "enhanced" badge when LHM provides extra sensors, "basic" when running without it. |
| **Battery health** | Wear curve over time, cycle count, projected months until replacement, wear rate per month |
| **Charge habits** | Score out of 100 based on a rolling 30-day window — overcharge frequency, deep discharge, time at 100%, with actionable tips |
| **Sessions** | Rolling 7-day timeline of every charge/discharge cycle with per-day drill-down and summary stats (time on battery, avg drain, peak drain, sleep drain) |
| **Sleep drain** | Measures battery loss during sleep, flags abnormal drain |
| **Charge speed** | Current, peak, and average charge rate while plugged in, with ETA to full |
| **"If you unplug now"** | Predicted battery life based on current per-app usage |
| **Notifications** | Charge limit (80%) alerts, low battery warnings, periodic summaries, sleep drain alerts |
| **Power plans** | Auto-switch Windows power plans at configurable battery thresholds |
| **Export** | Full reports in JSON and PDF |
| **System tray** | Battery % tooltip, quick actions, minimize-to-tray on close |
| **Theme** | Light and dark — follows Windows system preference with accent color integration |
| **Auto-updater** | Ed25519-signed updates from GitHub Releases |

## Supported hardware

| Platform | CPU power | GPU power | Battery | Sleep drain |
|----------|-----------|-----------|---------|-------------|
| Intel 12th gen+ | RAPL via EMI or LHM | PP1 (integrated) / NVML (discrete) | IOCTL | Modern Standby |
| AMD Zen 3+ | RAPL via EMI or LHM | EMI / ADLX (future) | IOCTL | Modern Standby |
| Qualcomm Snapdragon X | EMI (CPU clusters, GPU, modem, NPU) | EMI | IOCTL | Modern Standby |

If a sensor isn't available on your hardware, the card is hidden instead of showing dashes.

## Installation

Download the latest installer from [GitHub Releases](https://github.com/Dudiebug/BugJuice/releases):

- **BugJuice_1.0.0_x64-setup.exe** — Intel / AMD laptops
- **BugJuice_1.0.0_arm64-setup.exe** — Snapdragon X laptops (Surface Pro, Lenovo Yoga, etc.)

The installer registers a small Windows service (`bugjuice-svc`) that reads privileged power sensors. One UAC prompt at install, then the main app runs as a normal user.

**x64 only (optional):** Install [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) for enhanced power monitoring. BugJuice auto-detects LHM and shows a green "enhanced" badge on the Components page. ARM64 doesn't need LHM — Snapdragon X exposes all power domains directly via EMI.

## How it works

BugJuice is two binaries:

1. **Tauri v2 app** (Rust + React) — reads unprivileged APIs: battery IOCTLs, per-process CPU time, ProcessEnergyValues, GPU utilization, and optionally LHM via WMI.
2. **Windows service** (`bugjuice-svc`) — reads privileged EMI/RAPL data and serves it over a named pipe (`\\.\pipe\bugjuice`).

All data stays local in SQLite at `%LOCALAPPDATA%\BugJuice\bugjuice.db` with configurable retention (7–90 days, default 30).

## Building from source

**Prerequisites:** [Rust](https://rustup.rs/) 1.77+, [Node.js](https://nodejs.org/) 20+, [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with "Desktop development with C++", Windows 10/11.

```bash
git clone https://github.com/Dudiebug/BugJuice.git
cd BugJuice
npm install

# Build the service
cd service && cargo build --release
cp target/release/bugjuice-svc.exe ../src-tauri/
cd ..

# (x64 only) Build the LHM helper
cd lhm-helper && dotnet publish -c Release -r win-x64
cp bin/Release/net8.0-windows/win-x64/publish/bugjuice-lhm.exe ../src-tauri/
cd ..

# Dev mode
npx tauri dev

# Production build (NSIS installer)
npx tauri build                                  # native arch
npx tauri build --target x86_64-pc-windows-msvc  # cross-compile x64
```

## License

MIT

## Credits

- [Tauri](https://tauri.app/) — app framework
- [Recharts](https://recharts.org/) — charts
- [printpdf](https://github.com/nickkha/printpdf) — PDF export
- [LibreHardwareMonitorLib](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) — enhanced power sensors (x64)
