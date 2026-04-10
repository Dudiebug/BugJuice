import { useEffect } from 'react';
import { HashRouter, Route, Routes } from 'react-router-dom';
import { setNotificationPrefs } from './api';
import { Layout } from './components/Layout';
import { Apps } from './pages/Apps';
import { Components } from './pages/Components';
import { Dashboard } from './pages/Dashboard';
import { Health } from './pages/Health';
import { Sessions } from './pages/Sessions';
import { Settings } from './pages/Settings';

export function App() {
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
  }, []);

  return (
    <HashRouter>
      <Layout>
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
