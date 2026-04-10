import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { BatteryGauge } from '@/components/BatteryGauge';
import { useTick } from '@/hooks/useTick';
import { mock, toggleMockPowerSource } from '@/mock';

export function Dashboard() {
  useTick(2000);
  const status = mock.getStatus();
  const power = mock.getPower();
  const apps = mock.getApps().slice(0, 6);
  const history = mock.getHistory(60);

  const rateAbs = Math.abs(status.rateW);
  const charging = status.rateW > 0;

  return (
    <div className="page">
      <header className="page-header">
        <h1 className="page-title">Dashboard</h1>
        <p className="page-subtitle">Real-time battery, power, and top processes</p>
      </header>

      {/* ─── Hero: battery + current state ───────────────────────────── */}
      <section className="card">
        <div className="battery-hero">
          <BatteryGauge percent={status.percent} charging={charging} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'baseline', gap: 12, flexWrap: 'wrap' }}>
              <div className="stat-value">
                {status.percent.toFixed(1)}
                <span className="stat-unit">%</span>
              </div>
              <StateBadge onAc={status.onAc} rateW={status.rateW} />
            </div>
            <div className="stat-context" style={{ marginTop: 6, fontSize: 16 }}>
              {status.etaLabel}
            </div>
            <div className="stat-label" style={{ marginTop: 12 }}>
              {charging ? 'CHARGING AT' : 'DRAINING AT'}&nbsp;
              <strong style={{ fontSize: 18, color: 'var(--text)' }}>
                {rateAbs.toFixed(2)} W
              </strong>
            </div>
          </div>
          <button
            onClick={toggleMockPowerSource}
            style={{
              padding: '8px 16px',
              borderRadius: 'var(--radius-sm)',
              background: 'var(--bg-inset)',
              color: 'var(--text-subtle)',
              fontSize: 12,
              border: '1px solid var(--border)',
              whiteSpace: 'nowrap',
            }}
            title="Prototype-only: flip AC/DC to see how the numbers move"
          >
            toggle {status.onAc ? 'DC' : 'AC'}
          </button>
        </div>
      </section>

      {/* ─── Power breakdown ──────────────────────────────────────────── */}
      <section className="grid grid-4">
        <PowerCard title="Wall input" watts={power.wallInputW} sub="from charger" />
        <PowerCard
          title="System draw"
          watts={power.systemDrawW}
          sub="whole laptop"
        />
        <PowerCard
          title="CPU package"
          watts={power.cpuPackageW}
          sub={power.source.startsWith('Microsoft') ? 'RAPL via PPM' : 'EMI CPU clusters'}
        />
        <PowerCard title="GPU" watts={power.gpuW} sub="integrated" />
      </section>

      {/* ─── Battery history chart ────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Battery — last hour</div>
            <div className="card-subtitle">Percentage over time (simulated)</div>
          </div>
        </div>
        <div style={{ height: 220 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={history} margin={{ top: 10, right: 6, left: -10, bottom: 0 }}>
              <defs>
                <linearGradient id="batteryFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--accent)" stopOpacity={0.45} />
                  <stop offset="100%" stopColor="var(--accent)" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid
                stroke="var(--border)"
                vertical={false}
                strokeDasharray="3 3"
              />
              <XAxis
                dataKey="ts"
                stroke="var(--text-muted)"
                fontSize={11}
                tickLine={false}
                tickFormatter={(ts: number) => {
                  const d = new Date(ts * 1000);
                  return `${d.getHours().toString().padStart(2, '0')}:${d
                    .getMinutes()
                    .toString()
                    .padStart(2, '0')}`;
                }}
              />
              <YAxis
                domain={[0, 100]}
                stroke="var(--text-muted)"
                fontSize={11}
                tickLine={false}
                tickFormatter={(v) => `${v}%`}
                width={40}
              />
              <Tooltip
                contentStyle={{
                  background: 'var(--bg-card)',
                  border: '1px solid var(--border)',
                  borderRadius: 'var(--radius)',
                  fontSize: 12,
                  color: 'var(--text)',
                }}
                labelFormatter={(ts: number) => {
                  const d = new Date(ts * 1000);
                  return d.toLocaleTimeString();
                }}
                formatter={(value: number) => [`${value.toFixed(1)}%`, 'Battery']}
              />
              <Area
                type="monotone"
                dataKey="percent"
                stroke="var(--accent)"
                strokeWidth={2}
                fill="url(#batteryFill)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </section>

      {/* ─── Top apps now ─────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Top apps right now</div>
            <div className="card-subtitle">Estimated watts per process · updates every 2s</div>
          </div>
          <span className="badge badge-ok">live</span>
        </div>
        <div>
          {apps.map((app) => (
            <AppRow key={app.pid} app={app} maxWatts={apps[0].totalW} />
          ))}
        </div>
      </section>

      {/* ─── Secondary stats ──────────────────────────────────────────── */}
      <section className="grid grid-3">
        <MiniStat
          label="Voltage"
          value={status.voltageV.toFixed(2)}
          unit="V"
          context={`${status.capacityMwh.toLocaleString()} mWh of ${status.fullChargeMwh.toLocaleString()}`}
        />
        <MiniStat
          label="Temperature"
          value={status.tempC ? status.tempC.toFixed(1) : '—'}
          unit="°C"
          context={temperatureContext(status.tempC)}
        />
        <MiniStat
          label="Health"
          value={(100 - status.wearPercent).toFixed(0)}
          unit="%"
          context={`${status.cycleCount} cycles · ${status.chemistry}`}
        />
      </section>
    </div>
  );
}

