// Mock data for the frontend prototype. Produces realistic-looking values
// and walks the simulated battery state forward on each tick so you can
// leave the app running and watch numbers change.

import type {
  AppPowerRow,
  BatterySession,
  BatteryStatus,
  HealthSnapshot,
  HistoryPoint,
  PowerReading,
  SleepSession,
} from './types';

// ─── Simulated battery state ─────────────────────────────────────────────

interface SimState {
  percent: number;
  capacityMwh: number;
  voltageV: number;
  rateW: number;        // signed
  onAc: boolean;
  tempC: number;
  lastTick: number;
}

const FULL_CAPACITY = 54110;
const DESIGN_CAPACITY = 52330;

let sim: SimState = {
  percent: 82.4,
  capacityMwh: Math.round(FULL_CAPACITY * 0.824),
  voltageV: 12.54,
  rateW: -9.6, // starting on battery
  onAc: false,
  tempC: 32.1,
  lastTick: Date.now(),
};

/// Advance the simulation by `dtMs` milliseconds.
function tick(dtMs: number): void {
  const dtH = dtMs / 3_600_000;

  // Tiny randomness in the draw/charge rate so numbers move
  const jitter = (Math.random() - 0.5) * 0.8;

  if (sim.onAc) {
    // Charging at 35 W ± jitter until ~100%, then taper
    const headroom = (100 - sim.percent) / 100;
    sim.rateW = Math.max(1.5, 32 * headroom + jitter);
    sim.voltageV = 12.8 + (100 - sim.percent) / 200 + Math.random() * 0.02;
  } else {
    // Discharging at 8-14 W depending on load
    sim.rateW = -(8 + Math.random() * 6) + jitter;
    sim.voltageV = 11.3 + (sim.percent / 100) * 1.3 + Math.random() * 0.02;
  }

  const deltaMwh = sim.rateW * 1000 * dtH;
  sim.capacityMwh = Math.max(
    0,
    Math.min(FULL_CAPACITY, sim.capacityMwh + deltaMwh),
  );
  sim.percent = (sim.capacityMwh / FULL_CAPACITY) * 100;

  // Temperature tracks load
  const target = 30 + Math.abs(sim.rateW) * 0.25;
  sim.tempC += (target - sim.tempC) * 0.15 + (Math.random() - 0.5) * 0.3;
}

export function toggleMockPowerSource(): void {
  sim.onAc = !sim.onAc;
}

export function isMockOnAc(): boolean {
  return sim.onAc;
}

// ─── Public API ──────────────────────────────────────────────────────────

function getStatus(): BatteryStatus {
  const now = Date.now();
  tick(now - sim.lastTick);
  sim.lastTick = now;

  const etaMinutes = computeEta();
  const etaLabel = formatEtaLabel(etaMinutes, sim.rateW);

  return {
    percent: sim.percent,
    capacityMwh: Math.round(sim.capacityMwh),
    fullChargeMwh: FULL_CAPACITY,
    designMwh: DESIGN_CAPACITY,
    voltageV: sim.voltageV,
    rateW: sim.rateW,
    powerState: powerStateLabel(),
    onAc: sim.onAc,
    tempC: sim.tempC,
    etaMinutes,
    etaLabel,
    chemistry: 'LION',
    cycleCount: 53,
    wearPercent: 0,
    manufacturer: 'SWD',
    deviceName: 'SurfaceBattery',
  };
}

function computeEta(): number | null {
  if (Math.abs(sim.rateW) < 0.5) return null;
  if (sim.onAc) {
    const toFull = FULL_CAPACITY - sim.capacityMwh;
    if (toFull <= 100) return null;
    return Math.round((toFull / 1000 / sim.rateW) * 60);
  }
  return Math.round((sim.capacityMwh / 1000 / -sim.rateW) * 60);
}

function formatEtaLabel(etaMinutes: number | null, rate: number): string {
  if (etaMinutes === null) {
    if (sim.onAc && sim.percent > 99) return 'Fully charged';
    if (Math.abs(rate) < 0.5) return 'Rate not reported';
    return 'Calculating…';
  }
  const h = Math.floor(etaMinutes / 60);
  const m = etaMinutes % 60;
  const dur = h > 0 ? `${h}h ${m}m` : `${m} min`;
  if (sim.onAc) {
    return `about ${dur} to full at this rate`;
  }
  return `about ${dur} left at this rate`;
}

function powerStateLabel(): BatteryStatus['powerState'] {
  if (sim.percent >= 99.5 && sim.onAc) return 'full';
  if (sim.onAc) return 'charging';
  if (sim.percent < 10) return 'critical';
  return 'discharging';
}

