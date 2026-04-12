import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import {
  getBatteryHistory,
  getBatteryStatus,
  getChargeSpeed,
  getPowerReading,
  getTopApps,
  getUnplugEstimate,
} from '@/api';
import { BatteryGauge } from '@/components/BatteryGauge';
import { useApi } from '@/hooks/useApi';

export function Dashboard() {
  const status = useApi(getBatteryStatus, 2000);
  const power = useApi(getPowerReading, 2000);
  const appsResponse = useApi(getTopApps, 2000);
  const chargeSpeed = useApi(getChargeSpeed, 5000);
  const unplugEstimate = useApi(getUnplugEstimate, 10_000);
  const history = useApi(() => getBatteryHistory(60), 30_000);

  if (!status || !power) {
    return (
      <div className="page">
        <div className="page-header">
          <h1 className="page-title">Dashboard</h1>
          <p className="page-subtitle">Loading…</p>
        </div>
      </div>
    );
  }

  const apps = (appsResponse?.apps ?? []).slice(0, 6);
  const rateAbs = Math.abs(status.rateW);
  const rateKnown = rateAbs >= 0.01;
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
              {rateKnown ? (
                <>
                  {charging ? 'CHARGING AT' : 'DRAINING AT'}&nbsp;
                  <strong style={{ fontSize: 18, color: 'var(--text)' }}>
                    {rateAbs.toFixed(2)} W
                  </strong>
                </>
              ) : status.onAc ? (
                <strong style={{ fontSize: 18, color: 'var(--text)' }}>
                  On AC
                </strong>
              ) : (
                <strong style={{ fontSize: 18, color: 'var(--text-muted)' }}>
                  Rate not available
                </strong>
              )}
            </div>
          </div>
        </div>
      </section>

      {/* ─── Charging speed card (visible only while charging) ────────── */}
      {status.onAc && chargeSpeed && chargeSpeed.currentRateW > 0.1 && (
        <section className="card">
          <div className="card-header">
            <div>
              <div className="card-title">Charging speed</div>
              <div className="card-subtitle">{chargeSpeed.etaLabel}</div>
            </div>
            <span className="badge badge-ok">charging</span>
          </div>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: '1fr 1fr 1fr 1fr',
              gap: 24,
              marginTop: 12,
            }}
          >
            <MiniStat
              label="Current"
              value={`${chargeSpeed.currentRateW.toFixed(1)} W`}
            />
            <MiniStat
              label="Peak this session"
              value={`${chargeSpeed.maxRateW.toFixed(1)} W`}
            />
            <MiniStat
              label="Average"
              value={`${chargeSpeed.avgRateW.toFixed(1)} W`}
            />
            <MiniStat
              label="Gained"
              value={`+${(chargeSpeed.currentPercent - chargeSpeed.startPercent).toFixed(1)}%`}
            />
          </div>
        </section>
      )}

      {/* ─── "Before I unplug" estimate (visible only on AC) ────────── */}
      {status.onAc && unplugEstimate && unplugEstimate.totalHours > 0 && (
        <section className="card">
          <div className="card-header">
            <div>
              <div className="card-title">If you unplug now</div>
              <div className="card-subtitle">{unplugEstimate.totalLabel}</div>
            </div>
          </div>
          {unplugEstimate.topDrains.length > 0 && (
            <div style={{ marginTop: 12, fontSize: 13 }}>
              {unplugEstimate.topDrains.map((d) => (
                <div
                  key={d.name}
                  style={{
                    display: 'flex',
                    justifyContent: 'space-between',
                    padding: '6px 0',
                    borderBottom: '1px solid var(--border)',
                  }}
                >
                  <span>{d.name}</span>
                  <span style={{ color: 'var(--text-muted)', fontVariantNumeric: 'tabular-nums' }}>
                    {d.watts.toFixed(2)} W
                  </span>
                </div>
              ))}
              {unplugEstimate.systemOverheadW > 0.1 && (
                <div
                  style={{
                    display: 'flex',
                    justifyContent: 'space-between',
                    padding: '6px 0',
                    color: 'var(--text-muted)',
                    fontSize: 12,
                  }}
                >
                  <span>System / platform</span>
                  <span style={{ fontVariantNumeric: 'tabular-nums' }}>
                    {unplugEstimate.systemOverheadW.toFixed(2)} W
                  </span>
                </div>
              )}
            </div>
          )}
          <div
            style={{
              marginTop: 10,
              fontSize: 11,
              color: 'var(--text-muted)',
            }}
          >
            Based on current usage — actual battery life depends on what you run
          </div>
        </section>
      )}

      {/* ─── Power breakdown ──────────────────────────────────────────── */}
      {(() => {
        const cards: { title: string; watts: number | null; sub: string }[] = [];
        if (power.wallInputW != null) {
          cards.push({ title: 'Wall input', watts: power.wallInputW, sub: 'from charger' });
        } else if (rateKnown) {
          cards.push({
            title: charging ? 'Charge rate' : 'Discharge rate',
            watts: rateAbs,
            sub: charging ? 'from charger' : 'battery drain',
          });
        }
        if (power.systemDrawW != null) cards.push({ title: 'System draw', watts: power.systemDrawW, sub: 'whole laptop' });
        if (power.cpuPackageW != null) cards.push({ title: 'CPU package', watts: power.cpuPackageW, sub: power.source.startsWith('Microsoft') ? 'RAPL via PPM' : 'EMI CPU clusters' });
        if (power.gpuW != null) cards.push({ title: 'GPU', watts: power.gpuW, sub: 'integrated' });
        if (cards.length === 0) return null;
        return (
          <section className={`grid grid-${Math.min(cards.length, 4) as 2 | 3 | 4}`}>
            {cards.map((c) => (
              <PowerCard key={c.title} title={c.title} watts={c.watts} sub={c.sub} />
            ))}
          </section>
        );
      })()}

      {/* ─── Battery history chart ────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Battery — last hour</div>
            <div className="card-subtitle">Percentage over time</div>
          </div>
        </div>
        <div style={{ height: 220 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={history ?? []} margin={{ top: 10, right: 6, left: -10, bottom: 0 }}>
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
          {apps.length === 0 ? (
            <div className="stat-context" style={{ padding: '16px 4px' }}>
              Collecting per-process power data…
            </div>
          ) : (
            apps.map((app) => (
              <AppRow key={app.pid} app={app} maxWatts={apps[0].totalW} />
            ))
          )}
        </div>
      </section>

      {/* ─── Secondary stats ──────────────────────────────────────────── */}
      <section className="grid grid-2">
        <StatCard
          label="Voltage"
          value={status.voltageV.toFixed(2)}
          unit="V"
          context={`${status.capacityMwh.toLocaleString()} mWh of ${status.fullChargeMwh.toLocaleString()}`}
        />
        <StatCard
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

function MiniStat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="stat-label">{label}</div>
      <div style={{ fontSize: 20, fontWeight: 600, marginTop: 4 }}>{value}</div>
    </div>
  );
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
            {watts < 1 ? watts.toFixed(3) : watts.toFixed(2)}
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

function StatCard({
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