// ─── Sub-components ────────────────────────────────────────────────────

function StateBadge({ onAc, rateW }: { onAc: boolean; rateW: number }) {
  const label = onAc && rateW > 0.1 ? 'Charging' : onAc ? 'On AC' : 'On battery';
  return <span className="badge badge-ok">{label}</span>;
}

function PowerCard({
  title,
  watts,
  sub,
}: {
  title: string;
  watts: number | null;
  sub: string;
}) {
  return (
    <div className="card">
      <div className="card-title" style={{ marginBottom: 8 }}>
        {title}
      </div>
      {watts !== null && watts !== undefined ? (
        <>
          <div className="stat-value" style={{ fontSize: 28 }}>
            {watts.toFixed(2)}
            <span className="stat-unit"> W</span>
          </div>
          <div className="stat-context" style={{ marginTop: 4 }}>
            {sub}
          </div>
        </>
      ) : (
        <>
          <div className="stat-value" style={{ fontSize: 28, color: 'var(--text-muted)' }}>
            —
          </div>
          <div className="stat-context" style={{ marginTop: 4 }}>
            not reported
          </div>
        </>
      )}
    </div>
  );
}

function AppRow({
  app,
  maxWatts,
}: {
  app: { pid: number; name: string; cpuW: number; gpuW: number; totalW: number };
  maxWatts: number;
}) {
  const barPct = maxWatts > 0 ? (app.totalW / maxWatts) * 100 : 0;
  return (
    <div className="power-row">
      <div className="power-row-label">
        <div className="power-row-name">{app.name}</div>
        <div className="power-row-sub">
          pid {app.pid} · {app.cpuW.toFixed(2)} W CPU
          {app.gpuW > 0.01 && ` · ${app.gpuW.toFixed(2)} W GPU`}
        </div>
      </div>
      <div className="bar">
        <div className="bar-fill" style={{ width: `${barPct}%` }} />
      </div>
      <div className="power-row-value">{app.totalW.toFixed(2)} W</div>
    </div>
  );
}

function MiniStat({
  label,
  value,
  unit,
  context,
}: {
  label: string;
  value: string;
  unit: string;
  context: string;
}) {
  return (
    <div className="card">
      <div className="stat-label">{label}</div>
      <div className="stat-value" style={{ fontSize: 28, marginTop: 8 }}>
        {value}
        <span className="stat-unit">{unit}</span>
      </div>
      <div className="stat-context" style={{ marginTop: 8 }}>
        {context}
      </div>
    </div>
  );
}

function temperatureContext(c: number | null): string {
  if (c === null) return 'not available';
  if (c < 25) return 'cool';
  if (c < 35) return 'normal';
  if (c < 45) return 'warm';
  return 'hot — check airflow';
}