function getPower(): PowerReading {
  // Simulate EMI-style channels. Total CPU/GPU wattage tracks the battery
  // load so the dashboard feels consistent.
  const loadFactor = Math.abs(sim.rateW) / 12;
  const cpu = 2.5 + Math.random() * 6 * loadFactor;
  const gpu = 0.02 + Math.random() * 1.8 * loadFactor;
  const system = cpu + gpu + 4 + Math.random() * 2;
  const wallInput = sim.onAc ? system + Math.abs(sim.rateW) + Math.random() * 1 : 0;

  return {
    wallInputW: sim.onAc ? wallInput : null,
    systemDrawW: system,
    cpuPackageW: cpu,
    gpuW: gpu,
    dramW: 0.1 + Math.random() * 0.2,
    source: 'EMI v2 — Qualcomm 8380 (mock)',
    channels: [],
  };
}

function getApps(): AppPowerRow[] {
  // Generate a plausible top-of-process-list with some persistent names
  // that jitter per call.
  const names = [
    'chrome.exe',
    'Code.exe',
    'Discord.exe',
    'explorer.exe',
    'dwm.exe',
    'steamwebhelper.exe',
    'Spotify.exe',
    'nextcloud.exe',
    'NordVPN.exe',
    'MsMpEng.exe',
    'FluentFlyout.exe',
    'Taskmgr.exe',
  ];
  const scale = Math.max(1, Math.abs(sim.rateW) * 0.6);
  return names
    .map((name, i) => {
      const cpuW = (1 / (i + 1)) * scale * (0.6 + Math.random() * 0.8);
      const gpuW = i < 4 ? Math.random() * 0.6 : 0;
      return {
        pid: 1000 + i * 137,
        name,
        cpuW,
        gpuW,
        diskW: 0,
        netW: 0,
        totalW: cpuW + gpuW,
      };
    })
    .sort((a, b) => b.totalW - a.totalW);
}

function getHistory(minutes: number): HistoryPoint[] {
  // Produce a sawtooth over the past `minutes` showing discharge,
  // recharge, discharge. Uses current sim percent as the final point.
  const points: HistoryPoint[] = [];
  const nPoints = Math.min(minutes * 2, 200);
  const stepSeconds = (minutes * 60) / nPoints;
  const now = Math.floor(Date.now() / 1000);

  let pct = Math.max(5, Math.min(100, sim.percent + 15));
  let direction = -1;

  for (let i = nPoints - 1; i >= 0; i--) {
    const ts = now - Math.round((nPoints - 1 - i) * stepSeconds);
    const rate = direction < 0 ? -9 - Math.random() * 3 : 30 + Math.random() * 5;
    points.push({
      ts,
      percent: Math.max(0, Math.min(100, pct)),
      rateW: rate,
    });
    const dMinPct = direction * (Math.random() * 0.7 + 0.3);
    pct += dMinPct;
    if (pct <= 20 && direction < 0) direction = 1;
    if (pct >= 95 && direction > 0) direction = -1;
  }

  // Make the last point match current sim
  if (points.length > 0) {
    points[points.length - 1] = {
      ts: now,
      percent: sim.percent,
      rateW: sim.rateW,
    };
  }
  return points;
}

function getRecentSessions(): BatterySession[] {
  const now = Math.floor(Date.now() / 1000);
  return [
    {
      id: 6,
      startedAt: now - 3600 * 2,
      endedAt: null,
      startPercent: 98,
      endPercent: null,
      startCapacity: 53028,
      endCapacity: null,
      avgDrainW: -10.2,
      onAc: sim.onAc,
    },
    {
      id: 5,
      startedAt: now - 3600 * 8,
      endedAt: now - 3600 * 2,
      startPercent: 42,
      endPercent: 98,
      startCapacity: 22722,
      endCapacity: 53028,
      avgDrainW: 28.1,
      onAc: true,
    },
    {
      id: 4,
      startedAt: now - 3600 * 14,
      endedAt: now - 3600 * 8,
      startPercent: 100,
      endPercent: 42,
      startCapacity: 54110,
      endCapacity: 22722,
      avgDrainW: -11.8,
      onAc: false,
    },
  ];
}

