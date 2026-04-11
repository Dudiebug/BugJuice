# Contributing to BugJuice

Thanks for your interest in contributing! BugJuice is open source and welcomes pull requests.

## Getting Started

```bash
git clone https://github.com/Dudiebug/BugJuice.git
cd BugJuice
npm install

# Build the service binary (needed for privileged sensor reads)
cd service && cargo build --release && cp target/release/bugjuice-svc.exe ../src-tauri/ && cd ..

# Start in dev mode (hot-reload for frontend, rebuilds Rust on change)
npx tauri dev
```

**Requirements:** Rust stable (1.77.2+), Node.js 20+, Visual Studio Build Tools with C++ workload, Windows 10/11.

## Architecture

```
BugJuice/
├── src-tauri/src/       # Rust backend (Tauri v2)
│   ├── battery.rs       # Battery IOCTL interface
│   ├── commands.rs      # Tauri command surface (frontend ↔ backend)
│   ├── events.rs        # Power event callbacks (AC/DC, sleep/wake)
│   ├── gpu.rs           # GPU utilization (PDH) + NVML power
│   ├── lib.rs           # App setup, tray, plugins
│   ├── pipe_client.rs   # Named pipe client to service
│   ├── polling.rs       # Sensor polling loop + power attribution
│   ├── power.rs         # EMI power reading definitions
│   ├── power_plan.rs    # Windows power plan auto-switching
│   ├── processes.rs     # Per-process CPU time + ProcessEnergyValues
│   └── storage.rs       # SQLite persistence layer
├── service/src/         # Windows service (runs as SYSTEM)
│   └── main.rs          # EMI polling + named pipe server
├── src/                 # React frontend (TypeScript)
│   ├── pages/           # Dashboard, Components, Apps, Sessions, Health, Settings
│   ├── components/      # Sidebar, BatteryGauge, Layout
│   ├── hooks/           # useApi polling hook
│   └── api.ts           # Tauri invoke wrappers
└── src-tauri/
    ├── tauri.conf.json  # Tauri config (bundle, plugins, updater)
    └── nsis-hooks.nsi   # Installer service registration
```

**Data flow:** Polling thread (5-30s adaptive) reads sensors → logs to SQLite → frontend polls Tauri commands every 2s → React renders.

## Code Style

- **Rust:** `cargo fmt` before committing. No clippy warnings on `warn` level.
- **TypeScript:** Follow existing patterns. No linter configured yet.
- **Commits:** Concise imperative messages. One logical change per commit.

## Pull Request Process

1. Fork the repo and create a feature branch from `master`
2. Make your changes
3. Ensure `cargo check` passes in `src-tauri/` and `service/`
4. Ensure `npm run build` passes
5. Open a PR with a clear description of what and why

## Reporting Bugs

Open an issue on GitHub with:
- Windows version and CPU type (Intel/AMD/Snapdragon)
- What you expected vs what happened
- Steps to reproduce
- BugJuice version (Settings page, bottom)
