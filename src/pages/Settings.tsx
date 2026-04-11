import { useEffect, useState } from 'react';
import { enableAutostart, disableAutostart, isAutostartEnabled, setStartMinimized, getStartMinimized, setNotificationPrefs, setDataRetention, exportReportJson, exportReportPdf, getPowerPlanStatus, setPowerPlanConfig } from '@/api';
import type { PowerPlanStatus } from '@/api';

// Static example data for the notification preview. This is a design
// preview only — the real notification in the tray uses live data.
const PREVIEW_SUMMARY = {
  avgRateW: -9.4,
  startPercent: 78,
  endPercent: 76,
  deltaPercent: -2,
  topApp: 'chrome.exe',
  topAppW: 3.8,
  onAc: false,
};

type Theme = 'system' | 'light' | 'dark';

interface Prefs {
  theme: Theme;
  pollingInterval: number;       // seconds
  notifyCharge: boolean;
  chargeLimit: number;           // percent
  notifyLow: boolean;
  lowThreshold: number;          // percent
  notifySleepDrain: boolean;
  // Periodic charge / discharge summary
  summaryEnabled: boolean;
  summaryIntervalMin: number;    // minutes between summaries
  summaryShowRate: boolean;
  summaryShowEta: boolean;
  summaryShowDelta: boolean;
  summaryShowTopApp: boolean;
  summaryOnlyOnBattery: boolean;
  // Startup
  autostart: boolean;
  startMinimized: boolean;
  dataRetentionDays: number;
}

const DEFAULTS: Prefs = {
  theme: 'system',
  pollingInterval: 5,
  notifyCharge: true,
  chargeLimit: 80,
  notifyLow: true,
  lowThreshold: 20,
  notifySleepDrain: false,
  summaryEnabled: true,
  summaryIntervalMin: 15,
  summaryShowRate: true,
  summaryShowEta: true,
  summaryShowDelta: true,
  summaryShowTopApp: true,
  summaryOnlyOnBattery: false,
  autostart: true,
  startMinimized: false,
  dataRetentionDays: 30,
};

function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem('bugjuice-prefs');
    if (!raw) return DEFAULTS;
    return { ...DEFAULTS, ...JSON.parse(raw) };
  } catch {
    return DEFAULTS;
  }
}