function getRecentSleeps(): SleepSession[] {
  const now = Math.floor(Date.now() / 1000);
  return [
    {
      id: 3,
      sleepAt: now - 3600 * 10,
      wakeAt: now - 3600 * 8,
      preCapacity: 45400,
      postCapacity: 45370,
      drainMwh: 30,
      drainPercent: 0.1,
      drainRateMw: 161,
      dripsPercent: 96.4,
      verdict: 'normal',
    },
    {
      id: 2,
      sleepAt: now - 3600 * 28,
      wakeAt: now - 3600 * 20,
      preCapacity: 48200,
      postCapacity: 45980,
      drainMwh: 2220,
      drainPercent: 4.6,
      drainRateMw: 278,
      dripsPercent: 89.1,
      verdict: 'normal',
    },
  ];
}

function getHealthHistory(): HealthSnapshot[] {
  const now = Math.floor(Date.now() / 1000);
  const out: HealthSnapshot[] = [];
  for (let month = 11; month >= 0; month--) {
    const ts = now - month * 30 * 86400;
    // Realistic wear curve — starts flat, accelerates slightly over time.
    const monthsUsed = 11 - month;
    const wear = Math.pow(monthsUsed / 11, 1.3) * 6.2;
    out.push({
      ts,
      designCapacity: DESIGN_CAPACITY,
      fullChargeCapacity: Math.round(DESIGN_CAPACITY * (1 - wear / 100)),
      cycleCount: 12 + monthsUsed * 4,
      wearPercent: wear,
    });
  }
  return out;
}

// ─── Component power history (stacked area) ──────────────────────────────

export interface ComponentHistoryPoint {
  ts: number;
  cpu: number;
  gpu: number;
  dram: number;
  other: number;
}

function getComponentHistory(minutes: number): ComponentHistoryPoint[] {
  const out: ComponentHistoryPoint[] = [];
  const nPoints = Math.min(minutes * 2, 120);
  const stepSeconds = (minutes * 60) / nPoints;
  const now = Math.floor(Date.now() / 1000);

  for (let i = nPoints - 1; i >= 0; i--) {
    const ts = now - Math.round((nPoints - 1 - i) * stepSeconds);
    // Simulate bursty CPU work with GPU idle most of the time
    const phase = i / nPoints;
    const cpu = 2 + Math.sin(phase * 8) * 2 + Math.random() * 4;
    const gpu = Math.max(0, Math.sin(phase * 3) * 1.5 + Math.random() * 0.5);
    const dram = 0.2 + Math.random() * 0.2;
    const other = 3 + Math.random() * 2;
    out.push({ ts, cpu, gpu, dram, other });
  }
  return out;
}

// ─── Full app power list (more detail than Dashboard) ──────────────────

export interface AppRowExtended {
  pid: number;
  name: string;
  cpuW: number;
  gpuW: number;
  diskW: number;
  netW: number;
  totalW: number;
  cpuPct: number;       // 0..100 of total system CPU busy
  gpuPct: number;       // 0..100 of GPU engine time
  iconHint: string;     // first letter for a placeholder badge
  hog: boolean;         // over 3 W
}

function getAllApps(): AppRowExtended[] {
  const seed = [
    { name: 'chrome.exe', heavy: true },
    { name: 'Code.exe', heavy: true },
    { name: 'Discord.exe', heavy: false },
    { name: 'explorer.exe', heavy: false },
    { name: 'dwm.exe', heavy: false },
    { name: 'steamwebhelper.exe', heavy: false },
    { name: 'Spotify.exe', heavy: false },
    { name: 'nextcloud.exe', heavy: false },
    { name: 'NordVPN.exe', heavy: false },
    { name: 'MsMpEng.exe', heavy: true },
    { name: 'FluentFlyout.exe', heavy: false },
    { name: 'Taskmgr.exe', heavy: false },
    { name: 'svchost.exe', heavy: false },
    { name: 'RuntimeBroker.exe', heavy: false },
    { name: 'SearchIndexer.exe', heavy: false },
    { name: 'OneDrive.exe', heavy: false },
    { name: 'HWiNFO_ARM64.EXE', heavy: false },
    { name: 'WmiPrvSE.exe', heavy: false },
  ];
  const scale = Math.max(1, Math.abs(sim.rateW) * 0.6);
  return seed
    .map((s, i) => {
      const heavyBoost = s.heavy ? 1.8 : 1;
      const cpuW =
        (1 / (i * 0.6 + 1)) * scale * heavyBoost * (0.5 + Math.random() * 0.7);
      const gpuW = i < 4 ? Math.random() * 0.8 * heavyBoost : 0;
      const diskW = Math.random() * 0.15;
      const netW = i < 6 ? Math.random() * 0.12 : 0;
      const totalW = cpuW + gpuW + diskW + netW;
      return {
        pid: 1024 + i * 137 + Math.floor(Math.random() * 20),
        name: s.name,
        cpuW,
        gpuW,
        diskW,
        netW,
        totalW,
        cpuPct: (cpuW / scale) * 100,
        gpuPct: gpuW * 25,
        iconHint: s.name.charAt(0).toUpperCase(),
        hog: totalW > 3,
      };
    })
    .sort((a, b) => b.totalW - a.totalW);
}

