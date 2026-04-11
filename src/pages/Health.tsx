import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { getBatteryStatus, getChargeHabits, getHealthHistory } from '@/api';
import { useApi } from '@/hooks/useApi';

export function Health() {
  const status = useApi(getBatteryStatus, 30_000);
  const historyData = useApi(getHealthHistory, 60_000);
  const habits = useApi(getChargeHabits, 60_000);

  if (!status) {
    return (
      <div className="page">
        <div className="page-header">
          <h1 className="page-title">Health</h1>
          <p className="page-subtitle">Loading…</p>
        </div>
      </div>
    );
  }

  const history = historyData ?? [];
  // We always have the *current* snapshot from getBatteryStatus. The
  // historical chart is only meaningful once we have ≥ 2 snapshots spread
  // across real time.
  const latest = history.length > 0 ? history[history.length - 1] : {
    ts: Math.floor(Date.now() / 1000),
    designCapacity: status.designMwh,
    fullChargeCapacity: status.fullChargeMwh,
    cycleCount: status.cycleCount,
    wearPercent: status.wearPercent,
  };
  const first = history.length > 0 ? history[0] : latest;

  const healthPct = Math.max(0, 100 - latest.wearPercent);
  const expectedLifespanCycles = 1000;
  const cyclesRemaining = Math.max(0, expectedLifespanCycles - latest.cycleCount);

  // Real months-of-use and wear rate: only compute when we have snapshots
  // that actually span some time. Otherwise we don't know — show an em-dash.
  const spanSec = latest.ts - first.ts;
  const spanMonths = spanSec / (60 * 60 * 24 * 30.44);
  const haveRealTrend = history.length >= 2 && spanMonths >= 0.25; // ≥ ~1 week
  const wearDelta = latest.wearPercent - first.wearPercent;
  const wearRatePerMonth = haveRealTrend ? wearDelta / spanMonths : null;
  const cyclesPerMonth = haveRealTrend
    ? (latest.cycleCount - first.cycleCount) / spanMonths
    : null;
  const monthsUntilReplace =
    cyclesPerMonth && cyclesPerMonth > 0.1
      ? Math.round(cyclesRemaining / cyclesPerMonth)
      : null;

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
            context={
              wearRatePerMonth !== null
                ? `~${wearRatePerMonth.toFixed(2)}% per month`
                : 'trend unavailable yet'
            }
          />
          <HeroStat
            value={monthsUntilReplace !== null ? monthsUntilReplace.toString() : '—'}
            unit={monthsUntilReplace !== null ? 'mo' : ''}
            label="Projected lifespan"
            context={
              monthsUntilReplace !== null
                ? 'at current usage rate'
                : 'needs ≥ 1 week of data'
            }
          />
        </div>
      </section>

      {/* ─── Wear curve chart ───────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Capacity over time</div>
            <div className="card-subtitle">
              {history.length < 2
                ? 'Collecting health snapshots — one per minute of runtime'
                : `Full-charge capacity across ${formatSpan(spanSec)} (${history.length} snapshots)`}
            </div>
          </div>
          <span className="badge badge-ok">
            {((latest.fullChargeCapacity / Math.max(1, latest.designCapacity)) * 100).toFixed(0)}% of design
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
          {(!habits || !habits.hasEnoughData) ? (
            <>
              <p style={{ marginTop: 12, color: 'var(--text-subtle)', fontSize: 14 }}>
                Collecting charge sessions — need a couple more
                charge/discharge cycles before BugJuice can score your habits.
              </p>
              <p style={{ marginTop: 8, color: 'var(--text-subtle)', fontSize: 13 }}>
                In the meantime: keep the battery between{' '}
                <strong>20%</strong> and <strong>80%</strong> most of the
                time, and avoid leaving it at 100% for long periods.
              </p>
            </>
          ) : (
            <>
              {/* Score + verdict */}
              <div style={{ display: 'flex', alignItems: 'baseline', gap: 12, marginTop: 12 }}>
                <span
                  style={{
                    fontSize: 40,
                    fontWeight: 700,
                    color: habits.score >= 70 ? 'var(--accent)' : habits.score >= 50 ? 'var(--text)' : 'var(--danger, #ef4444)',
                  }}
                >
                  {habits.score}
                </span>
                <span style={{ fontSize: 14, color: 'var(--text-muted)' }}>
                  / 100 — {habits.verdict}
                </span>
              </div>

              {/* Provisional warning */}
              {habits.isProvisional && (
                <p
                  style={{
                    marginTop: 8,
                    padding: '8px 12px',
                    background: 'var(--bg-inset)',
                    borderRadius: 'var(--radius-sm)',
                    fontSize: 12,
                    color: 'var(--text-muted)',
                  }}
                >
                  Based on {habits.dataDays < 1 ? 'less than a day' : `~${Math.round(habits.dataDays)} day${Math.round(habits.dataDays) === 1 ? '' : 's'}`} of
                  data — this score may not reflect your actual habits until
                  after the first week.
                </p>
              )}

              {/* Metrics grid */}
              <div
                style={{
                  display: 'grid',
                  gridTemplateColumns: '1fr 1fr',
                  gap: '12px 24px',
                  marginTop: 16,
                  fontSize: 13,
                }}
              >
                <MetricRow
                  label="Avg max charge"
                  value={`${habits.metrics.avgMaxCharge.toFixed(0)}%`}
                />
                <MetricRow
                  label="Charged above 80%"
                  value={`${habits.metrics.overchargePct.toFixed(0)}% of sessions`}
                />
                <MetricRow
                  label="Drained below 20%"
                  value={`${habits.metrics.deepDischargePct.toFixed(0)}% of sessions`}
                />
                <MetricRow
                  label="Time at 100%"
                  value={
                    habits.metrics.timeAt100Minutes < 60
                      ? `${Math.round(habits.metrics.timeAt100Minutes)} min`
                      : `${(habits.metrics.timeAt100Minutes / 60).toFixed(1)} hrs`
                  }
                />
              </div>

              {/* Tips */}
              {habits.tips.length > 0 && (
                <ul
                  style={{
                    marginTop: 14,
                    paddingLeft: 18,
                    fontSize: 13,
                    color: 'var(--text-subtle)',
                    lineHeight: 1.6,
                  }}
                >
                  {habits.tips.map((tip, i) => (
                    <li key={i}>{tip}</li>
                  ))}
                </ul>
              )}
            </>
          )}

          {/* Always show the 80% reminder tip */}
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
            <strong style={{ color: 'var(--text)' }}>Tip:</strong> enable
            BugJuice's 80% charge-limit reminder in Settings.
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

function MetricRow({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ display: 'flex', justifyContent: 'space-between' }}>
      <span style={{ color: 'var(--text-muted)' }}>{label}</span>
      <span style={{ fontWeight: 500 }}>{value}</span>
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

function formatSpan(sec: number): string {
  if (sec < 60) return `${Math.round(sec)}s`;
  const min = sec / 60;
  if (min < 60) return `${Math.round(min)} min`;
  const hr = min / 60;
  if (hr < 48) return `${hr.toFixed(1)} hours`;
  const days = hr / 24;
  if (days < 60) return `${Math.round(days)} days`;
  const months = days / 30.44;
  return `${months.toFixed(1)} months`;
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
