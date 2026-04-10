import { useEffect, useMemo, useState } from 'react';
import {
  Area,
  AreaChart,
  CartesianGrid,
  ComposedChart,
  ReferenceArea,
  ResponsiveContainer,
  Scatter,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { getUnifiedTimeline, type UnifiedTimeline } from '@/api';
import type { BatterySession, SleepSession } from '@/types';

// ─── View modes ──────────────────────────────────────────────────────

type ViewMode = 'week' | 'day' | 'custom';

// ─── Main component ──────────────────────────────────────────────────

export function Sessions() {
  const [viewMode, setViewMode] = useState<ViewMode>('week');
  const [weekStart, setWeekStart] = useState<Date>(() => getLastSunday(new Date()));
  const [selectedDay, setSelectedDay] = useState<Date | null>(null);
  const [customStart, setCustomStart] = useState('');
  const [customEnd, setCustomEnd] = useState('');
  const [data, setData] = useState<UnifiedTimeline | null>(null);
  const [loading, setLoading] = useState(true);

  // Fetch data whenever view parameters change
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const { startTs, endTs } = getTimeRange(viewMode, weekStart, selectedDay, customStart, customEnd);
    getUnifiedTimeline(startTs, endTs).then((d) => {
      if (!cancelled) {
        setData(d);
        setLoading(false);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [viewMode, weekStart, selectedDay, customStart, customEnd]);

  // Derived data
  const { history, batterySessions, sleepSessions } = data ?? {
    history: [],
    batterySessions: [],
    sleepSessions: [],
  };

  const chargingSessions = useMemo(
    () => batterySessions.filter((s) => s.onAc),
    [batterySessions],
  );
  const dischargingSessions = useMemo(
    () => batterySessions.filter((s) => !s.onAc),
    [batterySessions],
  );

  // Summary stats
  const stats = useMemo(
    () => computeStats(batterySessions, sleepSessions),
    [batterySessions, sleepSessions],
  );

  // Anomaly points: where discharge rate exceeds 2x average
  const anomalyPoints = useMemo(() => {
    if (stats.avgDrainW === 0) return [];
    const threshold = Math.abs(stats.avgDrainW) * 2;
    return history
      .filter((p) => p.rateW < 0 && Math.abs(p.rateW) > threshold)
      .map((p) => ({ ts: p.ts, percent: p.percent }));
  }, [history, stats.avgDrainW]);

  // Unified session list (sorted newest first)
  const allSessions = useMemo(() => {
    const items: UnifiedSessionItem[] = [];
    for (const s of batterySessions) {
      items.push({ type: 'battery', session: s, sortTs: s.startedAt });
    }
    for (const s of sleepSessions) {
      items.push({ type: 'sleep', session: s, sortTs: s.sleepAt });
    }
    items.sort((a, b) => b.sortTs - a.sortTs);
    return items;
  }, [batterySessions, sleepSessions]);

  // X-axis time formatter based on view mode
  const formatAxisTime = (ts: number) => {
    const d = new Date(ts * 1000);
    if (viewMode === 'day') {
      return d.toLocaleTimeString(undefined, { hour: 'numeric', hour12: true });
    }
    return d.toLocaleDateString(undefined, { weekday: 'short' });
  };

  // Handle chart click for day drill-down
  const handleChartClick = (e: { activePayload?: Array<{ payload: { ts: number } }> } | null) => {
    if (viewMode === 'week' && e?.activePayload?.[0]) {
      const ts = e.activePayload[0].payload.ts;
      const clickedDate = new Date(ts * 1000);
      clickedDate.setHours(0, 0, 0, 0);
      setSelectedDay(clickedDate);
      setViewMode('day');
    }
  };

  return (
    <div className="page">
      {/* ─── Header bar ──────────────────────────────────────────── */}
      <header className="page-header" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <div>
          <h1 className="page-title">Sessions</h1>
          <p className="page-subtitle">
            {viewMode === 'day' && selectedDay
              ? formatDayTitle(selectedDay)
              : viewMode === 'custom'
                ? 'Custom date range'
                : formatWeekRange(weekStart)}
          </p>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          {viewMode === 'day' && (
            <button
              className="btn btn-ghost"
              onClick={() => { setViewMode('week'); setSelectedDay(null); }}
              style={ghostBtnStyle}
            >
              {'<-'} Back to Week
            </button>
          )}
          {viewMode === 'week' && (
            <>
              <button
                className="btn btn-ghost"
                onClick={() => setWeekStart((prev) => addDays(prev, -7))}
                style={ghostBtnStyle}
              >
                {'<'}
              </button>
              <span style={{ fontWeight: 500, fontSize: 14 }}>{formatWeekRange(weekStart)}</span>
              <button
                className="btn btn-ghost"
                onClick={() => setWeekStart((prev) => addDays(prev, 7))}
                style={ghostBtnStyle}
              >
                {'>'}
              </button>
            </>
          )}
          <button
            className="btn btn-ghost"
            onClick={() => setViewMode(viewMode === 'custom' ? 'week' : 'custom')}
            style={ghostBtnStyle}
            title="Custom date range"
          >
            {viewMode === 'custom' ? 'Week' : 'Custom'}
          </button>
        </div>
      </header>

      {/* ─── Custom date range picker ────────────────────────────── */}
      {viewMode === 'custom' && (
        <div style={{ display: 'flex', gap: 12, marginBottom: 4 }}>
          <input
            type="date"
            value={customStart}
            onChange={(e) => setCustomStart(e.target.value)}
            style={dateInputStyle}
          />
          <span style={{ alignSelf: 'center', color: 'var(--text-muted)', fontSize: 13 }}>to</span>
          <input
            type="date"
            value={customEnd}
            onChange={(e) => setCustomEnd(e.target.value)}
            style={dateInputStyle}
          />
        </div>
      )}

      {/* ─── Summary strip ─────────────────────────────────────────── */}
      <section className="grid grid-4">
        <SummaryCard
          label="Time on battery"
          value={humanDuration(stats.timeOnBatterySec)}
          context={`${dischargingSessions.length} discharge session${dischargingSessions.length !== 1 ? 's' : ''}`}
        />
        <SummaryCard
          label="Average drain"
          value={stats.avgDrainW !== 0 ? `${Math.abs(stats.avgDrainW).toFixed(1)} W` : '--'}
          context="mean discharge rate"
        />
        <SummaryCard
          label="Fastest discharge"
          value={stats.peakDrainW !== 0 ? `${Math.abs(stats.peakDrainW).toFixed(1)} W` : '--'}
          context="peak session drain"
          danger={stats.peakDrainW < -15}
        />
        <SummaryCard
          label="Sleep drain"
          value={
            stats.totalSleepDrainMwh > 0
              ? `${stats.totalSleepDrainMwh.toLocaleString()} mWh`
              : '--'
          }
          context={`${sleepSessions.length} sleep session${sleepSessions.length !== 1 ? 's' : ''}`}
        />
      </section>

      {/* ─── Battery timeline chart ────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Battery timeline</div>
            <div className="card-subtitle">
              {viewMode === 'week'
                ? 'Click a point to drill into that day'
                : viewMode === 'day'
                  ? 'Hourly battery percentage'
                  : 'Battery percentage over selected range'}
            </div>
          </div>
          {loading && (
            <span className="badge" style={{ fontSize: 11 }}>
              loading...
            </span>
          )}
        </div>
        <div style={{ height: 320 }}>
          {history.length === 0 ? (
            <div
              style={{
                height: '100%',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                color: 'var(--text-muted)',
                fontSize: 14,
              }}
            >
              {loading ? 'Loading timeline data...' : 'No history data for this period'}
            </div>
          ) : (
            <ResponsiveContainer width="100%" height="100%">
              <ComposedChart
                data={history}
                margin={{ top: 10, right: 12, left: -8, bottom: 0 }}
                onClick={handleChartClick}
                style={viewMode === 'week' ? { cursor: 'pointer' } : undefined}
              >
                <defs>
                  <linearGradient id="timelineFill" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor="var(--accent)" stopOpacity={0.45} />
                    <stop offset="100%" stopColor="var(--accent)" stopOpacity={0.02} />
                  </linearGradient>
                </defs>

                <CartesianGrid
                  stroke="var(--border)"
                  strokeDasharray="3 3"
                  vertical={false}
                />

                {/* Charging session bands (green) */}
                {chargingSessions.map((s, i) => (
                  <ReferenceArea
                    key={`charge-${i}`}
                    x1={s.startedAt}
                    x2={s.endedAt ?? Math.floor(Date.now() / 1000)}
                    fill="rgba(34, 197, 94, 0.15)"
                    fillOpacity={1}
                    ifOverflow="hidden"
                  />
                ))}

                {/* Sleep session bands (purple) */}
                {sleepSessions.map((s, i) => (
                  <ReferenceArea
                    key={`sleep-${i}`}
                    x1={s.sleepAt}
                    x2={s.wakeAt ?? Math.floor(Date.now() / 1000)}
                    fill="rgba(139, 92, 246, 0.15)"
                    fillOpacity={1}
                    ifOverflow="hidden"
                  />
                ))}

                <XAxis
                  dataKey="ts"
                  stroke="var(--text-muted)"
                  fontSize={11}
                  tickLine={false}
                  tickFormatter={formatAxisTime}
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
                  contentStyle={tooltipStyle}
                  labelFormatter={(ts: number) => new Date(ts * 1000).toLocaleString()}
                  formatter={(v: number, name: string) => {
                    if (name === 'percent') return [`${v.toFixed(1)}%`, 'Battery'];
                    return [`${v.toFixed(2)} W`, 'Rate'];
                  }}
                />

                <Area
                  type="monotone"
                  dataKey="percent"
                  stroke="var(--accent)"
                  strokeWidth={2}
                  fill="url(#timelineFill)"
                  isAnimationActive={false}
                />

                {/* Anomaly dots (high discharge) */}
                {anomalyPoints.length > 0 && (
                  <Scatter
                    data={anomalyPoints}
                    dataKey="percent"
                    fill="#ef4444"
                    isAnimationActive={false}
                    name="Anomaly"
                    shape={((props: unknown) => {
                      const { cx, cy } = props as { cx: number; cy: number };
                      return <circle cx={cx} cy={cy} r={4} fill="#ef4444" />;
                    }) as (props: unknown) => React.JSX.Element}
                  />
                )}
              </ComposedChart>
            </ResponsiveContainer>
          )}
        </div>

        {/* Chart legend */}
        {history.length > 0 && (
          <div
            style={{
              display: 'flex',
              gap: 20,
              marginTop: 12,
              paddingTop: 12,
              borderTop: '1px solid var(--border)',
              fontSize: 12,
              color: 'var(--text-muted)',
            }}
          >
            <LegendItem color="rgba(34, 197, 94, 0.4)" label="Charging" />
            <LegendItem color="rgba(139, 92, 246, 0.4)" label="Sleep" />
            <LegendItem color="#ef4444" label="Anomaly (>2x avg drain)" dot />
            {viewMode === 'week' && (
              <span style={{ marginLeft: 'auto', fontStyle: 'italic' }}>
                Click chart to view a single day
              </span>
            )}
          </div>
        )}
      </section>

      {/* ─── Component power chart ─────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Component power</div>
            <div className="card-subtitle">
              CPU, GPU, DRAM, and other subsystem draw over the period
            </div>
          </div>
        </div>
        <div style={{ height: 200 }}>
          {(data?.componentHistory ?? []).length === 0 ? (
            <div
              style={{
                height: '100%',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                color: 'var(--text-muted)',
                fontSize: 14,
              }}
            >
              {loading ? 'Loading component data...' : 'No component power data for this period'}
            </div>
          ) : (
            <ResponsiveContainer width="100%" height="100%">
              <AreaChart data={data!.componentHistory} margin={{ top: 10, right: 12, left: -8, bottom: 0 }}>
                <defs>
                  {(['cpu', 'gpu', 'dram', 'other'] as const).map((key, i) => {
                    const colors = ['var(--chart-1)', 'var(--chart-4)', 'var(--chart-5)', 'var(--chart-2)'];
                    return (
                      <linearGradient key={key} id={`sess-fill-${key}`} x1="0" y1="0" x2="0" y2="1">
                        <stop offset="0%" stopColor={colors[i]} stopOpacity={0.65} />
                        <stop offset="100%" stopColor={colors[i]} stopOpacity={0.1} />
                      </linearGradient>
                    );
                  })}
                </defs>
                <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" vertical={false} />
                <XAxis dataKey="ts" tickFormatter={formatAxisTime} stroke="var(--text-muted)" fontSize={11} tickLine={false} />
                <YAxis unit=" W" stroke="var(--text-muted)" fontSize={11} tickLine={false} width={48} />
                <Tooltip
                  contentStyle={tooltipStyle}
                  labelFormatter={(ts: number) => new Date(ts * 1000).toLocaleString()}
                  formatter={(v: number, name: string) => [`${v.toFixed(2)} W`, name.toUpperCase()]}
                />
                <Area type="monotone" dataKey="cpu" stackId="1" stroke="var(--chart-1)" strokeWidth={1.5} fill="url(#sess-fill-cpu)" isAnimationActive={false} name="CPU" />
                <Area type="monotone" dataKey="gpu" stackId="1" stroke="var(--chart-4)" strokeWidth={1.5} fill="url(#sess-fill-gpu)" isAnimationActive={false} name="GPU" />
                <Area type="monotone" dataKey="dram" stackId="1" stroke="var(--chart-5)" strokeWidth={1.5} fill="url(#sess-fill-dram)" isAnimationActive={false} name="DRAM" />
                <Area type="monotone" dataKey="other" stackId="1" stroke="var(--chart-2)" strokeWidth={1.5} fill="url(#sess-fill-other)" isAnimationActive={false} name="Other" />
              </AreaChart>
            </ResponsiveContainer>
          )}
        </div>
      </section>

      {/* ─── Top apps ──────────────────────────────────────────────── */}
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">
              Top Apps {viewMode === 'day' ? 'Today' : viewMode === 'week' ? 'This Week' : ''}
            </div>
            <div className="card-subtitle">Aggregated app power usage for this period</div>
          </div>
        </div>
        {(data?.appPowerSummary ?? []).length === 0 ? (
          <p className="stat-context" style={{ padding: '12px 0' }}>
            No app power data for this period
          </p>
        ) : (
          <div>
            {(data?.appPowerSummary ?? []).map((app) => (
              <div
                key={app.name}
                style={{
                  display: 'flex',
                  justifyContent: 'space-between',
                  alignItems: 'center',
                  padding: '10px 0',
                  borderBottom: '1px solid var(--border)',
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <span
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: '50%',
                      flexShrink: 0,
                      background:
                        app.avgWatts > 3
                          ? 'var(--bad)'
                          : app.avgWatts > 1
                            ? 'var(--warn)'
                            : 'var(--ok)',
                    }}
                  />
                  <span style={{ fontWeight: 500, fontSize: 14 }}>{app.name}</span>
                </div>
                <div style={{ display: 'flex', gap: 16, color: 'var(--text-subtle)', fontSize: 13, fontVariantNumeric: 'tabular-nums' }}>
                  <span>avg {app.avgWatts.toFixed(1)} W</span>
                  <span>peak {app.maxWatts.toFixed(1)} W</span>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* ─── Session list ──────────────────────────────────────────── */}
      <section className="card" style={{ padding: 0 }}>
        <div className="card-header" style={{ padding: 'var(--space-5)' }}>
          <div>
            <div className="card-title">Sessions</div>
            <div className="card-subtitle">
              {allSessions.length} session{allSessions.length !== 1 ? 's' : ''} in this period
            </div>
          </div>
        </div>
        {allSessions.length === 0 && (
          <div
            style={{
              padding: 32,
              textAlign: 'center',
              color: 'var(--text-muted)',
              fontSize: 14,
            }}
          >
            {loading
              ? 'Loading sessions...'
              : 'No sessions recorded in this time period.'}
          </div>
        )}
        {allSessions.map((item) =>
          item.type === 'battery' ? (
            <BatterySessionCard
              key={`bat-${item.session.id}`}
              session={item.session as BatterySession}
            />
          ) : (
            <SleepSessionCard
              key={`slp-${(item.session as SleepSession).id}`}
              session={item.session as SleepSession}
            />
          ),
        )}
      </section>
    </div>
  );
}

// ─── Summary card ────────────────────────────────────────────────────

function SummaryCard({
  label,
  value,
  context,
  danger,
}: {
  label: string;
  value: string;
  context: string;
  danger?: boolean;
}) {
  return (
    <div className="card">
      <div className="stat-label">{label}</div>
      <div
        className="stat-value"
        style={{
          fontSize: 24,
          marginTop: 8,
          color: danger ? '#ef4444' : 'var(--text)',
        }}
      >
        {value}
      </div>
      <div className="stat-context" style={{ marginTop: 6 }}>
        {context}
      </div>
    </div>
  );
}

// ─── Legend item ─────────────────────────────────────────────────────

function LegendItem({
  color,
  label,
  dot,
}: {
  color: string;
  label: string;
  dot?: boolean;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
      <span
        style={{
          width: dot ? 8 : 16,
          height: 8,
          borderRadius: dot ? '50%' : 2,
          background: color,
          flexShrink: 0,
        }}
      />
      <span>{label}</span>
    </div>
  );
}

// ─── Battery session card ────────────────────────────────────────────

function BatterySessionCard({ session }: { session: BatterySession }) {
  const isCharging = session.onAc;
  const isOpen = session.endedAt === null;
  const dur = session.endedAt
    ? humanDuration(session.endedAt - session.startedAt)
    : humanDuration(Math.floor(Date.now() / 1000) - session.startedAt);
  const dateLabel = formatDateRange(session.startedAt, session.endedAt);

  return (
    <div
      style={{
        width: '100%',
        display: 'grid',
        gridTemplateColumns: '40px 1fr auto auto',
        alignItems: 'center',
        gap: 16,
        padding: '16px 20px',
        textAlign: 'left',
        fontSize: 14,
        borderBottom: '1px solid var(--border)',
      }}
    >
      {/* Type icon */}
      <div
        style={{
          width: 36,
          height: 36,
          borderRadius: 8,
          background: isCharging ? 'rgba(34, 197, 94, 0.12)' : 'rgba(245, 158, 11, 0.12)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 18,
        }}
      >
        {isCharging ? '\u26A1' : '\uD83D\uDD0B'}
      </div>

      {/* Details */}
      <div style={{ minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontWeight: 500 }}>
          <span>{dateLabel}</span>
          {isOpen && (
            <span
              className="badge"
              style={{
                background: 'var(--accent-soft)',
                color: 'var(--accent)',
                fontSize: 11,
              }}
            >
              ongoing
            </span>
          )}
        </div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
          {isCharging ? 'Charging' : 'On battery'} &middot; {dur} &middot;{' '}
          {session.startPercent.toFixed(0)}%
          {' \u2192 '}
          {session.endPercent !== null ? `${session.endPercent.toFixed(0)}%` : '...'}
        </div>
      </div>

      {/* Key stat */}
      <div
        style={{
          fontVariantNumeric: 'tabular-nums',
          fontWeight: 600,
          color:
            session.avgDrainW && session.avgDrainW < 0 ? '#ef4444' : 'var(--accent)',
          textAlign: 'right',
        }}
      >
        {session.avgDrainW
          ? `${session.avgDrainW > 0 ? '+' : ''}${session.avgDrainW.toFixed(1)} W`
          : '\u2014'}
        <div style={{ fontSize: 11, color: 'var(--text-muted)', fontWeight: 400 }}>
          avg rate
        </div>
      </div>

      {/* Duration pill */}
      <div
        style={{
          padding: '4px 10px',
          borderRadius: 999,
          background: 'var(--bg-inset)',
          fontSize: 12,
          color: 'var(--text-subtle)',
          fontWeight: 500,
          whiteSpace: 'nowrap',
        }}
      >
        {dur}
      </div>
    </div>
  );
}

// ─── Sleep session card ──────────────────────────────────────────────

function SleepSessionCard({ session }: { session: SleepSession }) {
  const dur =
    session.wakeAt !== null ? humanDuration(session.wakeAt - session.sleepAt) : '\u2014';
  const dateLabel = formatDateRange(session.sleepAt, session.wakeAt);

  return (
    <div
      style={{
        width: '100%',
        display: 'grid',
        gridTemplateColumns: '40px 1fr auto auto',
        alignItems: 'center',
        gap: 16,
        padding: '16px 20px',
        textAlign: 'left',
        fontSize: 14,
        borderBottom: '1px solid var(--border)',
      }}
    >
      {/* Type icon */}
      <div
        style={{
          width: 36,
          height: 36,
          borderRadius: 8,
          background: 'rgba(139, 92, 246, 0.12)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 18,
        }}
      >
        {'\uD83C\uDF19'}
      </div>

      {/* Details */}
      <div style={{ minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontWeight: 500 }}>
          <span>{dateLabel}</span>
        </div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
          Sleep &middot; {dur}
          {session.dripsPercent != null && ` \u00B7 DRIPS ${session.dripsPercent.toFixed(0)}%`}
        </div>
      </div>

      {/* Key stat */}
      <div
        style={{
          fontVariantNumeric: 'tabular-nums',
          fontWeight: 600,
          textAlign: 'right',
        }}
      >
        {session.drainMwh != null ? `${session.drainMwh.toLocaleString()} mWh` : '\u2014'}
        <div style={{ fontSize: 11, color: 'var(--text-muted)', fontWeight: 400 }}>
          {session.drainRateMw != null
            ? `${session.drainRateMw.toFixed(0)} mW avg`
            : 'drain'}
        </div>
      </div>

      {/* Verdict badge */}
      <VerdictBadge verdict={session.verdict} />
    </div>
  );
}

// ─── Verdict badge ──────────────────────────────────────────────────

function VerdictBadge({ verdict }: { verdict: SleepSession['verdict'] }) {
  if (verdict == null) return <div style={{ width: 80 }} />;

  const configs: Record<
    string,
    { bg: string; color: string; dot: string; label: string }
  > = {
    excellent: {
      bg: 'var(--accent-soft)',
      color: 'var(--accent)',
      dot: 'var(--accent)',
      label: 'excellent',
    },
    normal: {
      bg: 'var(--bg-inset)',
      color: 'var(--text-subtle)',
      dot: 'var(--text-muted)',
      label: 'normal',
    },
    high: {
      bg: 'rgba(245, 158, 11, 0.12)',
      color: '#d97706',
      dot: '#f59e0b',
      label: 'high',
    },
    'very-high': {
      bg: 'rgba(239, 68, 68, 0.12)',
      color: '#dc2626',
      dot: '#ef4444',
      label: 'very high',
    },
  };

  const c = configs[verdict];
  if (!c) return null;

  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        padding: '4px 12px',
        borderRadius: 999,
        fontSize: 12,
        fontWeight: 500,
        background: c.bg,
        color: c.color,
        whiteSpace: 'nowrap',
      }}
    >
      <span
        style={{
          width: 6,
          height: 6,
          borderRadius: '50%',
          background: c.dot,
        }}
      />
      {c.label}
    </span>
  );
}

// ─── Helpers ────────────────────────────────────────────────────────

interface UnifiedSessionItem {
  type: 'battery' | 'sleep';
  session: BatterySession | SleepSession;
  sortTs: number;
}

interface SummaryStats {
  timeOnBatterySec: number;
  avgDrainW: number;
  peakDrainW: number;
  totalSleepDrainMwh: number;
}

function getTimeRange(
  viewMode: ViewMode,
  weekStart: Date,
  selectedDay: Date | null,
  customStart: string,
  customEnd: string,
): { startTs: number; endTs: number } {
  if (viewMode === 'day' && selectedDay) {
    const start = new Date(selectedDay);
    start.setHours(0, 0, 0, 0);
    const end = new Date(selectedDay);
    end.setHours(23, 59, 59, 999);
    return {
      startTs: Math.floor(start.getTime() / 1000),
      endTs: Math.floor(end.getTime() / 1000),
    };
  }
  if (viewMode === 'custom' && customStart && customEnd) {
    return {
      startTs: Math.floor(new Date(customStart).getTime() / 1000),
      endTs: Math.floor(new Date(customEnd + 'T23:59:59').getTime() / 1000),
    };
  }
  // Week view (default)
  const start = new Date(weekStart);
  const end = new Date(weekStart);
  end.setDate(end.getDate() + 7);
  return {
    startTs: Math.floor(start.getTime() / 1000),
    endTs: Math.floor(end.getTime() / 1000),
  };
}

function computeStats(
  batterySessions: BatterySession[],
  sleepSessions: SleepSession[],
): SummaryStats {
  const now = Math.floor(Date.now() / 1000);

  let timeOnBatterySec = 0;
  let drainSum = 0;
  let drainCount = 0;
  let peakDrainW = 0;

  for (const s of batterySessions) {
    if (!s.onAc) {
      const end = s.endedAt ?? now;
      timeOnBatterySec += end - s.startedAt;
      if (s.avgDrainW != null) {
        drainSum += s.avgDrainW;
        drainCount++;
        if (s.avgDrainW < peakDrainW) {
          peakDrainW = s.avgDrainW;
        }
      }
    }
  }

  const avgDrainW = drainCount > 0 ? drainSum / drainCount : 0;

  let totalSleepDrainMwh = 0;
  for (const s of sleepSessions) {
    if (s.drainMwh != null) {
      totalSleepDrainMwh += Math.abs(s.drainMwh);
    }
  }

  return { timeOnBatterySec, avgDrainW, peakDrainW, totalSleepDrainMwh };
}

function getLastSunday(d: Date): Date {
  const result = new Date(d);
  result.setDate(result.getDate() - result.getDay());
  result.setHours(0, 0, 0, 0);
  return result;
}

function addDays(d: Date, n: number): Date {
  const result = new Date(d);
  result.setDate(result.getDate() + n);
  return result;
}

function formatWeekRange(start: Date): string {
  const end = addDays(start, 6);
  const opts: Intl.DateTimeFormatOptions = { month: 'short', day: 'numeric' };
  return `${start.toLocaleDateString(undefined, opts)} \u2013 ${end.toLocaleDateString(undefined, { ...opts, year: 'numeric' })}`;
}

function formatDayTitle(d: Date): string {
  return d.toLocaleDateString(undefined, {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
    year: 'numeric',
  });
}

function humanDuration(sec: number): string {
  if (sec < 0) sec = 0;
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.round(sec / 60)} min`;
  const h = Math.floor(sec / 3600);
  const m = Math.round((sec % 3600) / 60);
  return m > 0 ? `${h}h ${m}m` : `${h}h`;
}

function formatDateRange(start: number, end: number | null): string {
  const s = new Date(start * 1000);
  if (end === null) {
    return `${formatShortDateTime(s)} \u2192 now`;
  }
  const e = new Date(end * 1000);
  if (
    s.getFullYear() === e.getFullYear() &&
    s.getMonth() === e.getMonth() &&
    s.getDate() === e.getDate()
  ) {
    return `${formatShortDateTime(s)} \u2192 ${formatShortTime(e)}`;
  }
  return `${formatShortDateTime(s)} \u2192 ${formatShortDateTime(e)}`;
}

function formatShortDateTime(d: Date): string {
  return d.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatShortTime(d: Date): string {
  return d.toLocaleString(undefined, { hour: '2-digit', minute: '2-digit' });
}

// ─── Shared styles ──────────────────────────────────────────────────

const tooltipStyle = {
  background: 'var(--bg-card)',
  border: '1px solid var(--border)',
  borderRadius: 'var(--radius)',
  fontSize: 12,
  color: 'var(--text)',
  boxShadow: 'var(--shadow)',
};

const ghostBtnStyle: React.CSSProperties = {
  padding: '6px 12px',
  borderRadius: 'var(--radius-sm)',
  border: '1px solid var(--border)',
  background: 'var(--bg-inset)',
  color: 'var(--text-subtle)',
  fontSize: 13,
  fontWeight: 500,
  cursor: 'pointer',
};

const dateInputStyle: React.CSSProperties = {
  padding: '8px 12px',
  borderRadius: 'var(--radius-sm)',
  border: '1px solid var(--border)',
  background: 'var(--bg-inset)',
  color: 'var(--text)',
  fontSize: 13,
};