// ─── Extended sessions (enough for the Sessions page) ────────────────────

function getAllBatterySessions(): BatterySession[] {
  const now = Math.floor(Date.now() / 1000);
  const sessions: BatterySession[] = [];
  let cursor = now;
  let id = 20;
  const hours = [2, 6, 14, 3, 10, 20, 5, 4, 18, 8];
  let onAc = sim.onAc;
  for (const h of hours) {
    const start = cursor - h * 3600;
    const startPct = 30 + Math.random() * 60;
    const drain = onAc ? 25 + Math.random() * 10 : -(8 + Math.random() * 6);
    const endPct = Math.max(5, Math.min(100, startPct + (drain * h) / 10));
    sessions.push({
      id: id--,
      startedAt: start,
      endedAt: id === 19 ? null : cursor, // first one is open
      startPercent: startPct,
      endPercent: id === 19 ? null : endPct,
      startCapacity: Math.round((startPct / 100) * FULL_CAPACITY),
      endCapacity: id === 19 ? null : Math.round((endPct / 100) * FULL_CAPACITY),
      avgDrainW: drain,
      onAc,
    });
    cursor = start;
    onAc = !onAc;
  }
  return sessions;
}

function getAllSleepSessions(): SleepSession[] {
  const now = Math.floor(Date.now() / 1000);
  const entries = [
    { hoursAgo: 10, durHours: 2, rateMw: 161, verdict: 'normal' as const },
    { hoursAgo: 28, durHours: 8, rateMw: 278, verdict: 'normal' as const },
    { hoursAgo: 50, durHours: 9, rateMw: 142, verdict: 'normal' as const },
    { hoursAgo: 78, durHours: 8, rateMw: 512, verdict: 'high' as const },
    { hoursAgo: 100, durHours: 7, rateMw: 87, verdict: 'excellent' as const },
    { hoursAgo: 128, durHours: 9, rateMw: 195, verdict: 'normal' as const },
    { hoursAgo: 152, durHours: 2, rateMw: 834, verdict: 'very-high' as const },
  ];
  return entries.map((e, i) => {
    const sleepAt = now - e.hoursAgo * 3600;
    const wakeAt = sleepAt + e.durHours * 3600;
    const drainMwh = Math.round(e.rateMw * e.durHours);
    const preCap = 45000 - i * 300;
    return {
      id: 20 - i,
      sleepAt,
      wakeAt,
      preCapacity: preCap,
      postCapacity: preCap - drainMwh,
      drainMwh,
      drainPercent: (drainMwh / preCap) * 100,
      drainRateMw: e.rateMw,
      dripsPercent: 85 + Math.random() * 12,
      verdict: e.verdict,
    };
  });
}

// ─── Per-session detail (history + stats for the drill-down view) ──────

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

/// Deterministic pseudo-random based on a seed so the same session
/// produces the same shape every time it's opened.
function seededRandom(seed: number): () => number {
  let s = seed;
  return () => {
    s = (s * 1664525 + 1013904223) % 0x100000000;
    return s / 0x100000000;
  };
}

