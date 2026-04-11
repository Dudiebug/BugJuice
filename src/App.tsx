import { useEffect, useState } from 'react';
import { HashRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { setNotificationPrefs, getLhmStatus } from './api';
import type { LhmStatus } from './api';
import { Layout } from './components/Layout';
import { Apps } from './pages/Apps';
import { Components } from './pages/Components';
import { Dashboard } from './pages/Dashboard';
import { Health } from './pages/Health';
import { Sessions } from './pages/Sessions';
import { Settings } from './pages/Settings';

export function App() {
  const [updateAvailable, setUpdateAvailable] = useState<string | null>(null);
  const [lhmStatus, setLhmStatus] = useState<LhmStatus | null>(null);
  const [lhmDismissed, setLhmDismissed] = useState(
    () => localStorage.getItem('bugjuice-lhm-dismissed') === '1',
  );

  // Sync notification preferences to Rust backend on every startup.
  // The Rust-side prefs live in a Mutex<> static and are lost on restart,
  // so we must always re-send from localStorage (or defaults if empty).
  useEffect(() => {
    try {
      const raw = localStorage.getItem('bugjuice-prefs');
      const p = raw ? JSON.parse(raw) : {};
      setNotificationPrefs({
        notifyCharge: p.notifyCharge ?? true,
        chargeLimit: p.chargeLimit ?? 80,
        notifyLow: p.notifyLow ?? true,
        lowThreshold: p.lowThreshold ?? 20,
        notifySleepDrain: p.notifySleepDrain ?? true,
        summaryEnabled: p.summaryEnabled ?? true,
        summaryIntervalMin: p.summaryIntervalMin ?? 15,
        summaryOnlyOnBattery: p.summaryOnlyOnBattery ?? false,
        summaryShowRate: p.summaryShowRate ?? true,
        summaryShowEta: p.summaryShowEta ?? true,
        summaryShowDelta: p.summaryShowDelta ?? true,
        summaryShowTopApp: p.summaryShowTopApp ?? true,
      }).catch(() => {});
    } catch {}

    // Check for updates on startup.
    checkForUpdate().then(setUpdateAvailable).catch(() => {});

    // Check LHM status on startup + every 60s.
    getLhmStatus().then(setLhmStatus).catch(() => {});
    const lhmInterval = setInterval(() => {
      getLhmStatus().then((s) => {
        setLhmStatus(s);
        // Auto-clear dismissed state when LHM is detected.
        if (!s.needed || s.running) {
          setLhmDismissed(false);
          localStorage.removeItem('bugjuice-lhm-dismissed');
        }
      }).catch(() => {});
    }, 60_000);
    return () => clearInterval(lhmInterval);
  }, []);

  const showLhmBanner = lhmStatus?.needed && !lhmStatus?.running && !lhmDismissed;

  return (
    <HashRouter>
      <Layout>
        {updateAvailable && (
          <div
            style={{
              padding: '10px 20px',
              background: 'var(--accent-soft)',
              color: 'var(--accent)',
              fontSize: 13,
              fontWeight: 500,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
            }}
          >
            <span>BugJuice {updateAvailable} is available</span>
            <button
              onClick={async () => {
                try {
                  const { check } = await import('@tauri-apps/plugin-updater');
                  const update = await check();
                  if (update) {
                    await update.downloadAndInstall();
                    const { relaunch } = await import('@tauri-apps/plugin-process');
                    await relaunch();
                  }
                } catch (e) {
                  console.error('update failed:', e);
                }
              }}
              style={{
                padding: '4px 14px',
                background: 'var(--accent)',
                color: '#fff',
                border: 'none',
                borderRadius: 'var(--radius-sm)',
                fontSize: 12,
                fontWeight: 600,
                cursor: 'pointer',
              }}
            >
              Update &amp; restart
            </button>
          </div>
        )}
        {showLhmBanner && <LhmBanner onDismiss={() => {
          setLhmDismissed(true);
          localStorage.setItem('bugjuice-lhm-dismissed', '1');
        }} />}
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/components" element={<Components />} />
          <Route path="/apps" element={<Apps />} />
          <Route path="/sessions" element={<Sessions />} />
          <Route path="/health" element={<Health />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </Layout>
    </HashRouter>
  );
}

function LhmBanner({ onDismiss }: { onDismiss: () => void }) {
  const navigate = useNavigate();
  return (
    <div
      style={{
        padding: '10px 20px',
        background: 'hsl(38 90% 50% / 0.12)',
        color: 'hsl(38 80% 40%)',
        fontSize: 13,
        fontWeight: 500,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 12,
      }}
    >
      <span>CPU and GPU power monitoring requires a quick one-time setup</span>
      <div style={{ display: 'flex', gap: 8, flexShrink: 0 }}>
        <button
          onClick={() => navigate('/settings')}
          style={{
            padding: '4px 14px',
            background: 'hsl(38 80% 45%)',
            color: '#fff',
            border: 'none',
            borderRadius: 'var(--radius-sm)',
            fontSize: 12,
            fontWeight: 600,
            cursor: 'pointer',
          }}
        >
          Set up
        </button>
        <button
          onClick={onDismiss}
          style={{
            padding: '4px 10px',
            background: 'transparent',
            color: 'hsl(38 80% 40%)',
            border: '1px solid hsl(38 80% 40% / 0.3)',
            borderRadius: 'var(--radius-sm)',
            fontSize: 12,
            cursor: 'pointer',
          }}
        >
          Dismiss
        </button>
      </div>
    </div>
  );
}

async function checkForUpdate(): Promise<string | null> {
  try {
    const { check } = await import('@tauri-apps/plugin-updater');
    const update = await check();
    if (update?.available) {
      return update.version;
    }
  } catch {
    // Updater not configured or not in Tauri — silently ignore.
  }
  return null;
}
