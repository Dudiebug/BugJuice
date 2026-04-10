// Single source of truth for getting data into the React UI.
//
// When running inside Tauri, this calls real Rust backend commands via
// `invoke()`. When running in a plain browser (vite preview / vite dev /
// the GitHub Pages preview), it transparently falls back to mock data so
// the UI still works for design iteration.
//
// Every page should import from `@/api` instead of `@/mock` directly.

import type {
  AppPowerRow,
  BatterySession,
  BatteryStatus,
  HealthSnapshot,
  HistoryPoint,
  PowerReading,
  SleepSession,
} from './types';
import { mock, type ComponentHistoryPoint, type SessionDetail, type SleepDetail } from './mock';

// Detect Tauri at runtime. Tauri injects this global into the WebView.
function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

// Lazily import @tauri-apps/api so the browser bundle still works when
// the package's globals aren't present.
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(cmd, args);
}

// ─── Public API ──────────────────────────────────────────────────────────────

export async function getBatteryStatus(): Promise<BatteryStatus> {
  if (inTauri()) {
    try {
      return await tauriInvoke<BatteryStatus>('get_battery_status');
    } catch (e) {
      console.warn('get_battery_status failed, falling back to mock:', e);
    }
  }
  return mock.getStatus();
}

export async function getPowerReading(): Promise<PowerReading> {
  if (inTauri()) {
    try {
      return await tauriInvoke<PowerReading>('get_power_reading');
    } catch (e) {
      console.warn('get_power_reading failed, falling back to mock:', e);
    }
  }
  return mock.getPower();
}

export async function getTopApps(): Promise<AppPowerRow[]> {
  if (inTauri()) {
    try {
      const rows = await tauriInvoke<AppPowerRow[]>('get_top_apps');
      // The first ~tick after launch the table is empty — fall back to
      // current mock so the UI doesn't look dead.
      if (rows.length > 0) return rows;
    } catch (e) {
      console.warn('get_top_apps failed:', e);
    }
  }
  return mock.getApps();
}

export async function getBatteryHistory(minutes: number): Promise<HistoryPoint[]> {
  if (inTauri()) {
    try {
      const rows = await tauriInvoke<HistoryPoint[]>('get_battery_history', { minutes });
      if (rows.length > 1) return rows;
    } catch (e) {
      console.warn('get_battery_history failed:', e);
    }
  }
  return mock.getHistory(minutes);
}

export async function getBatterySessions(): Promise<BatterySession[]> {
  if (inTauri()) {
    try {
      const rows = await tauriInvoke<BatterySession[]>('get_battery_sessions');
      if (rows.length > 0) return rows;
    } catch (e) {
      console.warn('get_battery_sessions failed:', e);
    }
  }
  return mock.getAllBatterySessions();
}

export async function getSleepSessions(): Promise<SleepSession[]> {
  if (inTauri()) {
    try {
      const rows = await tauriInvoke<SleepSession[]>('get_sleep_sessions');
      if (rows.length > 0) return rows;
    } catch (e) {
      console.warn('get_sleep_sessions failed:', e);
    }
  }
  return mock.getAllSleepSessions();
}

export async function getHealthHistory(): Promise<HealthSnapshot[]> {
  if (inTauri()) {
    try {
      const rows = await tauriInvoke<HealthSnapshot[]>('get_health_history');
      if (rows.length > 0) return rows;
    } catch (e) {
      console.warn('get_health_history failed:', e);
    }
  }
  return mock.getHealthHistory();
}

export async function getSessionDetail(sessionId: number): Promise<SessionDetail> {
  if (inTauri()) {
    try {
      const detail = await tauriInvoke<SessionDetail>('get_session_detail', { sessionId });
      if (detail.history.length > 1) return detail;
    } catch (e) {
      console.warn('get_session_detail failed:', e);
    }
  }
  // Mock fallback — find the session in the mock list and generate detail.
  const sessions = mock.getAllBatterySessions();
  const session = sessions.find((s) => s.id === sessionId) ?? sessions[0];
  return mock.getBatterySessionDetail(session);
}

export async function getSleepDetail(sleepId: number): Promise<SleepDetail> {
  // Sleep detail is mock-only for now (we don't store per-sleep history
  // in the prototype DB; only pre/post capacity).
  const sleeps = mock.getAllSleepSessions();
  const sleep = sleeps.find((s) => s.id === sleepId) ?? sleeps[0];
  return mock.getSleepSessionDetail(sleep);
}

export async function getComponentHistory(minutes: number): Promise<ComponentHistoryPoint[]> {
  // Real backend has per-channel power_history but the shape is per-channel
  // arrays, not the stacked-area shape the chart wants. We'd need to bucket
  // by timestamp and align channels. For now, mock this until we add a
  // dedicated command. The single-frame Components page (top section) still
  // uses real data via getPowerReading().
  return mock.getComponentHistory(minutes);
}

// Convenience: returns true if we're connected to the real Rust backend.
// The Settings page can use this to show "demo mode" vs "live".
export function isLive(): boolean {
  return inTauri();
}