function getBatterySessionDetail(session: BatterySession): SessionDetail {
  const start = session.startedAt;
  const end = session.endedAt ?? Math.floor(Date.now() / 1000);
  const durationSec = Math.max(60, end - start);
  const nPoints = 80;
  const step = durationSec / nPoints;
  const rng = seededRandom(session.id * 7919 + 1);

  const history: SessionDetailPoint[] = [];
  let pct = session.startPercent;
  let minR = Infinity;
  let maxR = -Infinity;
  let sumR = 0;

  // Walk forward, generating bursty load. The base rate is the session
  // average; we add multi-frequency noise so the chart looks like real
  // workload (idle stretches + bursts of activity).
  const baseRate = session.avgDrainW ?? -10;
  for (let i = 0; i <= nPoints; i++) {
    const ts = start + Math.round(i * step);
    const phase = i / nPoints;
    const burst = Math.sin(phase * 14) * 4 + Math.sin(phase * 27) * 2;
    const noise = (rng() - 0.5) * 4;
    let rateW = baseRate + burst + noise;
    // Don't allow charging during a battery session or vice versa
    if (session.onAc && rateW < 0) rateW = Math.abs(rateW) * 0.3;
    if (!session.onAc && rateW > 0) rateW = -Math.abs(rateW) * 0.3;

    // Update simulated percent
    const dPct = ((rateW * step) / 3600 / FULL_CAPACITY) * 1000 * 100;
    pct = Math.max(0, Math.min(100, pct + dPct));

    history.push({ ts, percent: pct, rateW });
    if (rateW < minR) minR = rateW;
    if (rateW > maxR) maxR = rateW;
    sumR += rateW;
  }

  // Force the last point to match the recorded end percent if known.
  if (session.endPercent !== null && history.length > 0) {
    history[history.length - 1].percent = session.endPercent;
  }

  const avgRateW = sumR / history.length;
  const totalEnergyMwh = Math.abs(avgRateW * (durationSec / 3600) * 1000);

  return {
    history,
    minRateW: minR,
    maxRateW: maxR,
    avgRateW,
    totalEnergyMwh,
    durationSec,
  };
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

function getSleepSessionDetail(sleep: SleepSession): SleepDetail {
  if (sleep.wakeAt === null || sleep.preCapacity === 0) {
    return {
      history: [],
      minRateMw: 0,
      maxRateMw: 0,
      avgRateMw: 0,
      totalDrainMwh: 0,
      durationSec: 0,
    };
  }
  const start = sleep.sleepAt;
  const end = sleep.wakeAt;
  const durationSec = end - start;
  const nPoints = 60;
  const step = durationSec / nPoints;
  const rng = seededRandom(sleep.id * 31 + 5);
  const baseRateMw = sleep.drainRateMw ?? 200;

  const history: SleepDetailPoint[] = [];
  let cap = sleep.preCapacity;
  let minR = Infinity;
  let maxR = -Infinity;
  let sumR = 0;

  for (let i = 0; i <= nPoints; i++) {
    const ts = start + Math.round(i * step);
    // Modern Standby = mostly steady drain with brief background-wake spikes
    const spike = rng() < 0.08 ? rng() * baseRateMw * 1.6 : 0;
    const jitter = (rng() - 0.5) * baseRateMw * 0.3;
    const rate = Math.max(0, baseRateMw + spike + jitter);

    cap = Math.max(0, cap - (rate * step) / 3600);
    history.push({ ts, capacity: cap, drainRateMw: rate });
    if (rate < minR) minR = rate;
    if (rate > maxR) maxR = rate;
    sumR += rate;
  }

  const avgRateMw = sumR / history.length;
  const totalDrainMwh = sleep.drainMwh ?? Math.round((avgRateMw * durationSec) / 3600);
  return {
    history,
    minRateMw: minR,
    maxRateMw: maxR,
    avgRateMw,
    totalDrainMwh,
    durationSec,
  };
}

// ─── Recent activity for the periodic notification ─────────────────────

export interface RecentSummary {
  startedAt: number;
  endedAt: number;
  startPercent: number;
  endPercent: number;
  netEnergyMwh: number;     // positive = gained, negative = drained
  avgRateW: number;
  topApp: string;
  onAc: boolean;
}

function getRecentSummary(minutes: number): RecentSummary {
  const now = Math.floor(Date.now() / 1000);
  const start = now - minutes * 60;
  const status = getStatus();

  // Approximate where the battery was `minutes` ago. The mock sim moves
  // by ~0.18%/min on AC and ~0.13%/min on battery, so we use the current
  // power state's rate as a stand-in. In production this comes from the
  // SQLite reading at (now - minutes).
  const dPct = sim.onAc ? minutes * 0.18 : -minutes * 0.13;
  const startPct = Math.max(0, Math.min(100, status.percent - dPct));
  const endPct = status.percent;

  // True average wattage = energy delta / time delta. Sign convention:
  // positive = charging (gained energy), negative = discharging (lost).
  const energyDeltaMwh =
    ((endPct - startPct) / 100) * status.fullChargeMwh;
  const timeDeltaH = minutes / 60;
  const avgRateW = energyDeltaMwh / timeDeltaH / 1000;

  const apps = getApps();
  return {
    startedAt: start,
    endedAt: now,
    startPercent: startPct,
    endPercent: endPct,
    netEnergyMwh: Math.round(energyDeltaMwh),
    avgRateW,
    topApp: apps[0]?.name ?? 'unknown',
    onAc: sim.onAc,
  };
}

export const mock = {
  getStatus,
  getPower,
  getApps,
  getAllApps,
  getHistory,
  getComponentHistory,
  getRecentSessions,
  getAllBatterySessions,
  getBatterySessionDetail,
  getRecentSleeps,
  getAllSleepSessions,
  getSleepSessionDetail,
  getHealthHistory,
  getRecentSummary,
};
