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
  other: number;
}

export interface AppPowerSummary {
  name: string;
  avgWatts: number;
  maxWatts: number;
  sampleCount: number;
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
}

const EMPTY_TOP_APPS: TopAppsResponse = {
  apps: [],
  confidencePercent: 0,
  batteryDischargeW: 0,
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

// Convenience: returns true if we're connected to the real Rust backend.
export function isLive(): boolean {
  return inTauri();
}
