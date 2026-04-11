// Single source of truth for getting data into the React UI.
//
// This file ONLY talks to the real Rust backend via Tauri `invoke()`.
// There is no mock fallback: if the backend isn't available or returns
// nothing, we return empty/zero values and let pages render empty states.
// Running outside Tauri (vite preview) will show empty states too.

import type {
  AppPowerRow,
  BatterySession,
  BatteryStatus,
  HealthSnapshot,
  HistoryPoint,
  PowerReading,
  SleepSession,
} from './types';

// ─── Detail shapes (were previously in mock.ts) ──────────────────────────

export interface ComponentHistoryPoint {
  ts: number;
  cpu: number;
  gpu: number;
  dram: number;
  modem: number;
  npu: number;
  other: number;
}

export interface AppPowerSummary {
  name: string;
  avgWatts: number;
  maxWatts: number;
  sampleCount: number;
  totalEnergy: number;
}

export interface SessionDetailPoint {
  ts: number;
  percent: number;
  rateW: number;
}

export interface SessionDetail {
  history: SessionDetailPoint[];
  minRateW: number;
  maxRateW: number;
  avgRateW: number;
  totalEnergyMwh: number;
  durationSec: number;
}

export interface SleepDetailPoint {
  ts: number;
  capacity: number;
  drainRateMw: number;
}

export interface SleepDetail {
  history: SleepDetailPoint[];
  minRateMw: number;
  maxRateMw: number;
  avgRateMw: number;
  totalDrainMwh: number;
  durationSec: number;
}

// ─── Tauri plumbing ──────────────────────────────────────────────────────

function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(cmd, args);
}

// Safe wrapper: returns `fallback` if not in Tauri or if the call throws.
async function safe<T>(cmd: string, fallback: T, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri()) return fallback;
  try {
    return await tauriInvoke<T>(cmd, args);
  } catch (e) {
    console.warn(`${cmd} failed:`, e);
    return fallback;
  }
}

// ─── Empty values ────────────────────────────────────────────────────────

const EMPTY_BATTERY_STATUS: BatteryStatus = {
  percent: 0,
  capacityMwh: 0,
  fullChargeMwh: 0,
  designMwh: 0,
  voltageV: 0,
  rateW: 0,
  powerState: 'idle',
  onAc: false,
  tempC: null,
  etaMinutes: null,
  etaLabel: 'No data yet',
  chemistry: '',
  cycleCount: 0,
  wearPercent: 0,
  manufacturer: '',
  deviceName: '',
};

const EMPTY_POWER_READING: PowerReading = {
  wallInputW: null,
  systemDrawW: null,
  cpuPackageW: null,
  gpuW: null,
  dramW: null,
  source: 'No data yet',
  channels: [],
};

const EMPTY_SESSION_DETAIL: SessionDetail = {
  history: [],
  minRateW: 0,
  maxRateW: 0,
  avgRateW: 0,
  totalEnergyMwh: 0,
  durationSec: 0,
};

const EMPTY_SLEEP_DETAIL: SleepDetail = {
  history: [],
  minRateMw: 0,
  maxRateMw: 0,
  avgRateMw: 0,
  totalDrainMwh: 0,
  durationSec: 0,
};

// ─── Public API ──────────────────────────────────────────────────────────

export async function getBatteryStatus(): Promise<BatteryStatus> {
  return safe('get_battery_status', EMPTY_BATTERY_STATUS);
}

export async function getPowerReading(): Promise<PowerReading> {
  return safe('get_power_reading', EMPTY_POWER_READING);
}

export interface TopAppsResponse {
  apps: AppPowerRow[];
  confidencePercent: number;
  batteryDischargeW: number;
  systemOverheadW: number;
}

const EMPTY_TOP_APPS: TopAppsResponse = {
  apps: [],
  confidencePercent: 0,
  batteryDischargeW: 0,
  systemOverheadW: 0,
};

export async function getTopApps(): Promise<TopAppsResponse> {
  return safe<TopAppsResponse>('get_top_apps', EMPTY_TOP_APPS);
}

export async function getBatteryHistory(minutes: number): Promise<HistoryPoint[]> {
  return safe<HistoryPoint[]>('get_battery_history', [], { minutes });
}

