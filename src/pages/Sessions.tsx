import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import {
  Area,
  AreaChart,
  CartesianGrid,
  ComposedChart,
  ReferenceArea,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { getUnifiedTimeline, type UnifiedTimeline } from '@/api';
import type { BatterySession, HistoryPoint, SleepSession } from '@/types';

// ─── View modes ──────────────────────────────────────────────────────

type ViewMode = 'week' | 'day' | 'custom';

// ─── Main component ──────────────────────────────────────────────────

export function Sessions() {
  const [viewMode, setViewMode] = useState<ViewMode>('week');
  const [weekStart, setWeekStart] = useState<Date>(() => {
    const d = new Date();
    d.setHours(0, 0, 0, 0);
    d.setDate(d.getDate() - 6);
    return d;
  });
  const [selectedDay, setSelectedDay] = useState<Date | null>(null);
  const [customStart, setCustomStart] = useState('');
  const [customEnd, setCustomEnd] = useState('');
  const [data, setData] = useState<UnifiedTimeline | null>(null);
  const [loading, setLoading] = useState(true);

  // Window bounds for the current view. Lifted out of the fetch effect so
  // the chart, stats, and cursor overlay can all share the same numeric
  // range without re-deriving it.
  const { startTs: windowStartTs, endTs: windowEndTs } = useMemo(
    () => getTimeRange(viewMode, weekStart, selectedDay, customStart, customEnd),
    [viewMode, weekStart, selectedDay, customStart, customEnd],
  );

  // Fetch data whenever the window changes
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    getUnifiedTimeline(windowStartTs, windowEndTs).then((d) => {
      if (!cancelled) {
        setData(d);
        setLoading(false);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [windowStartTs, windowEndTs]);

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

  // ── Gap detection ──────────────────────────────────────────────────
  // Walk through consecutive history points and flag time gaps where the
  // app wasn't recording (sleep, laptop off, etc.). Gaps that overlap a
  // known sleep session are tagged as "sleep"; everything else is
  // "interpolated" and will be shown with diagonal hatching.
  const GAP_THRESHOLD = 300; // 5 minutes in seconds

  const gaps = useMemo(() => {
    if (history.length < 2) return [] as GapRegion[];
    const result: GapRegion[] = [];
    for (let i = 0; i < history.length - 1; i++) {
      const curr = history[i];
      const next = history[i + 1];
      if (next.ts - curr.ts > GAP_THRESHOLD) {
        const isSleep = sleepSessions.some(
          (s) => s.sleepAt < next.ts && (s.wakeAt ?? Infinity) > curr.ts,
        );
        result.push({
          startTs: curr.ts,
          endTs: next.ts,
          type: isSleep ? 'sleep' : 'interpolated',
        });
      }
    }
    return result;
  }, [history, sleepSessions, GAP_THRESHOLD]);

  const interpolatedGaps = useMemo(
    () => gaps.filter((g) => g.type === 'interpolated'),
    [gaps],
  );

  // ── Component chart: no-data region detection ──────────────────────
  // Walk componentHistory and emit bands where there's no data so we can
  // paint diagonal hatching over them (same treatment as battery gaps).
  const componentNoDataRegions = useMemo(() => {
    const ch = data?.componentHistory ?? [];
    const regions: { startTs: number; endTs: number }[] = [];
    if (ch.length === 0) {
      regions.push({ startTs: windowStartTs, endTs: windowEndTs });
      return regions;
    }
    if (ch[0].ts - windowStartTs > GAP_THRESHOLD)
      regions.push({ startTs: windowStartTs, endTs: ch[0].ts });
    for (let i = 0; i < ch.length - 1; i++) {
      if (ch[i + 1].ts - ch[i].ts > GAP_THRESHOLD)
        regions.push({ startTs: ch[i].ts, endTs: ch[i + 1].ts });
    }
    if (windowEndTs - ch[ch.length - 1].ts > GAP_THRESHOLD)
      regions.push({ startTs: ch[ch.length - 1].ts, endTs: windowEndTs });
    return regions;
  }, [data?.componentHistory, windowStartTs, windowEndTs, GAP_THRESHOLD]);

  // Inject null rows at each interior gap so Recharts breaks the area
  // instead of drawing a line straight across the missing period.
  const processedComponentHistory = useMemo(() => {
    const ch = data?.componentHistory ?? [];
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const out: any[] = [];
    for (let i = 0; i < ch.length; i++) {
      out.push(ch[i]);
      if (i < ch.length - 1 && ch[i + 1].ts - ch[i].ts > GAP_THRESHOLD) {
        out.push({ ts: ch[i].ts + 1, cpu: null, gpu: null, dram: null, other: null });
        out.push({ ts: ch[i + 1].ts - 1, cpu: null, gpu: null, dram: null, other: null });
      }
    }
    return out;
  }, [data?.componentHistory, GAP_THRESHOLD]);

  // ── Week view: per-day slices for squircle mini-charts ────────────
  const daySlices = useMemo(() => {
    if (viewMode !== 'week') return [] as Array<{ day: Date; dayStart: number; dayEnd: number; pts: HistoryPoint[]; hasData: boolean }>;
    return Array.from({ length: 7 }, (_, i) => {
      const day = addDays(weekStart, i);
      day.setHours(0, 0, 0, 0);
      const dayStart = Math.floor(day.getTime() / 1000);
      const dayEnd = dayStart + 86399;
      const pts = history.filter((p) => p.ts >= dayStart && p.ts <= dayEnd);
      return { day, dayStart, dayEnd, pts, hasData: pts.length > 0 };
    });
  }, [viewMode, weekStart, history]);

  // Summary stats — derives avg/peak drain from the history series so
  // live (in-progress) sessions count and the numbers scope cleanly to
  // the visible window.
  const stats = useMemo(
    () => computeStats(history, batterySessions, sleepSessions, windowStartTs, windowEndTs),
    [history, batterySessions, sleepSessions, windowStartTs, windowEndTs],
  );

  // Prevent navigating forward past the current rolling window
  const isAtCurrentWeek = useMemo(() => {
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    return weekStart >= addDays(today, -6);
  }, [weekStart]);

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

  // X-axis tick positions — we generate clean hour / day boundaries
  // so the numeric axis has well-placed labels. Driven by the window
  // bounds (not history) so ticks are stable even when history is
  // sparse or the view window extends beyond the first/last data point.
  const xTicks = useMemo(() => {
    const range = windowEndTs - windowStartTs;
    if (range <= 0) return [];
    let interval: number;
    if (range <= 86400) {
      interval = 3600; // every hour for <= 1 day
    } else if (range <= 86400 * 3) {
      interval = 3600 * 6; // every 6h for <= 3 days
    } else {
      interval = 86400; // every day for longer ranges
    }
    const ticks: number[] = [];
    let tick = Math.ceil(windowStartTs / interval) * interval;
    while (tick <= windowEndTs) {
      ticks.push(tick);
      tick += interval;
    }
    return ticks;
  }, [windowStartTs, windowEndTs]);

  // Day boundaries for the week view divider lines.
  const dayBoundaries = useMemo(() => {
    if (viewMode !== 'week') return [] as number[];
    const boundaries: number[] = [];
    for (let i = 1; i <= 6; i++) {
      const d = addDays(weekStart, i);
      d.setHours(0, 0, 0, 0);
      boundaries.push(Math.floor(d.getTime() / 1000));
    }
    return boundaries;
  }, [viewMode, weekStart]);

  // X-axis time formatter based on view mode
  const formatAxisTime = (ts: number) => {
    const d = new Date(ts * 1000);
    if (viewMode === 'day') {
      return d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit', hour12: true });
    }
    return d.toLocaleDateString(undefined, { weekday: 'short', hour: 'numeric', hour12: true });
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

  // ── Cursor overlay (day / custom view only) ───────────────────────
  // Maps pointer position to a timestamp so we can show a tooltip
  // anywhere including inside gap / sleep / charging bands where
  // Recharts' built-in Tooltip snaps to the nearest data point.
  // rAF-throttled: we only call setState once per animation frame so
  // fast mousemove events don't trigger a re-render on every event.
  const chartContainerRef = useRef<HTMLDivElement>(null);
  const rafRef = useRef<number | null>(null);
  const pendingMouseX = useRef<number | null>(null);
  const [hoverTs, setHoverTs] = useState<number | null>(null);
  const [hoverClientX, setHoverClientX] = useState<number | null>(null);

  // Cancel any pending rAF on unmount
  useEffect(() => () => { if (rafRef.current !== null) cancelAnimationFrame(rafRef.current); }, []);

  const handleChartMouseMove = (e: ReactMouseEvent<HTMLDivElement>) => {
    pendingMouseX.current = e.clientX;
    if (rafRef.current !== null) return; // already scheduled
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      const clientX = pendingMouseX.current;
      if (clientX === null) return;
      const container = chartContainerRef.current;
      if (!container || history.length === 0) return;
      const rect = container.getBoundingClientRect();
      const relX = clientX - rect.left;
      const plotLeft = 40 + CHART_MARGIN.left;
      const plotRight = rect.width - CHART_MARGIN.right;
      if (relX < plotLeft || relX > plotRight) {
        setHoverTs(null);
        setHoverClientX(null);
        return;
      }
      const frac = (relX - plotLeft) / Math.max(1, plotRight - plotLeft);
      setHoverTs(Math.round(windowStartTs + frac * (windowEndTs - windowStartTs)));
      setHoverClientX(relX);
    });
  };

  const handleChartMouseLeave = () => {
    if (rafRef.current !== null) { cancelAnimationFrame(rafRef.current); rafRef.current = null; }
    pendingMouseX.current = null;
    setHoverTs(null);
    setHoverClientX(null);
  };

  const hoverInfo = useMemo(() => {
    if (hoverTs == null) return null;
    return computeHoverInfo(
      hoverTs,
      history,
      chargingSessions,
      sleepSessions,
      gaps,
      windowEndTs,
    );
  }, [hoverTs, history, chargingSessions, sleepSessions, gaps, windowEndTs]);

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
                onClick={() => {
                  setWeekStart((prev) => {
                    const next = addDays(prev, 7);
                    const today = new Date();
                    today.setHours(0, 0, 0, 0);
                    const maxStart = addDays(today, -6);
                    return next > maxStart ? maxStart : next;
                  });
                }}
                style={ghostBtnStyle}
                disabled={isAtCurrentWeek}
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
          value={stats.avgDrainW > 0 ? `${stats.avgDrainW.toFixed(1)} W` : '--'}
          context="mean discharge rate"
        />
        <SummaryCard
          label="Fastest discharge"
          value={stats.peakDrainW > 0 ? `${stats.peakDrainW.toFixed(1)} W` : '--'}
          context="peak session drain"
          danger={stats.peakDrainW > 15}
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

      {/* ─── Week view: 7 squircle mini-charts ───────────────────────── */}
      {viewMode === 'week' && (
        <section className="card">
          <div className="card-header">
            <div>
              <div className="card-title">Battery timeline</div>
              <div className="card-subtitle">Click a day to explore it</div>
            </div>
            {loading && <span className="badge" style={{ fontSize: 11 }}>loading...</span>}
          </div>
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(7, 1fr)', gap: 10 }}>
            {daySlices.map(({ day, pts, hasData }, i) => (
              <div
                key={i}
                onClick={() => { setSelectedDay(new Date(day)); setViewMode('day'); }}
                style={{
                  borderRadius: 16,
                  overflow: 'hidden',
                  border: '1px solid var(--border)',
                  background: 'var(--bg-card)',
                  cursor: 'pointer',
                  height: 160,
                  display: 'flex',
                  flexDirection: 'column',
                  transition: 'border-color 0.15s, box-shadow 0.15s',
                }}
                onMouseEnter={(e) => {
                  const el = e.currentTarget as HTMLDivElement;
                  el.style.borderColor = 'var(--accent)';
                  el.style.boxShadow = '0 0 0 1px var(--accent), 0 4px 16px rgba(0,0,0,0.25)';
                }}
                onMouseLeave={(e) => {
                  const el = e.currentTarget as HTMLDivElement;
                  el.style.borderColor = 'var(--border)';
                  el.style.boxShadow = 'none';
                }}
              >
                <div style={{ padding: '8px 10px 4px', fontSize: 11, fontWeight: 600, color: 'var(--text-muted)', flexShrink: 0 }}>
                  {day.toLocaleDateString(undefined, { weekday: 'short', month: 'numeric', day: 'numeric' })}
                </div>
                <div style={{ flex: 1, minHeight: 0 }}>
                  {hasData ? (
                    <ResponsiveContainer width="100%" height="100%">
                      <AreaChart data={pts} margin={{ top: 4, right: 6, left: -30, bottom: 0 }}>
                        <defs>
                          <linearGradient id={`dayFill-${i}`} x1="0" y1="0" x2="0" y2="1">
                            <stop offset="5%" stopColor="var(--accent)" stopOpacity={0.35} />
                            <stop offset="95%" stopColor="var(--accent)" stopOpacity={0.02} />
                          </linearGradient>
                        </defs>
                        <XAxis dataKey="ts" hide />
                        <YAxis domain={[0, 100]} hide />
                        <Area
                          type="monotone"
                          dataKey="percent"
                          stroke="var(--accent)"
                          strokeWidth={1.5}
                          fill={`url(#dayFill-${i})`}
                          dot={false}
                          isAnimationActive={false}
                        />
                      </AreaChart>
                    </ResponsiveContainer>
                  ) : (
                    <div style={{ height: '100%', display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--text-muted)', fontSize: 11 }}>
                      No data
                    </div>
                  )}
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {/* ─── Day / custom: full battery timeline chart ────────────────── */}
      {viewMode !== 'week' && (
      <section className="card">
        <div className="card-header">
          <div>
            <div className="card-title">Battery timeline</div>
            <div className="card-subtitle">
              {viewMode === 'day'
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
        <div
          ref={chartContainerRef}
          onMouseMove={handleChartMouseMove}
          onMouseLeave={handleChartMouseLeave}
          style={{ height: 320, position: 'relative' }}
        >
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
                margin={CHART_MARGIN}
                onClick={handleChartClick}
                style={viewMode === 'week' ? { cursor: 'pointer' } : undefined}
              >
                <defs>
                  <linearGradient id="timelineFill" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stopColor="var(--accent)" stopOpacity={0.45} />
                    <stop offset="100%" stopColor="var(--accent)" stopOpacity={0.02} />
                  </linearGradient>
                  {/* Diagonal hatching for interpolated (no-data) regions */}
                  <pattern
                    id="hatchInterpolated"
                    patternUnits="userSpaceOnUse"
                    width="8"
                    height="8"
                    patternTransform="rotate(45)"
                  >
                    <rect width="8" height="8" fill="rgba(255, 255, 255, 0.03)" />
                    <line
                      x1="0" y1="0" x2="0" y2="8"
                      stroke="var(--text-muted)"
                      strokeWidth="1.5"
                      strokeOpacity="0.25"
                    />
                  </pattern>
                </defs>

                <CartesianGrid
                  stroke="var(--border)"
                  strokeDasharray="3 3"
                  vertical={false}
                />

                {/* Day boundary dividers (week view only) */}
                {dayBoundaries.map((ts) => (
                  <ReferenceLine
                    key={`daybound-${ts}`}
                    x={ts}
                    stroke="var(--border)"
                    strokeDasharray="2 4"
                    strokeOpacity={0.6}
                    ifOverflow="visible"
                  />
                ))}

                {/* Interpolated gap bands (diagonal hatching) — no-data
                    regions that aren't sleep. Rendered first so they sit
                    behind charging / sleep bands. Bands are clamped to the
                    window bounds so sessions crossing the window edge still
                    render their in-window portion. */}
                {interpolatedGaps.map((g, i) => {
                  const band = clampBand(g.startTs, g.endTs, windowStartTs, windowEndTs);
                  if (!band) return null;
                  return (
                    <ReferenceArea
                      key={`interp-${i}`}
                      x1={band[0]}
                      x2={band[1]}
                      fill="url(#hatchInterpolated)"
                      fillOpacity={1}
                      isFront
                    />
                  );
                })}

                {/* Charging session bands (green) */}
                {chargingSessions.map((s, i) => {
                  const band = clampBand(
                    s.startedAt,
                    s.endedAt ?? windowEndTs,
                    windowStartTs,
                    windowEndTs,
                  );
                  if (!band) return null;
                  return (
                    <ReferenceArea
                      key={`charge-${i}`}
                      x1={band[0]}
                      x2={band[1]}
                      fill="rgba(34, 197, 94, 0.2)"
                      fillOpacity={1}
                      isFront
                    />
                  );
                })}

                {/* Sleep session bands (purple) */}
                {sleepSessions.map((s, i) => {
                  const band = clampBand(
                    s.sleepAt,
                    s.wakeAt ?? windowEndTs,
                    windowStartTs,
                    windowEndTs,
                  );
                  if (!band) return null;
                  return (
                    <ReferenceArea
                      key={`sleep-${i}`}
                      x1={band[0]}
                      x2={band[1]}
                      fill="rgba(139, 92, 246, 0.25)"
                      fillOpacity={1}
                      isFront
                    />
                  );
                })}

                {/* Cursor guideline driven by hoverTs state */}
                {hoverTs != null && (
                  <ReferenceLine
                    x={hoverTs}
                    stroke="var(--text-muted)"
                    strokeDasharray="3 3"
                    strokeOpacity={0.8}
                    ifOverflow="visible"
                  />
                )}

                {/* Explicit domain = window bounds. With ['dataMin','dataMax']
                    Recharts would discard ReferenceAreas whose x1/x2 fall
                    outside the actual history points, which broke the day
                    view overlays for sessions crossing midnight. */}
                <XAxis
                  dataKey="ts"
                  type="number"
                  domain={[windowStartTs, windowEndTs]}
                  scale="linear"
                  ticks={xTicks}
                  stroke="var(--text-muted)"
                  fontSize={11}
                  tickLine={false}
                  tickFormatter={formatAxisTime}
                  allowDataOverflow
                />
                <YAxis
                  domain={[0, 100]}
                  stroke="var(--text-muted)"
                  fontSize={11}
                  tickLine={false}
                  tickFormatter={(v) => `${v}%`}
                  width={40}
                />

                <Area
                  type="linear"
                  dataKey="percent"
                  stroke="var(--accent)"
                  strokeWidth={2}
                  fill="url(#timelineFill)"
                  isAnimationActive={false}
                />


              </ComposedChart>
            </ResponsiveContainer>
          )}
          {/* Custom cursor tooltip overlay — works across interpolated,
              sleep, and charging bands (Recharts' built-in Tooltip snaps to
              data points and fails inside gap regions). */}
          {hoverTs != null && hoverInfo && (
            <div
              style={{
                position: 'absolute',
                left: hoverClientX ?? 0,
                top: 8,
                transform: `translateX(${
                  hoverClientX != null && chartContainerRef.current
                    ? hoverClientX > chartContainerRef.current.clientWidth - 180
                      ? '-100%'
                      : '8px'
                    : '8px'
                })`,
                pointerEvents: 'none',
                background: 'var(--bg-raised)',
                border: '1px solid var(--border)',
                borderRadius: 6,
                padding: '8px 10px',
                fontSize: 12,
                lineHeight: 1.5,
                color: 'var(--text)',
                boxShadow: '0 4px 14px rgba(0,0,0,0.25)',
                minWidth: 160,
                zIndex: 5,
              }}
            >
              <div style={{ color: 'var(--text-muted)', fontSize: 11 }}>
                {formatFullTimestamp(hoverTs)}
              </div>
              <div style={{ marginTop: 4, display: 'flex', gap: 6, alignItems: 'center' }}>
                <RegionDot region={hoverInfo.region} />
                <span style={{ fontWeight: 500, textTransform: 'capitalize' }}>
                  {hoverInfo.region}
                </span>
              </div>
              {hoverInfo.percent != null && (
                <div style={{ marginTop: 2 }}>
                  Battery: <b>{hoverInfo.percent.toFixed(1)}%</b>
                </div>
              )}
              {hoverInfo.rateLabel && (
                <div>
                  {hoverInfo.rateEstimated ? 'Est. rate' : 'Rate'}:{' '}
                  <b>{hoverInfo.rateLabel}</b>
                </div>
              )}
            </div>
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
            {interpolatedGaps.length > 0 && (
              <LegendItem color="" label="Interpolated" hatched />
            )}
          </div>
        )}
      </section>
      )}

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
              <AreaChart data={processedComponentHistory} margin={{ top: 10, right: 12, left: -8, bottom: 0 }}>
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
                  <pattern id="compHatch" patternUnits="userSpaceOnUse" width="8" height="8" patternTransform="rotate(45)">
                    <rect width="8" height="8" fill="rgba(255,255,255,0.03)" />
                    <line x1="0" y1="0" x2="0" y2="8" stroke="var(--text-muted)" strokeWidth="1.5" strokeOpacity="0.22" />
                  </pattern>
                </defs>
                <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" vertical={false} />
                <XAxis
                  dataKey="ts"
                  type="number"
                  domain={[windowStartTs, windowEndTs]}
                  scale="linear"
                  ticks={xTicks}
                  tickFormatter={formatAxisTime}
                  stroke="var(--text-muted)"
                  fontSize={11}
                  tickLine={false}
                  allowDataOverflow
                />
                <YAxis unit=" W" stroke="var(--text-muted)" fontSize={11} tickLine={false} width={48} />
                <Tooltip
                  contentStyle={tooltipStyle}
                  labelFormatter={(ts: number) => new Date(ts * 1000).toLocaleString()}
                  formatter={(v: number, name: string) => [`${v != null ? v.toFixed(2) : '--'} W`, name.toUpperCase()]}
                />
                <Area type="monotone" dataKey="cpu" stackId="1" stroke="var(--chart-1)" strokeWidth={1.5} fill="url(#sess-fill-cpu)" isAnimationActive={false} connectNulls={false} name="CPU" />
                <Area type="monotone" dataKey="gpu" stackId="1" stroke="var(--chart-4)" strokeWidth={1.5} fill="url(#sess-fill-gpu)" isAnimationActive={false} connectNulls={false} name="GPU" />
                <Area type="monotone" dataKey="dram" stackId="1" stroke="var(--chart-5)" strokeWidth={1.5} fill="url(#sess-fill-dram)" isAnimationActive={false} connectNulls={false} name="DRAM" />
                <Area type="monotone" dataKey="other" stackId="1" stroke="var(--chart-2)" strokeWidth={1.5} fill="url(#sess-fill-other)" isAnimationActive={false} connectNulls={false} name="Other" />
                {componentNoDataRegions.map((r, i) => {
                  const band = clampBand(r.startTs, r.endTs, windowStartTs, windowEndTs);
                  if (!band) return null;
                  return <ReferenceArea key={`cnd-${i}`} x1={band[0]} x2={band[1]} fill="url(#compHatch)" fillOpacity={1} isFront />;
                })}
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
            <div className="card-subtitle">Share of battery usage for this period</div>
          </div>
        </div>
        {(() => {
          const appSummary = data?.appPowerSummary ?? [];
          const totalEnergy = appSummary.reduce((sum, a) => sum + a.totalEnergy, 0);
          if (appSummary.length === 0) {
            return (
              <p className="stat-context" style={{ padding: '12px 0' }}>
                No app power data for this period
              </p>
            );
          }
          return (
            <div>
              {appSummary.map((app) => {
                const pct = totalEnergy > 0 ? (app.totalEnergy / totalEnergy) * 100 : 0;
                return (
                  <div
                    key={app.name}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      padding: '10px 0',
                      borderBottom: '1px solid var(--border)',
                      gap: 10,
                    }}
                  >
                    <span
                      style={{
                        width: 8,
                        height: 8,
                        borderRadius: '50%',
                        flexShrink: 0,
                        background:
                          pct > 30
                            ? 'var(--bad)'
                            : pct > 15
                              ? 'var(--warn)'
                              : 'var(--ok)',
                      }}
                    />
                    <span style={{ fontWeight: 500, fontSize: 14, flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{app.name}</span>
                    <div style={{ width: 100, height: 6, borderRadius: 3, background: 'var(--bg-inset)', flexShrink: 0 }}>
                      <div style={{ width: `${Math.min(pct, 100)}%`, height: '100%', borderRadius: 3, background: pct > 30 ? 'var(--bad)' : pct > 15 ? 'var(--warn)' : 'var(--accent)' }} />
                    </div>
                    <span style={{ fontWeight: 600, fontSize: 14, fontVariantNumeric: 'tabular-nums', minWidth: 48, textAlign: 'right' }}>
                      {pct.toFixed(1)}%
                    </span>
                  </div>
                );
              })}
            </div>
          );
        })()}
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
  hatched,
}: {
  color: string;
  label: string;
  dot?: boolean;
  hatched?: boolean;
}) {
  if (hatched) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <svg width={16} height={8} style={{ flexShrink: 0 }}>
          <defs>
            <pattern
              id="legendHatch"
              patternUnits="userSpaceOnUse"
              width="4"
              height="4"
              patternTransform="rotate(45)"
            >
              <line x1="0" y1="0" x2="0" y2="4" stroke="currentColor" strokeWidth="1" opacity="0.5" />
            </pattern>
          </defs>
          <rect width={16} height={8} rx={2} fill="url(#legendHatch)" stroke="currentColor" strokeWidth="0.5" strokeOpacity="0.3" />
        </svg>
        <span>{label}</span>
      </div>
    );
  }
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

interface GapRegion {
  startTs: number;
  endTs: number;
  type: 'sleep' | 'interpolated';
}

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

// Region shown in the custom cursor tooltip.
type HoverRegion = 'charging' | 'sleep' | 'interpolated' | 'discharge' | 'idle';

interface HoverInfo {
  region: HoverRegion;
  percent: number | null;
  rateLabel: string | null;
  rateEstimated: boolean;
}

// Recharts chart margins. Kept as a module-scoped constant so the cursor
// overlay math stays in sync with what Recharts sees.
const CHART_MARGIN = { top: 10, right: 12, left: -8, bottom: 0 } as const;

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

// Compute summary stats for the current window.
//
// `avgDrainW` and `peakDrainW` are derived from the per-tick `history`
// series rather than from `BatterySession.avgDrainW`. Sessions that are
// still in progress have `avgDrainW === null` (the backend only writes
// it on session close), so the rolling 7-day window frequently left
// both cards reading "--" when the user's only discharge session was
// the one currently in progress. Deriving from `history` also correctly
// scopes the numbers to the visible window.
//
// `timeOnBatterySec` clamps each discharge session's duration to the
// window bounds so sessions that cross the window boundary only count
// their in-window portion.
function computeStats(
  history: HistoryPoint[],
  batterySessions: BatterySession[],
  sleepSessions: SleepSession[],
  windowStartTs: number,
  windowEndTs: number,
): SummaryStats {
  const now = Math.floor(Date.now() / 1000);

  let timeOnBatterySec = 0;
  for (const s of batterySessions) {
    if (s.onAc) continue;
    const start = Math.max(s.startedAt, windowStartTs);
    const end = Math.min(s.endedAt ?? now, windowEndTs);
    if (end > start) timeOnBatterySec += end - start;
  }

  // Time-weighted mean of history.rateW for discharging samples.
  let weightedSum = 0;
  let weightTotal = 0;
  let minRate = 0;
  for (let i = 0; i < history.length - 1; i++) {
    const a = history[i];
    const b = history[i + 1];
    const dt = b.ts - a.ts;
    if (dt <= 0 || dt > STATS_GAP_THRESHOLD) continue;
    if (!Number.isFinite(a.rateW)) continue;
    if (a.rateW < 0) {
      weightedSum += a.rateW * dt;
      weightTotal += dt;
      if (a.rateW < minRate) minRate = a.rateW;
    }
  }
  // Include the final point for peak detection (the loop above only
  // inspects history[0..length-2]).
  if (history.length > 0) {
    const last = history[history.length - 1];
    if (Number.isFinite(last.rateW) && last.rateW < minRate) {
      minRate = last.rateW;
    }
  }

  const avgDrainW = weightTotal > 0 ? Math.abs(weightedSum / weightTotal) : 0;
  const peakDrainW = minRate < 0 ? Math.abs(minRate) : 0;

  let totalSleepDrainMwh = 0;
  for (const s of sleepSessions) {
    if (s.drainMwh != null) {
      totalSleepDrainMwh += Math.abs(s.drainMwh);
    }
  }

  return { timeOnBatterySec, avgDrainW, peakDrainW, totalSleepDrainMwh };
}

// Gap threshold in seconds — matches the component's local constant
// so a long polling gap doesn't poison the time-weighted average.
const STATS_GAP_THRESHOLD = 300;

// Clamp a band's [a, b] interval to [lo, hi]. Returns null if the
// clamped interval has zero or negative length.
function clampBand(
  a: number,
  b: number,
  lo: number,
  hi: number,
): [number, number] | null {
  const x1 = Math.max(a, lo);
  const x2 = Math.min(b, hi);
  return x2 > x1 ? [x1, x2] : null;
}

// Linearly interpolate battery percent and rate at a given ts between
// adjacent history points. Returns null if ts is outside the history
// range (caller falls back to region-based estimates).
function interpolateHistoryAt(
  history: HistoryPoint[],
  ts: number,
): { percent: number; rateW: number } | null {
  if (history.length === 0) return null;
  if (ts <= history[0].ts) {
    return { percent: history[0].percent, rateW: history[0].rateW };
  }
  if (ts >= history[history.length - 1].ts) {
    const last = history[history.length - 1];
    return { percent: last.percent, rateW: last.rateW };
  }
  // Binary search for the right-side index.
  let lo = 0;
  let hi = history.length - 1;
  while (lo + 1 < hi) {
    const mid = (lo + hi) >> 1;
    if (history[mid].ts <= ts) lo = mid;
    else hi = mid;
  }
  const a = history[lo];
  const b = history[hi];
  const span = b.ts - a.ts;
  if (span <= 0) return { percent: a.percent, rateW: a.rateW };
  const frac = (ts - a.ts) / span;
  return {
    percent: a.percent + (b.percent - a.percent) * frac,
    rateW: a.rateW + (b.rateW - a.rateW) * frac,
  };
}

// Classify which "kind" of timeline region the cursor is hovering over.
// Priority: sleep > interpolated > charging > (rateW-based) discharge/idle.
function classifyRegion(
  ts: number,
  chargingSessions: BatterySession[],
  sleepSessions: SleepSession[],
  gaps: GapRegion[],
  interpolated: { percent: number; rateW: number } | null,
  windowEndTs: number,
): HoverRegion {
  for (const s of sleepSessions) {
    const wake = s.wakeAt ?? windowEndTs;
    if (s.sleepAt <= ts && ts <= wake) return 'sleep';
  }
  for (const g of gaps) {
    if (g.type === 'interpolated' && g.startTs <= ts && ts <= g.endTs) {
      return 'interpolated';
    }
  }
  for (const s of chargingSessions) {
    const end = s.endedAt ?? windowEndTs;
    if (s.startedAt <= ts && ts <= end) return 'charging';
  }
  if (interpolated && interpolated.rateW < -0.1) return 'discharge';
  return 'idle';
}

// Compute everything the cursor tooltip needs to render for a given ts.
function computeHoverInfo(
  ts: number,
  history: HistoryPoint[],
  chargingSessions: BatterySession[],
  sleepSessions: SleepSession[],
  gaps: GapRegion[],
  windowEndTs: number,
): HoverInfo {
  const interp = interpolateHistoryAt(history, ts);
  const region = classifyRegion(ts, chargingSessions, sleepSessions, gaps, interp, windowEndTs);

  let percent: number | null = interp ? interp.percent : null;
  let rateLabel: string | null = null;
  let rateEstimated = false;

  if (region === 'sleep') {
    // Prefer the SleepSession's recorded drain rate if the cursor lands
    // inside a known sleep. Fall back to the percent-delta estimate from
    // the enclosing gap, if any.
    const sleep = sleepSessions.find((s) => {
      const wake = s.wakeAt ?? windowEndTs;
      return s.sleepAt <= ts && ts <= wake;
    });
    if (sleep && sleep.drainRateMw != null && Number.isFinite(sleep.drainRateMw)) {
      rateLabel = `${(sleep.drainRateMw / 1000).toFixed(2)} W`;
      rateEstimated = true;
    } else {
      const gap = gaps.find((g) => g.startTs <= ts && ts <= g.endTs);
      if (gap) {
        const est = estimatePercentPerHour(history, gap.startTs, gap.endTs);
        if (est != null) {
          rateLabel = `${est.toFixed(2)} %/h`;
          rateEstimated = true;
        }
      }
    }
  } else if (region === 'interpolated') {
    const gap = gaps.find((g) => g.startTs <= ts && ts <= g.endTs);
    if (gap) {
      const est = estimatePercentPerHour(history, gap.startTs, gap.endTs);
      if (est != null) {
        rateLabel = `${est.toFixed(2)} %/h`;
        rateEstimated = true;
      }
    }
  } else if (interp) {
    rateLabel = `${interp.rateW.toFixed(2)} W`;
  }

  return { region, percent, rateLabel, rateEstimated };
}

// Estimate percent-per-hour across a gap by reading the history percent
// at the gap boundaries. Returns null if we can't bracket the gap.
function estimatePercentPerHour(
  history: HistoryPoint[],
  startTs: number,
  endTs: number,
): number | null {
  const a = interpolateHistoryAt(history, startTs);
  const b = interpolateHistoryAt(history, endTs);
  if (!a || !b) return null;
  const dtHours = (endTs - startTs) / 3600;
  if (dtHours <= 0) return null;
  return (b.percent - a.percent) / dtHours;
}

function formatFullTimestamp(ts: number): string {
  return new Date(ts * 1000).toLocaleString(undefined, {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
    hour12: true,
  });
}

function RegionDot({ region }: { region: HoverRegion }) {
  const colors: Record<HoverRegion, string> = {
    charging: 'rgb(34, 197, 94)',
    sleep: 'rgb(139, 92, 246)',
    interpolated: 'var(--text-muted)',
    discharge: 'var(--accent)',
    idle: 'var(--text-subtle)',
  };
  return (
    <span
      style={{
        width: 8,
        height: 8,
        borderRadius: '50%',
        background: colors[region],
        display: 'inline-block',
        flexShrink: 0,
      }}
    />
  );
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
