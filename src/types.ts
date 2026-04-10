// Data shapes the frontend will consume. These mirror the Rust backend's
// database schema (cli/src/storage.rs). When we merge into Tauri the
// `invoke('get_battery_status')` → `BatteryStatus` mapping will line up
// 1:1 with these types.

export interface BatteryStatus {
  percent: number;              // 0..100
  capacityMwh: number;          // current mWh
  fullChargeMwh: number;        // max mWh
  designMwh: number;            // designed mWh
  voltageV: number;             // current voltage
  rateW: number;                // signed: + = charging, - = discharging, 0 = idle
  powerState: 'charging' | 'discharging' | 'full' | 'idle' | 'critical';
  onAc: boolean;
  tempC: number | null;
  // Pre-computed human context
  etaMinutes: number | null;    // null = unknown/idle
  etaLabel: string;             // "~1h 24m left at this rate"
  chemistry: string;
  cycleCount: number;
  wearPercent: number;
  manufacturer: string;
  deviceName: string;
}

export interface PowerReading {
  // Current wattages across the measurable power channels. Any of these
  // may be null if the hardware doesn't expose it.
  wallInputW: number | null;    // PSU_USB / USBC_TOTAL
  systemDrawW: number | null;   // SYS / PSys
  cpuPackageW: number | null;   // PKG or sum of CPU_CLUSTER_*
  gpuW: number | null;          // PP1 or GPU channel
  dramW: number | null;         // DRAM (Intel only)
  source: string;               // "EMI v2 — Qualcomm 8380" / "Microsoft PPM"
  channels: PowerChannel[];     // Raw per-channel values for diagnostics
}

export interface PowerChannel {
  name: string;
  watts: number;
}

export interface AppPowerRow {
  pid: number;
  name: string;
  cpuW: number;
  gpuW: number;
  diskW: number;
  netW: number;
  totalW: number;
}

export interface BatterySession {
  id: number;
  startedAt: number;            // unix seconds
  endedAt: number | null;
  startPercent: number;
  endPercent: number | null;
  startCapacity: number;
  endCapacity: number | null;
  avgDrainW: number | null;
  onAc: boolean;
}

export interface SleepSession {
  id: number;
  sleepAt: number;              // unix seconds
  wakeAt: number | null;
  preCapacity: number;
  postCapacity: number | null;
  drainMwh: number | null;
  drainPercent: number | null;
  drainRateMw: number | null;
  dripsPercent: number | null;
  verdict: 'excellent' | 'normal' | 'high' | 'very-high' | null;
}

export interface HealthSnapshot {
  ts: number;
  designCapacity: number;
  fullChargeCapacity: number;
  cycleCount: number;
  wearPercent: number;
}

export interface HistoryPoint {
  ts: number;                   // unix seconds
  percent: number;
  rateW: number;                // negative = discharging
}