export function Settings() {
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);
  const [autostartReal, setAutostartReal] = useState(prefs.autostart);
  const [minimizedReal, setMinimizedReal] = useState(prefs.startMinimized);

  const [powerPlan, setPowerPlan] = useState<PowerPlanStatus>({
    enabled: false,
    lowThreshold: 30,
    highThreshold: 80,
    activeScheme: 'unknown',
  });

  // Load real autostart + start-minimized + power plan state from backend on mount
  useEffect(() => {
    isAutostartEnabled().then(setAutostartReal).catch(() => {});
    getStartMinimized().then(setMinimizedReal).catch(() => {});
    getPowerPlanStatus().then(setPowerPlan).catch(() => {});
  }, []);

  useEffect(() => {
    localStorage.setItem('bugjuice-prefs', JSON.stringify(prefs));
    // Sync notification prefs to Rust backend
    setNotificationPrefs({
      notifyCharge: prefs.notifyCharge,
      chargeLimit: prefs.chargeLimit,
      notifyLow: prefs.notifyLow,
      lowThreshold: prefs.lowThreshold,
      notifySleepDrain: prefs.notifySleepDrain,
      summaryEnabled: prefs.summaryEnabled,
      summaryIntervalMin: prefs.summaryIntervalMin,
      summaryOnlyOnBattery: prefs.summaryOnlyOnBattery,
      summaryShowRate: prefs.summaryShowRate,
      summaryShowEta: prefs.summaryShowEta,
      summaryShowDelta: prefs.summaryShowDelta,
      summaryShowTopApp: prefs.summaryShowTopApp,
    }).catch(() => {});
    setDataRetention(prefs.dataRetentionDays).catch(() => {});
    const root = document.documentElement;
    if (prefs.theme === 'system') {
      root.removeAttribute('data-theme');
      localStorage.removeItem('bugjuice-theme');
    } else {
      root.setAttribute('data-theme', prefs.theme);
      // Persist for the early-load guard in main.tsx (reads 'bugjuice-theme'
      // before React boots to avoid a flash of the wrong scheme).
      localStorage.setItem('bugjuice-theme', prefs.theme);
    }
  }, [prefs]);

  const update = <K extends keyof Prefs>(key: K, value: Prefs[K]) =>
    setPrefs((p) => ({ ...p, [key]: value }));

  return (
    <div className="page">
      <header className="page-header">
        <h1 className="page-title">Settings</h1>
        <p className="page-subtitle">
          Appearance, monitoring, notifications, and startup
        </p>
      </header>

      {/* ─── Appearance ─────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Appearance</div>
            <div className="card-subtitle">
              Theme follows your Windows setting by default
            </div>
          </div>
        </div>
        <SettingRow
          label="Theme"
          help="Automatically match Windows light/dark mode, or override"
        >
          <SegmentedControl
            value={prefs.theme}
            options={[
              { value: 'system', label: 'System' },
              { value: 'light', label: 'Light' },
              { value: 'dark', label: 'Dark' },
            ]}
            onChange={(v) => update('theme', v as Theme)}
          />
        </SettingRow>
      </section>

      {/* ─── Monitoring ─────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Monitoring</div>
            <div className="card-subtitle">
              How often BugJuice polls sensors
            </div>
          </div>
        </div>
        <SettingRow
          label="Polling interval"
          help="Lower is more accurate but uses slightly more battery. BugJuice slows down automatically on battery."
        >
          <Slider
            min={2}
            max={30}
            step={1}
            value={prefs.pollingInterval}
            unit="s"
            onChange={(v) => update('pollingInterval', v)}
          />
        </SettingRow>
        <SettingRow
          label="Data retention"
          help="How long to keep raw sensor readings. Hourly and daily summaries are kept much longer."
        >
          <Slider
            min={7}
            max={90}
            step={1}
            value={prefs.dataRetentionDays}
            unit=" days"
            onChange={(v) => update('dataRetentionDays', v)}
          />
        </SettingRow>
        <SettingRow
          label="Export data"
          help="Save a full report (battery status, health, sessions, charge habits)."
        >
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              onClick={async () => {
                try {
                  const r = await exportReportJson();
                  if (r === 'exported') alert('JSON report exported.');
                } catch (e) { console.error('export failed:', e); }
              }}
              style={{
                padding: '8px 20px',
                background: 'var(--accent)',
                color: '#fff',
                border: 'none',
                borderRadius: 'var(--radius-sm)',
                fontWeight: 600,
                fontSize: 13,
                cursor: 'pointer',
              }}
            >
              Export JSON
            </button>
            <button
              onClick={async () => {
                try {
                  const r = await exportReportPdf();
                  if (r === 'exported') alert('PDF report exported.');
                } catch (e) { console.error('export failed:', e); }
              }}
              style={{
                padding: '8px 20px',
                background: 'var(--bg-inset)',
                color: 'var(--text)',
                border: '1px solid var(--border)',
                borderRadius: 'var(--radius-sm)',
                fontWeight: 600,
                fontSize: 13,
                cursor: 'pointer',
              }}
            >
              Export PDF
            </button>
          </div>
        </SettingRow>
      </section>

      {/* ─── Power plan ──────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Power plan</div>
            <div className="card-subtitle">
              Automatically switch Windows power plan based on battery level
              {powerPlan.activeScheme !== 'unknown' && (
                <> · current: <strong>{powerPlan.activeScheme}</strong></>
              )}
            </div>
          </div>
        </div>
        <SettingRow
          label="Auto-switch power plan"
          help="Switch to Power Saver at low battery, Balanced when charged"
        >
          <label className="toggle">
            <input
              type="checkbox"
              checked={powerPlan.enabled}
              onChange={(e) => {
                const next = { ...powerPlan, enabled: e.target.checked };
                setPowerPlan(next);
                setPowerPlanConfig(next.enabled, next.lowThreshold, next.highThreshold).catch(() => {});
              }}
            />
            <span className="toggle-track" />
          </label>
        </SettingRow>
        {powerPlan.enabled && (
          <>
            <SettingRow
              label="Power Saver below"
              help="Switch to Power Saver when battery drops below this level"
            >
              <Slider
                min={5}
                max={50}
                step={5}
                value={powerPlan.lowThreshold}
                unit="%"
                onChange={(v) => {
                  const next = { ...powerPlan, lowThreshold: v };
                  setPowerPlan(next);
                  setPowerPlanConfig(next.enabled, next.lowThreshold, next.highThreshold).catch(() => {});
                }}
              />
            </SettingRow>
            <SettingRow
              label="Balanced above"
              help="Switch back to Balanced when battery rises above this level"
            >
              <Slider
                min={powerPlan.lowThreshold + 10}
                max={100}
                step={5}
                value={powerPlan.highThreshold}
                unit="%"
                onChange={(v) => {
                  const next = { ...powerPlan, highThreshold: v };
                  setPowerPlan(next);
                  setPowerPlanConfig(next.enabled, next.lowThreshold, next.highThreshold).catch(() => {});
                }}
              />
            </SettingRow>
          </>
        )}
      </section>

      {/* ─── Notifications ──────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Notifications</div>
            <div className="card-subtitle">
              What BugJuice will alert you about
            </div>
          </div>
        </div>

        <SettingRow
          label="Charge-limit reminder"
          help="Alert when battery passes a percentage to protect lifespan"
        >
          <Toggle
            checked={prefs.notifyCharge}
            onChange={(v) => update('notifyCharge', v)}
          />
        </SettingRow>
        {prefs.notifyCharge && (
          <SettingRow label="Charge limit" indented>
            <Slider
              min={50}
              max={100}
              step={5}
              value={prefs.chargeLimit}
              unit="%"
              onChange={(v) => update('chargeLimit', v)}
            />
          </SettingRow>
        )}

        <SettingRow
          label="Low battery warning"
          help="Alert when battery drops below a threshold"
        >
          <Toggle
            checked={prefs.notifyLow}
            onChange={(v) => update('notifyLow', v)}
          />
        </SettingRow>
        {prefs.notifyLow && (
          <SettingRow label="Low threshold" indented>
            <Slider
              min={5}
              max={40}
              step={1}
              value={prefs.lowThreshold}
              unit="%"
              onChange={(v) => update('lowThreshold', v)}
            />
          </SettingRow>
        )}

        <SettingRow
          label="Abnormal sleep drain"
          help="Alert when the battery loses more than 200 mW average during sleep"
        >
          <Toggle
            checked={prefs.notifySleepDrain}
            onChange={(v) => update('notifySleepDrain', v)}
          />
        </SettingRow>
      </section>

      {/* ─── Periodic charge summary ────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Periodic charge summary</div>
            <div className="card-subtitle">
              Get a regular toast notification with the latest battery activity
            </div>
          </div>
          {prefs.summaryEnabled && (
            <span className="badge badge-ok">
              every {prefs.summaryIntervalMin} min
            </span>
          )}
        </div>

        <SettingRow
          label="Enable summary notifications"
          help="A small Windows toast appears at the chosen interval with current rate, ETA, and top app"
        >
          <Toggle
            checked={prefs.summaryEnabled}
            onChange={(v) => update('summaryEnabled', v)}
          />
        </SettingRow>

        {prefs.summaryEnabled && (
          <>
            <SettingRow label="Interval" indented>
              <SegmentedControl
                value={prefs.summaryIntervalMin.toString()}
                options={[
                  { value: '5', label: '5 min' },
                  { value: '10', label: '10 min' },
                  { value: '15', label: '15 min' },
                  { value: '30', label: '30 min' },
                  { value: '60', label: '1 hr' },
                ]}
                onChange={(v) => update('summaryIntervalMin', Number(v))}
              />
            </SettingRow>

            <SettingRow
              label="Only when on battery"
              help="Skip the summary while plugged in"
              indented
            >
              <Toggle
                checked={prefs.summaryOnlyOnBattery}
                onChange={(v) => update('summaryOnlyOnBattery', v)}
              />
            </SettingRow>

            <SettingRow
              label="What to include"
              help="Pick which lines appear in the toast"
              indented
            >
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <CheckboxRow
                  checked={prefs.summaryShowRate}
                  onChange={(v) => update('summaryShowRate', v)}
                  label="Current charge / discharge rate"
                />
                <CheckboxRow
                  checked={prefs.summaryShowDelta}
                  onChange={(v) => update('summaryShowDelta', v)}
                  label="Net change since last summary"
                />
                <CheckboxRow
                  checked={prefs.summaryShowEta}
                  onChange={(v) => update('summaryShowEta', v)}
                  label="Estimated time to full / empty"
                />
                <CheckboxRow
                  checked={prefs.summaryShowTopApp}
                  onChange={(v) => update('summaryShowTopApp', v)}
                  label="Biggest power consumer"
                />
              </div>
            </SettingRow>

            <div
              style={{
                marginTop: 16,
                paddingTop: 16,
                borderTop: '1px solid var(--border)',
              }}
            >
              <div
                className="stat-label"
                style={{ marginBottom: 12 }}
              >
                Preview
              </div>
              <NotificationPreview prefs={prefs} />
            </div>
          </>
        )}
      </section>

      {/* ─── Startup ────────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Startup</div>
            <div className="card-subtitle">
              What BugJuice does when Windows starts
            </div>
          </div>
        </div>
        <SettingRow
          label="Start with Windows"
          help="Launch BugJuice automatically at login"
        >
          <Toggle
            checked={autostartReal}
            onChange={async (v: boolean) => {
              update('autostart', v);
              try {
                if (v) await enableAutostart();
                else await disableAutostart();
                setAutostartReal(v);
              } catch (e) {
                console.error('autostart toggle failed:', e);
              }
            }}
          />
        </SettingRow>
        <SettingRow
          label="Start minimized to tray"
          help="Hide the window on startup; click the tray icon to open"
        >
          <Toggle
            checked={minimizedReal}
            onChange={async (v: boolean) => {
              update('startMinimized', v);
              try {
                await setStartMinimized(v);
                setMinimizedReal(v);
              } catch (e) {
                console.error('start-minimized toggle failed:', e);
              }
            }}
          />
        </SettingRow>
      </section>

      {/* ─── About ──────────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-title">About</div>
        <div
          style={{
            marginTop: 12,
            fontSize: 14,
            color: 'var(--text-subtle)',
            lineHeight: 1.6,
          }}
        >
          <div>
            <strong style={{ color: 'var(--text)' }}>BugJuice</strong> by
            DudieBug — v1.0.0
          </div>
          <div style={{ marginTop: 6 }}>
            Open source battery monitoring for Windows. Built with Tauri,
            Rust, and React.
          </div>
          <div style={{ marginTop: 12, display: 'flex', gap: 20 }}>
            <a
              href="https://github.com/Dudiebug/BugJuice"
              style={{ color: 'var(--accent)' }}
              target="_blank"
              rel="noreferrer"
            >
              GitHub →
            </a>
            <a
              href="https://dudiebug.net/bugjuice"
              style={{ color: 'var(--accent)' }}
              target="_blank"
              rel="noreferrer"
            >
              Website →
            </a>
          </div>
        </div>
      </section>
    </div>
  );
}

// ─── Sub-components ────────────────────────────────────────────────

function SettingRow({
  label,
  help,
  children,
  indented,
}: {
  label: string;
  help?: string;
  children: React.ReactNode;
  indented?: boolean;
}) {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '14px 0',
        paddingLeft: indented ? 32 : 0,
        borderTop: '1px solid var(--border)',
        gap: 24,
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 14, fontWeight: 500, color: 'var(--text)' }}>
          {label}
        </div>
        {help && (
          <div
            style={{
              fontSize: 12,
              color: 'var(--text-muted)',
              marginTop: 2,
              maxWidth: 520,
            }}
          >
            {help}
          </div>
        )}
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
  );
}

function Toggle({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      onClick={() => onChange(!checked)}
      role="switch"
      aria-checked={checked}
      style={{
        width: 42,
        height: 22,
        borderRadius: 11,
        background: checked ? 'var(--accent)' : 'var(--bg-inset)',
        border: '1px solid var(--border)',
        position: 'relative',
        transition: 'background var(--dur-fast) var(--ease-out)',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: checked ? 22 : 2,
          width: 16,
          height: 16,
          borderRadius: '50%',
          background: checked ? 'var(--accent-text)' : 'var(--text-muted)',
          transition: 'left var(--dur-fast) var(--ease-out)',
        }}
      />
    </button>
  );
}

function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div
      style={{
        display: 'inline-flex',
        padding: 2,
        background: 'var(--bg-inset)',
        borderRadius: 'var(--radius-sm)',
        border: '1px solid var(--border)',
      }}
    >
      {options.map((opt) => (
        <button
          key={opt.value}
          onClick={() => onChange(opt.value)}
          style={{
            padding: '6px 16px',
            borderRadius: 4,
            background: value === opt.value ? 'var(--bg-card)' : 'transparent',
            color: value === opt.value ? 'var(--text)' : 'var(--text-subtle)',
            fontSize: 13,
            fontWeight: value === opt.value ? 600 : 400,
            transition: 'all var(--dur-fast) var(--ease-out)',
            boxShadow: value === opt.value ? 'var(--shadow-sm)' : 'none',
          }}
        >
          {opt.label}
        </button>
      ))}
    </div>
  );
}

function CheckboxRow({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
}) {
  return (
    <label
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        cursor: 'pointer',
        userSelect: 'none',
      }}
    >
      <span
        onClick={() => onChange(!checked)}
        style={{
          width: 18,
          height: 18,
          borderRadius: 4,
          border: '2px solid ' + (checked ? 'var(--accent)' : 'var(--border-strong)'),
          background: checked ? 'var(--accent)' : 'transparent',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          color: 'var(--accent-text)',
          fontSize: 12,
          flexShrink: 0,
          transition: 'all var(--dur-fast) var(--ease-out)',
        }}
      >
        {checked && '✓'}
      </span>
      <span
        onClick={() => onChange(!checked)}
        style={{ fontSize: 13, color: 'var(--text-subtle)' }}
      >
        {label}
      </span>
    </label>
  );
}

function NotificationPreview({ prefs }: { prefs: Prefs }) {
  void prefs.summaryIntervalMin;
  const summary = PREVIEW_SUMMARY;
  const charging = summary.avgRateW > 0;
  const rateAbs = Math.abs(summary.avgRateW);

  // Approx ETA in minutes
  const etaMin = (() => {
    if (!prefs.summaryShowEta) return null;
    if (Math.abs(summary.avgRateW) < 0.5) return null;
    if (charging) {
      return Math.round(((100 - summary.endPercent) / 100) * 60 * 6);
    }
    return Math.round((summary.endPercent / 100) * 60 * 6);
  })();
  const etaLabel = etaMin === null ? null : etaMin >= 60 ? `${Math.floor(etaMin / 60)}h ${etaMin % 60}m` : `${etaMin} min`;

  const skip = prefs.summaryOnlyOnBattery && summary.onAc;

  if (skip) {
    return (
      <div
        style={{
          padding: 16,
          background: 'var(--bg-inset)',
          borderRadius: 'var(--radius-sm)',
          border: '1px dashed var(--border)',
          color: 'var(--text-muted)',
          fontSize: 13,
          textAlign: 'center',
        }}
      >
        (no notification — currently on AC and "only on battery" is enabled)
      </div>
    );
  }

  const lines: { icon: string; text: string }[] = [];
  if (prefs.summaryShowRate) {
    // Show the AVERAGE wattage over the user's selected window, not the
    // instantaneous reading. This is much more useful for "what's been
    // happening" than a one-shot snapshot.
    if (rateAbs < 0.1) {
      lines.push({
        icon: '⏸',
        text: `Idle on average over the last ${prefs.summaryIntervalMin} min`,
      });
    } else {
      lines.push({
        icon: charging ? '⚡' : '🔋',
        text: charging
          ? `Avg charge: ${rateAbs.toFixed(1)} W (last ${prefs.summaryIntervalMin} min)`
          : `Avg drain: ${rateAbs.toFixed(1)} W (last ${prefs.summaryIntervalMin} min)`,
      });
    }
  }
  if (prefs.summaryShowDelta) {
    const delta = summary.endPercent - summary.startPercent;
    const sign = delta >= 0 ? '+' : '';
    lines.push({
      icon: '📊',
      text: `${sign}${delta.toFixed(1)}% in the last ${prefs.summaryIntervalMin} min`,
    });
  }
  if (prefs.summaryShowEta && etaLabel) {
    lines.push({
      icon: '⏱',
      text: charging ? `${etaLabel} until full` : `${etaLabel} remaining`,
    });
  }
  if (prefs.summaryShowTopApp) {
    lines.push({ icon: '👑', text: `Top: ${summary.topApp}` });
  }

  return (
    <div
      style={{
        maxWidth: 380,
        background: 'var(--bg-card)',
        border: '1px solid var(--border)',
        borderRadius: 8,
        padding: 16,
        boxShadow: 'var(--shadow-lg)',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          marginBottom: 10,
          paddingBottom: 10,
          borderBottom: '1px solid var(--border)',
        }}
      >
        <div
          style={{
            width: 24,
            height: 24,
            borderRadius: 4,
            background: 'var(--accent)',
            color: 'var(--accent-text)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            fontSize: 13,
            fontWeight: 700,
          }}
        >
          B
        </div>
        <div style={{ flex: 1 }}>
          <div style={{ fontWeight: 600, fontSize: 13 }}>BugJuice</div>
          <div style={{ fontSize: 11, color: 'var(--text-muted)' }}>
            {summary.endPercent.toFixed(0)}% · {summary.onAc ? 'plugged in' : 'on battery'}
          </div>
        </div>
        <span style={{ fontSize: 11, color: 'var(--text-muted)' }}>now</span>
      </div>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {lines.length === 0 && (
          <div
            style={{
              color: 'var(--text-muted)',
              fontSize: 13,
              fontStyle: 'italic',
            }}
          >
            (nothing selected — pick at least one line above)
          </div>
        )}
        {lines.map((l, i) => (
          <div
            key={i}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              fontSize: 13,
              color: 'var(--text)',
            }}
          >
            <span style={{ width: 16, textAlign: 'center' }}>{l.icon}</span>
            <span>{l.text}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function Slider({
  min,
  max,
  step,
  value,
  unit,
  onChange,
}: {
  min: number;
  max: number;
  step: number;
  value: number;
  unit: string;
  onChange: (v: number) => void;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 12, minWidth: 240 }}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
        style={{
          flex: 1,
          accentColor: 'var(--accent)',
        }}
      />
      <span
        style={{
          fontSize: 14,
          fontWeight: 500,
          color: 'var(--text)',
          minWidth: 60,
          textAlign: 'right',
          fontVariantNumeric: 'tabular-nums',
        }}
      >
        {value}
        {unit}
      </span>
    </div>
  );
}
