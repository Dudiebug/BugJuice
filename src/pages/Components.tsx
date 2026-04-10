import {
  Area,
  AreaChart,
  Cell,
  Legend,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { useTick } from '@/hooks/useTick';
import { mock } from '@/mock';

interface Slice {
  name: string;
  value: number;
  fill: string;
  desc: string;
}

export function Components() {
  useTick(2000);
  const power = mock.getPower();
  const history = mock.getComponentHistory(20);

  const cpu = power.cpuPackageW ?? 0;
  const gpu = power.gpuW ?? 0;
  const dram = power.dramW ?? 0;
  const system = power.systemDrawW ?? 0;
  // "Other" = everything in the system that isn't CPU/GPU/DRAM — display,
  // wifi, storage idle draw, platform controllers. Close to system draw
  // minus the component breakdown.
  const other = Math.max(0, system - cpu - gpu - dram);

  const slices: Slice[] = [
    { name: 'CPU', value: cpu, fill: 'var(--chart-1)', desc: 'processor package' },
    { name: 'GPU', value: gpu, fill: 'var(--chart-2)', desc: 'integrated graphics' },
    { name: 'DRAM', value: dram, fill: 'var(--chart-3)', desc: 'memory subsystem' },
    { name: 'Other', value: other, fill: 'var(--chart-4)', desc: 'display, Wi-Fi, platform' },
  ].filter((s) => s.value > 0.01);

  const total = slices.reduce((acc, s) => acc + s.value, 0);
  const biggest = [...slices].sort((a, b) => b.value - a.value)[0];

  return (
    <div className="page">
      <header className="page-header">
        <h1 className="page-title">Components</h1>
        <p className="page-subtitle">
          Power breakdown by subsystem · {power.source}
        </p>
      </header>

      {/* ─── Biggest drain callout ──────────────────────────────────── */}
      {biggest && (
        <section
          className="card"
          style={{
            background: 'var(--accent-soft)',
            borderColor: 'color-mix(in srgb, AccentColor 30%, transparent)',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
            <div
              style={{
                width: 48,
                height: 48,
                borderRadius: 12,
                background: 'var(--accent)',
                color: 'var(--accent-text)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: 24,
                fontWeight: 700,
              }}
            >
              ⚡
            </div>
            <div style={{ flex: 1 }}>
              <div className="stat-label">Biggest drain right now</div>
              <div style={{ fontSize: 22, fontWeight: 600, marginTop: 4 }}>
                {biggest.name} — {biggest.value.toFixed(2)} W (
                {((biggest.value / total) * 100).toFixed(0)}% of measured)
              </div>
              <div
                className="stat-context"
                style={{ marginTop: 4, fontSize: 13 }}
              >
                {biggest.desc}
              </div>
            </div>
          </div>
        </section>
      )}

      {/* ─── Pie + live breakdown ───────────────────────────────────── */}
      <section className="grid grid-2">
        <div className="card">
          <div className="card-header">
            <div>
              <div className="card-title">Current distribution</div>
              <div className="card-subtitle">
                Total measured: {total.toFixed(2)} W
              </div>
            </div>
          </div>
          <div style={{ height: 260 }}>
            <ResponsiveContainer width="100%" height="100%">
              <PieChart>
                <Pie
                  data={slices}
                  dataKey="value"
                  nameKey="name"
                  cx="50%"
                  cy="50%"
                  innerRadius={60}
                  outerRadius={95}
                  paddingAngle={2}
                  stroke="none"
                  isAnimationActive={false}
                >
                  {slices.map((s, i) => (
                    <Cell key={i} fill={s.fill} />
                  ))}
                </Pie>
                <Tooltip
                  contentStyle={tooltipStyle}
                  formatter={(v: number) => `${v.toFixed(2)} W`}
                />
                <Legend
                  verticalAlign="bottom"
                  iconType="circle"
                  wrapperStyle={{ fontSize: 12, color: 'var(--text-subtle)' }}
                />
              </PieChart>
            </ResponsiveContainer>
          </div>
        </div>

        <div className="card">
          <div className="card-header">
            <div>
              <div className="card-title">Component details</div>
              <div className="card-subtitle">
                Live values, refreshed every 2s
              </div>
            </div>
          </div>
          <div>
            {slices.map((s) => {
              const pct = total > 0 ? (s.value / total) * 100 : 0;
              return (
                <div className="power-row" key={s.name}>
                  <div className="power-row-label">
                    <div className="power-row-name">{s.name}</div>
                    <div className="power-row-sub">{s.desc}</div>
                  </div>
                  <div className="bar">
                    <div
                      className="bar-fill"
                      style={{ width: `${pct}%`, background: s.fill }}
                    />
                  </div>
                  <div className="power-row-value">{s.value.toFixed(2)} W</div>
                </div>
              );
            })}
          </div>
        </div>
      </section>

      {/* ─── Stacked area over time ─────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Power over the last 20 minutes</div>
            <div className="card-subtitle">
              Stacked view of each component's contribution
            </div>
          </div>
        </div>
        <div style={{ height: 260 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={history} margin={{ top: 10, right: 12, left: -8, bottom: 0 }}>
              <defs>
                {(['cpu', 'gpu', 'dram', 'other'] as const).map((key, i) => (
                  <linearGradient
                    key={key}
                    id={`fill-${key}`}
                    x1="0"
                    y1="0"
                    x2="0"
                    y2="1"
                  >
                    <stop
                      offset="0%"
                      stopColor={`var(--chart-${i + 1})`}
                      stopOpacity={0.65}
                    />
                    <stop
                      offset="100%"
                      stopColor={`var(--chart-${i + 1})`}
                      stopOpacity={0.1}
                    />
                  </linearGradient>
                ))}
              </defs>
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
                stroke="var(--text-muted)"
                fontSize={11}
                tickLine={false}
                tickFormatter={(v) => `${v} W`}
                width={40}
              />
              <Tooltip
                contentStyle={tooltipStyle}
                labelFormatter={(ts: number) =>
                  new Date(ts * 1000).toLocaleTimeString()
                }
                formatter={(v: number, name: string) => [
                  `${v.toFixed(2)} W`,
                  name.toUpperCase(),
                ]}
              />
              <Area
                type="monotone"
                dataKey="cpu"
                stackId="1"
                stroke="var(--chart-1)"
                strokeWidth={1.5}
                fill="url(#fill-cpu)"
                isAnimationActive={false}
              />
              <Area
                type="monotone"
                dataKey="gpu"
                stackId="1"
                stroke="var(--chart-2)"
                strokeWidth={1.5}
                fill="url(#fill-gpu)"
                isAnimationActive={false}
              />
              <Area
                type="monotone"
                dataKey="dram"
                stackId="1"
                stroke="var(--chart-3)"
                strokeWidth={1.5}
                fill="url(#fill-dram)"
                isAnimationActive={false}
              />
              <Area
                type="monotone"
                dataKey="other"
                stackId="1"
                stroke="var(--chart-4)"
                strokeWidth={1.5}
                fill="url(#fill-other)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </section>
    </div>
  );
}

const tooltipStyle = {
  background: 'var(--bg-card)',
  border: '1px solid var(--border)',
  borderRadius: 'var(--radius)',
  fontSize: 12,
  color: 'var(--text)',
  boxShadow: 'var(--shadow)',
};