export async function getBatterySessions(): Promise<BatterySession[]> {
  return safe<BatterySession[]>('get_battery_sessions', []);
}

export async function getSleepSessions(): Promise<SleepSession[]> {
  return safe<SleepSession[]>('get_sleep_sessions', []);
}

export async function getHealthHistory(): Promise<HealthSnapshot[]> {
  return safe<HealthSnapshot[]>('get_health_history', []);
}

export async function getSessionDetail(sessionId: number): Promise<SessionDetail> {
  return safe('get_session_detail', EMPTY_SESSION_DETAIL, { sessionId });
}

export async function getSleepDetail(_sleepId: number): Promise<SleepDetail> {
  // No backend command yet — we only store pre/post capacity for sleeps,
  // not per-tick history. Return empty until Phase 3 adds it.
  return EMPTY_SLEEP_DETAIL;
}

export async function getComponentHistory(minutes: number): Promise<ComponentHistoryPoint[]> {
  return safe<ComponentHistoryPoint[]>('get_component_history', [], { minutes });
}

// ─── Unified timeline (Sessions page) ───────────────────────────────────

export interface UnifiedTimeline {
  history: HistoryPoint[];
  batterySessions: BatterySession[];
  sleepSessions: SleepSession[];
  componentHistory: ComponentHistoryPoint[];
  appPowerSummary: AppPowerSummary[];
}

const EMPTY_UNIFIED_TIMELINE: UnifiedTimeline = {
  history: [],
  batterySessions: [],
  sleepSessions: [],
  componentHistory: [],
  appPowerSummary: [],
};

export async function getUnifiedTimeline(startTs: number, endTs: number): Promise<UnifiedTimeline> {
  return safe('get_unified_timeline', EMPTY_UNIFIED_TIMELINE, { startTs, endTs });
}

// ─── Accent color ───────────────────────────────────────────────────────
export async function getAccentColor(): Promise<string | null> {
  if (!inTauri()) return null;
  try {
    return await tauriInvoke<string>('get_accent_color');
  } catch {
    return null;
  }
}

// ─── Autostart ──────────────────────────────────────────────────────────
export async function enableAutostart(): Promise<void> {
  return safe('enable_autostart', undefined);
}

export async function disableAutostart(): Promise<void> {
  return safe('disable_autostart', undefined);
}

export async function isAutostartEnabled(): Promise<boolean> {
  return safe('is_autostart_enabled', false);
}

// ─── Start minimized ──────────────────────────────────────────────────────
export async function setStartMinimized(enabled: boolean): Promise<void> {
  return safe('set_start_minimized', undefined, { enabled });
}

export async function getStartMinimized(): Promise<boolean> {
  return safe('get_start_minimized', false);
}

// ─── Charge speed ──────────────────────────────────────────────────────

export interface ChargeSpeed {
  currentRateW: number;
  maxRateW: number;
  avgRateW: number;
  timeToFullMin: number | null;
  etaLabel: string;
  startPercent: number;
  currentPercent: number;
}

const EMPTY_CHARGE_SPEED: ChargeSpeed = {
  currentRateW: 0,
  maxRateW: 0,
  avgRateW: 0,
  timeToFullMin: null,
  etaLabel: '',
  startPercent: 0,
  currentPercent: 0,
};

export async function getChargeSpeed(): Promise<ChargeSpeed> {
  return safe('get_charge_speed', EMPTY_CHARGE_SPEED);
}

// ─── Charge habits ─────────────────────────────────────────────────────

export interface ChargeHabitMetrics {
  avgMaxCharge: number;
  overchargePct: number;
  deepDischargePct: number;
  timeAt100Minutes: number;
  chargesTo100: number;
  dischargesBelow20: number;
}

export interface ChargeHabits {
  score: number;
  verdict: string;
  hasEnoughData: boolean;
  isProvisional: boolean;
  dataDays: number;
  metrics: ChargeHabitMetrics;
  tips: string[];
}

const EMPTY_CHARGE_HABITS: ChargeHabits = {
  score: 0,
  verdict: '',
  hasEnoughData: false,
  isProvisional: true,
  dataDays: 0,
  metrics: {
    avgMaxCharge: 0,
    overchargePct: 0,
    deepDischargePct: 0,
    timeAt100Minutes: 0,
    chargesTo100: 0,
    dischargesBelow20: 0,
  },
  tips: [],
};

