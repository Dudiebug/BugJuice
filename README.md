# BugJuice

**Battery monitoring and analytics for Windows.** Real-time wattage, per-app power attribution, sleep drain analysis, battery health trending — all with plain-english context, no raw numbers without explanation.

Built by [DudieBug](https://github.com/Dudiebug). Free and open source.

<!-- ![BugJuice Dashboard](docs/screenshot-dashboard.png) -->

## Features

- **Real-time dashboard** — battery percentage, charge/discharge rate in watts, time remaining, voltage, temperature
- **Per-app power rankings** — see which processes drain the most battery, with confidence scoring
- **Component breakdown** — CPU, GPU, DRAM, modem, and NPU power via pie chart and stacked area chart
- **Battery health tracking** — wear curve over months, cycle count, projected lifespan
- **Charge habit scoring** — 0-100 score based on your charging patterns, with actionable tips
- **Sleep drain detection** — measures battery loss during sleep, flags abnormal drain
- **Session logging** — every unplug-to-plug cycle tracked with detailed drill-down
- **Charge speed tracking** — current, peak, and average charge rate while plugged in
- **"Before I unplug" estimate** — predicted battery life per-app based on current usage
- **Power plan auto-switching** — automatically switch Windows power plan at configurable battery thresholds
- **Smart notifications** — charge limit alerts, low battery warnings, periodic summaries, sleep drain alerts
- **Export** — full reports in JSON and PDF
- **Auto-updater** — Ed25519-signed updates from GitHub Releases
- **System tray** — battery percentage tooltip, quick actions, minimize to tray on close
- **Light and dark theme** — follows Windows system preference, with accent color integration

## Supported Hardware

| Platform | CPU Power | GPU Power | Battery | Sleep Drain |
|----------|-----------|-----------|---------|-------------|
| Intel 12th gen+ | RAPL PP0/PP1 via EMI | NVML (discrete) or PP1 (integrated) | IOCTL | Modern Standby |
| AMD Zen 3+ | RAPL via EMI | ADLX (future) or EMI | IOCTL | Modern Standby |
| Qualcomm Snapdragon X | EMI (CPU clusters, GPU, modem, NPU) | EMI | IOCTL | Modern Standby |

Graceful degradation: if a sensor isn't available on your hardware, the feature is hidden rather than showing "N/A."

## Installation

Download the latest installer from [GitHub Releases](https://github.com/Dudiebug/BugJuice/releases):

- **BugJuice_x64-setup.exe** — Intel/AMD laptops
- **BugJuice_arm64-setup.exe** — Snapdragon X laptops (Surface, Lenovo Yoga, etc.)

The installer registers a small Windows service (`bugjuice-svc`) that reads privileged power sensors. One UAC prompt at install, never again. The main app runs as a normal user.

**Optional (x64 only):** Install [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) for enhanced power monitoring — per-core CPU power, AMD GPU power, and additional sensors. BugJuice automatically detects LHM and surfaces extra data when available.

## How It Works

BugJuice is two binaries: a **Tauri v2 app** (Rust backend + React frontend) that reads unprivileged APIs (battery IOCTLs, GPU utilization, per-process CPU time, ProcessEnergyValues), and a **Windows service** that reads privileged EMI/RAPL data over a named pipe. All data is stored locally in SQLite.

## Building from Source

**Prerequisites:**
- [Rust](https://rustup.rs/) (stable, 1.77.2+)
- [Node.js](https://nodejs.org/) (20+)
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/) with "Desktop development with C++"
- Windows 10/11

```bash
git clone https://github.com/Dudiebug/BugJuice.git
cd BugJuice
npm install

# Build the service
cd service
cargo build --release
cp target/release/bugjuice-svc.exe ../src-tauri/
cd ..

# Dev mode
npx tauri dev

# Production build (creates NSIS installer)
npx tauri build
```

## License

MIT

## Credits

- [Tauri](https://tauri.app/) — app framework
- [Recharts](https://recharts.org/) — charts
- [printpdf](https://github.com/nickkha/printpdf) — PDF generation
- Logo: shield beetle with lightning bolt, designed for BugJuice
