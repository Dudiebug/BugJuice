import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { useTick } from '@/hooks/useTick';
import { mock } from '@/mock';

export function Health() {
  useTick(30_000);
  const status = mock.getStatus();
  const history = mock.getHealthHistory();
  const latest = history[history.length - 1];
  const first = history[0];

  const healthPct = 100 - latest.wearPercent;
  const wearRatePerMonth = (latest.wearPercent - first.wearPercent) / 11;
  const expectedLifespanCycles = 1000;
  const cyclesRemaining = Math.max(0, expectedLifespanCycles - latest.cycleCount);
  const monthsOfUse = 11;
  const cyclesPerMonth = latest.cycleCount / Math.max(1, monthsOfUse);
  const monthsUntilReplace = Math.round(cyclesRemaining / Math.max(0.1, cyclesPerMonth));

  return (
    <div className="page">
      <header className="page-header">
        <h1 className="page-title">Health</h1>
        <p className="page-subtitle">
          Long-term wear, charge habits, and projected lifespan
        </p>
      </header>

      {/* ─── Headline verdict card ──────────────────────────────────── */}
      <section className="card">
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: '1fr 1fr 1fr 1fr',
            gap: 32,
          }}
        >
          <HeroStat
            value={healthPct.toFixed(0)}
            unit="%"
            label="Health"
            context={healthVerdict(healthPct)}
            accent
          />
          <HeroStat
            value={latest.cycleCount.toString()}
            unit=""
            label="Cycles used"
            context={`of ~${expectedLifespanCycles} expected`}
          />
          <HeroStat
            value={latest.wearPercent.toFixed(1)}
            unit="%"
            label="Wear"
            context={`~${wearRatePerMonth.toFixed(2)}% per month`}
          />
          <HeroStat
            value={monthsUntilReplace.toString()}
            unit="mo"
            label="Projected lifespan"
            context="at current usage rate"
          />
        </div>
      </section>

      {/* ─── Wear curve chart ───────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Capacity over time</div>
            <div className="card-subtitle">
              Full-charge capacity across the last {history.length} months
            </div>
          </div>
          <span className="badge badge-ok">
            {((latest.fullChargeCapacity / latest.designCapacity) * 100).toFixed(0)}% of design
          </span>
        </div>
        <div style={{ height: 280 }}>
          <ResponsiveContainer width="100%" height="100%">
            <LineChart
              data={history}
              margin={{ top: 10, right: 16, left: -4, bottom: 0 }}
            >
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
              <XAxis
                dataKey="ts"
                stroke="var(--text-muted)"
                fontSize={11}
                tickLine={false}
                tickFormatter={(ts: number) => {
                  const d = new Date(ts * 1000);
                  return d.toLocaleDateString(undefined, { month: 'short' });
                }}
              />
              <YAxis
                stroke="var(--text-muted)"
                fontSize={11}
                tickLine={false}
                tickFormatter={(v) => `${(v / 1000).toFixed(0)}k`}
                width={50}
              />
              <Tooltip
                contentStyle={tooltipStyle}
                labelFormatter={(ts: number) =>
                  new Date(ts * 1000).toLocaleDateString(undefined, {
                    year: 'numeric',
                    month: 'long',
                  })
                }
                formatter={(v: number, name: string) => {
                  if (name === 'designCapacity') return [`${v.toLocaleString()} mWh`, 'Design'];
                  return [`${v.toLocaleString()} mWh`, 'Full charge'];
                }}
              />
              <Line
                type="monotone"
                dataKey="designCapacity"
                stroke="var(--text-muted)"
                strokeWidth={1.5}
                strokeDasharray="6 6"
                dot={false}
                isAnimationActive={false}
              />
              <Line
                type="monotone"
                dataKey="fullChargeCapacity"
                stroke="var(--chart-1)"
                strokeWidth={3}
                dot={{ fill: 'var(--chart-1)', r: 3 }}
                isAnimationActive={false}
              />
            </LineChart>
          </ResponsiveContainer>
        </div>
      </section>

      {/* ─── Secondary context cards ────────────────────────────────── */}
      <section className="grid grid-2">
        <div className="card">
          <div className="card-title">Charge habits</div>
          <p style={{ marginTop: 12, color: 'var(--text-subtle)', fontSize: 14 }}>
            You mostly keep the battery between <strong>30%</strong> and <strong>90%</strong>,
            which is ideal for lithium-ion longevity. Avoiding full-to-empty
            cycles and keeping below 90% at night can extend lifespan by
            <strong> 20-40%</strong>.
          </p>
          <div
            style={{
              marginTop: 16,
              padding: 12,
              background: 'var(--bg-inset)',
              borderRadius: 'var(--radius-sm)',
              fontSize: 13,
              color: 'var(--text-subtle)',
            }}
          >
            <strong style={{ color: 'var(--text)' }}>Tip:</strong> consider
            enabling BugJuice's 80% charge-limit reminder in Settings.
          </div>
        </div>

        <div className="card">
          <div className="card-title">Design specs</div>
          <div style={{ marginTop: 12, fontSize: 14 }}>
            <SpecRow label="Chemistry" value={status.chemistry} />
            <SpecRow label="Manufacturer" value={status.manufacturer} />
            <SpecRow label="Model" value={status.deviceName} />
            <SpecRow
              label="Designed capacity"
              value={`${status.designMwh.toLocaleString()} mWh`}
            />
            <SpecRow
              label="Current full charge"
              value={`${status.fullChargeMwh.toLocaleString()} mWh`}
            />
            <SpecRow label="Voltage (nominal)" value={`${status.voltageV.toFixed(1)} V`} />
          </div>
        </div>
      </section>
    </div>
  );
}

// ─── Sub-components ────────────────────────────────────────────────

function HeroStat({
  value,
  unit,
  label,
  context,
  accent,
}: {
  value: string;
  unit: string;
  label: string;
  context: string;
  accent?: boolean;
}) {
  return (
    <div>
      <div className="stat-label">{label}</div>
      <div
        className="stat-value"
        style={{
          fontSize: 36,
          marginTop: 6,
          color: accent ? 'var(--accent)' : 'var(--text)',
        }}
      >
        {value}
        <span className="stat-unit">{unit}</span>
      </div>
      <div className="stat-context" style={{ marginTop: 4 }}>
        {context}
      </div>
    </div>
  );
}

function SpecRow({ label, value }: { label: string; value: string }) {
  return (
    <div
      style={{
        display: 'flex',
        justifyContent: 'space-between',
        padding: '8px 0',
        borderBottom: '1px solid var(--border)',
      }}
    >
      <span style={{ color: 'var(--text-muted)' }}>{label}</span>
      <span style={{ fontWeight: 500 }}>{value}</span>
    </div>
  );
}

function healthVerdict(pct: number): string {
  if (pct >= 95) return 'still healthy';
  if (pct >= 85) return 'some wear, normal for age';
  if (pct >= 70) return 'noticeably worn';
  return 'heavy wear, nearing replacement';
}

const tooltipStyle = {
  background: 'var(--bg-card)',
  border: '1px solid var(--border)',
  borderRadius: 'var(--radius)',
  fontSize: 12,
  color: 'var(--text)',
  boxShadow: 'var(--shadow)',
};