export async function getChargeHabits(): Promise<ChargeHabits> {
  return safe('get_charge_habits', EMPTY_CHARGE_HABITS);
}

// ─── Data retention ────────────────────────────────────────────────────

export async function setDataRetention(days: number): Promise<void> {
  return safe('set_data_retention', undefined, { days });
}

// ─── Export ────────────────────────────────────────────────────────────

export async function exportReportJson(): Promise<string> {
  return safe('export_report_json', 'error');
}

export async function exportReportPdf(): Promise<string> {
  return safe('export_report_pdf', 'error');
}

// ─── "Before I unplug" estimate ────────────────────────────────────────

export interface UnplugDrainEntry {
  name: string;
  watts: number;
  estHours: number;
}

export interface UnplugEstimate {
  totalHours: number;
  totalLabel: string;
  topDrains: UnplugDrainEntry[];
  systemOverheadW: number;
}

const EMPTY_UNPLUG: UnplugEstimate = {
  totalHours: 0,
  totalLabel: '',
  topDrains: [],
  systemOverheadW: 0,
};

export async function getUnplugEstimate(): Promise<UnplugEstimate> {
  return safe('get_unplug_estimate', EMPTY_UNPLUG);
}

// ─── Power plan auto-switching ──────────────────────────────────────────

export interface PowerPlanStatus {
  enabled: boolean;
  lowThreshold: number;
  highThreshold: number;
  activeScheme: string;
}

export async function getPowerPlanStatus(): Promise<PowerPlanStatus> {
  return safe('get_power_plan_status', {
    enabled: false,
    lowThreshold: 30,
    highThreshold: 80,
    activeScheme: 'unknown',
  });
}

export async function setPowerPlanConfig(
  enabled: boolean,
  low: number,
  high: number,
): Promise<void> {
  return safe('set_power_plan_config', undefined, { enabled, low, high });
}

// ─── Notification preferences ───────────────────────────────────────────
export interface NotificationPrefsInput {
  notifyCharge: boolean;
  chargeLimit: number;
  notifyLow: boolean;
  lowThreshold: number;
  notifySleepDrain: boolean;
  summaryEnabled: boolean;
  summaryIntervalMin: number;
  summaryOnlyOnBattery: boolean;
  summaryShowRate: boolean;
  summaryShowEta: boolean;
  summaryShowDelta: boolean;
  summaryShowTopApp: boolean;
}

export async function setNotificationPrefs(prefs: NotificationPrefsInput): Promise<void> {
  return safe('set_notification_prefs', undefined, { prefs });
}

// ─── LHM Setup ────────────────────────────────────────────────────────

export interface LhmStatus {
  needed: boolean;
  running: boolean;
  installed: boolean;
  autoStartEnabled: boolean;
  lhmDir: string;
  hasEmi: boolean;
  serviceRunning: boolean;
}

const EMPTY_LHM_STATUS: LhmStatus = {
  needed: false,
  running: false,
  installed: false,
  autoStartEnabled: false,
  lhmDir: '',
  hasEmi: false,
  serviceRunning: false,
};

export async function getLhmStatus(): Promise<LhmStatus> {
  return safe('get_lhm_status', EMPTY_LHM_STATUS);
}

export interface LhmDownloadResult {
  success: boolean;
  zipPath: string;
  error: string | null;
}

export async function lhmDownload(): Promise<LhmDownloadResult> {
  return safe('lhm_download', { success: false, zipPath: '', error: 'not available' });
}

export async function lhmFindDownload(): Promise<string | null> {
  if (!inTauri()) return null;
  try {
    return await tauriInvoke<string | null>('lhm_find_download');
  } catch {
    return null;
  }
}

export interface LhmInstallResult {
  success: boolean;
  exePath: string;
  error: string | null;
}

export async function lhmInstall(zipPath?: string): Promise<LhmInstallResult> {
  return safe('lhm_install', { success: false, exePath: '', error: 'not available' },
    zipPath ? { zipPath } : undefined);
}

export async function lhmVerify(): Promise<boolean> {
  return safe('lhm_verify', false);
}

// Convenience: returns true if we're connected to the real Rust backend.
export function isLive(): boolean {
  return inTauri();
}
