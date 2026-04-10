import { useEffect, useMemo, useState } from 'react';
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
  getBatterySessions,
  getSessionDetail,
  getSleepDetail,
  getSleepSessions,
} from '@/api';
import { useApi } from '@/hooks/useApi';
import { mock } from '@/mock';
import type { BatterySession, SleepSession } from '@/types';

type Tab = 'battery' | 'sleep';

export function Sessions() {
  const [tab, setTab] = useState<Tab>('battery');
  const sessionsData = useApi(getBatterySessions, 10_000);
  const sleepsData = useApi(getSleepSessions, 10_000);
  const sessions = sessionsData ?? [];
  const sleeps = sleepsData ?? [];
  const [openId, setOpenId] = useState<number | null>(null);
  // When data first arrives, open the most recent session.
  useEffect(() => {
    if (openId === null && sessions.length > 0 && tab === 'battery') {
      setOpenId(sessions[0].id);
    }
  }, [sessions, tab, openId]);
  useEffect(() => {
    if (openId === null && sleeps.length > 0 && tab === 'sleep') {
      setOpenId(sleeps[0].id);
    }
  }, [sleeps, tab, openId]);

  return (
    <div className="page">
      <header className="page-header">
        <h1 className="page-title">Sessions</h1>
        <p className="page-subtitle">
          Click any row to see full details, charts, and per-session stats
        </p>
      </header>

      <div style={{ display: 'flex', gap: 4, borderBottom: '1px solid var(--border)' }}>
        <TabButton
          active={tab === 'battery'}
          onClick={() => {
            setTab('battery');
            setOpenId(sessions[0]?.id ?? null);
          }}
          label="Battery sessions"
          count={sessions.length}
        />
        <TabButton
          active={tab === 'sleep'}
          onClick={() => {
            setTab('sleep');
            setOpenId(sleeps[0]?.id ?? null);
          }}
          label="Sleep sessions"
          count={sleeps.length}
        />
      </div>

      {tab === 'battery' ? (
        <BatterySessionsList
          sessions={sessions}
          openId={openId}
          setOpenId={setOpenId}
        />
      ) : (
        <SleepSessionsList sleeps={sleeps} openId={openId} setOpenId={setOpenId} />
      )}
    </div>
  );
}

function TabButton({
  active,
  onClick,
  label,
  count,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  count: number;
}) {
  return (
    <button
      onClick={onClick}
      style={{
        padding: '12px 20px',
        background: 'transparent',
        color: active ? 'var(--accent)' : 'var(--text-subtle)',
        fontSize: 14,
        fontWeight: 500,
        borderBottom: active ? '2px solid var(--accent)' : '2px solid transparent',
        marginBottom: -1,
      }}
    >
      {label}
      <span
        style={{
          marginLeft: 8,
          padding: '2px 8px',
          borderRadius: 999,
          fontSize: 11,
          background: active ? 'var(--accent-soft)' : 'var(--bg-inset)',
          color: active ? 'var(--accent)' : 'var(--text-muted)',
        }}
      >
        {count}
      </span>
    </button>
  );
}

// ─── BATTERY SESSIONS ────────────────────────────────────────────────

function BatterySessionsList({
  sessions,
  openId,
  setOpenId,
}: {
  sessions: BatterySession[];
  openId: number | null;
  setOpenId: (id: number | null) => void;
}) {
  return (
    <section className="card" style={{ padding: 0 }}>
      {sessions.map((s) => (
        <BatterySessionRow
          key={s.id}
          session={s}
          open={openId === s.id}
          onToggle={() => setOpenId(openId === s.id ? null : s.id)}
        />
      ))}
    </section>
  );
}

function BatterySessionRow({
  session,
  open,
  onToggle,
}: {
  session: BatterySession;
  open: boolean;
  onToggle: () => void;
}) {
  // Async load — only fetch when this row is open.
  const [detail, setDetail] = useState<ReturnType<typeof mock.getBatterySessionDetail> | null>(null);
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    getSessionDetail(session.id).then((d) => {
      if (!cancelled) setDetail(d);
    });
    return () => {
      cancelled = true;
    };
  }, [session.id, open]);

  // Fall back to a quick mock detail for the inline sparkline so the row
  // shows something even when not open.
  const sparkDetail = useMemo(
    () => detail ?? mock.getBatterySessionDetail(session),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [session.id, detail],
  );
  const isOpen = session.endedAt === null;
  const durLabel = humanDuration(sparkDetail.durationSec);
  const dateLabel = formatDateRange(session.startedAt, session.endedAt);
  const timeAgoLabel = formatTimeAgo(session.startedAt);

  // Sparkline preview for closed rows: 20-point downsample
  const spark = useMemo(() => {
    const stride = Math.max(1, Math.floor(sparkDetail.history.length / 20));
    return sparkDetail.history.filter((_, i) => i % stride === 0);
  }, [sparkDetail.history]);

  return (
    <div style={{ borderBottom: '1px solid var(--border)' }}>
      <button
        onClick={onToggle}
        style={{
          width: '100%',
          display: 'grid',
          gridTemplateColumns: '14px 1fr 130px 110px 130px 24px',
          alignItems: 'center',
          gap: 16,
          padding: '18px 24px',
          background: open ? 'var(--bg-inset)' : 'transparent',
          textAlign: 'left',
          fontSize: 14,
          transition: 'background var(--dur-fast) var(--ease-out)',
        }}
      >
        <span
          style={{
            width: 10,
            height: 10,
            borderRadius: '50%',
            background: session.onAc ? 'var(--accent)' : '#f59e0b',
            boxShadow: isOpen ? '0 0 0 4px var(--accent-soft)' : 'none',
          }}
        />
        <div style={{ minWidth: 0 }}>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              fontWeight: 500,
            }}
          >
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
          <div
            style={{
              fontSize: 12,
              color: 'var(--text-muted)',
              marginTop: 2,
            }}
          >
            {timeAgoLabel} · {session.onAc ? 'on AC power' : 'on battery'} · {durLabel}
          </div>
        </div>
        <div style={{ color: 'var(--text-subtle)', fontSize: 13 }}>
          {session.startPercent.toFixed(0)}%
          <span style={{ color: 'var(--text-muted)' }}> → </span>
          {session.endPercent !== null
            ? `${session.endPercent.toFixed(0)}%`
            : '…'}
        </div>
        <div
          style={{
            color:
              session.avgDrainW && session.avgDrainW < 0
                ? '#ef4444'
                : 'var(--accent)',
            fontVariantNumeric: 'tabular-nums',
            fontWeight: 600,
          }}
        >
          {session.avgDrainW
            ? `${session.avgDrainW > 0 ? '+' : ''}${session.avgDrainW.toFixed(1)} W`
            : '—'}
        </div>
        {/* Inline sparkline */}
        <div style={{ height: 28 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart data={spark} margin={{ top: 0, right: 0, left: 0, bottom: 0 }}>
              <Area
                type="monotone"
                dataKey="percent"
                stroke="var(--accent)"
                strokeWidth={1.5}
                fill="var(--accent-soft)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
        <span style={{ color: 'var(--text-muted)' }}>{open ? '▾' : '▸'}</span>
      </button>

      {open && detail && <BatterySessionDetailView session={session} detail={detail} />}
      {open && !detail && (
        <div style={{ padding: '0 24px 20px 48px', color: 'var(--text-muted)', fontSize: 13 }}>
          loading session detail…
        </div>
      )}
    </div>
  );
}

function BatterySessionDetailView({
  session,
  detail,
}: {
  session: BatterySession;
  detail: ReturnType<typeof mock.getBatterySessionDetail>;
}) {
  return (
    <div style={{ padding: '0 24px 24px 48px' }}>
      {/* Stat strip */}
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(4, 1fr)',
          gap: 16,
          marginBottom: 20,
          paddingTop: 4,
        }}
      >
        <DetailStat
          label="Peak draw"
          value={`${Math.abs(detail.minRateW).toFixed(1)} W`}
          context={session.onAc ? '(slowest charge)' : '(highest discharge)'}
          danger={!session.onAc}
        />
        <DetailStat
          label="Lowest draw"
          value={`${Math.abs(detail.maxRateW).toFixed(1)} W`}
          context={session.onAc ? '(fastest charge)' : '(idle moments)'}
        />
        <DetailStat
          label="Average"
          value={`${Math.abs(detail.avgRateW).toFixed(2)} W`}
          context="across the session"
        />
        <DetailStat
          label="Energy moved"
          value={`${(detail.totalEnergyMwh / 1000).toFixed(2)} Wh`}
          context={session.onAc ? 'into battery' : 'out of battery'}
        />
      </div>

      {/* Battery percentage chart */}
      <div className="card" style={{ background: 'var(--bg-card)' }}>
        <div className="card-header">
          <div>
            <div className="card-title">Battery percentage</div>
            <div className="card-subtitle">
              {formatDateTime(session.startedAt)}
              {session.endedAt && ` → ${formatDateTime(session.endedAt)}`}
            </div>
          </div>
        </div>
        <div style={{ height: 200 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart
              data={detail.history}
              margin={{ top: 8, right: 12, left: -8, bottom: 0 }}
            >
              <defs>
                <linearGradient id="sessionFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--accent)" stopOpacity={0.45} />
                  <stop offset="100%" stopColor="var(--accent)" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
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
                contentStyle={tooltipStyle}
                labelFormatter={(ts: number) =>
                  new Date(ts * 1000).toLocaleString()
                }
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
                fill="url(#sessionFill)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </div>

      {/* Wattage chart */}
      <div className="card" style={{ background: 'var(--bg-card)', marginTop: 16 }}>
        <div className="card-header">
          <div>
            <div className="card-title">Power rate</div>
            <div className="card-subtitle">
              {session.onAc ? 'Charging wattage' : 'Discharge wattage'} over time
            </div>
          </div>
        </div>
        <div style={{ height: 180 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart
              data={detail.history}
              margin={{ top: 8, right: 12, left: -8, bottom: 0 }}
            >
              <defs>
                <linearGradient id="rateFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--chart-2)" stopOpacity={0.4} />
                  <stop offset="100%" stopColor="var(--chart-2)" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
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
                tickFormatter={(v) => `${Math.abs(v).toFixed(0)} W`}
                width={50}
              />
              <Tooltip
                contentStyle={tooltipStyle}
                formatter={(v: number) => [`${v.toFixed(2)} W`, 'Rate']}
                labelFormatter={(ts: number) =>
                  new Date(ts * 1000).toLocaleString()
                }
              />
              <Area
                type="monotone"
                dataKey="rateW"
                stroke="var(--chart-2)"
                strokeWidth={1.8}
                fill="url(#rateFill)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </div>
    </div>
  );
}

// ─── SLEEP SESSIONS ──────────────────────────────────────────────────

function SleepSessionsList({
  sleeps,
  openId,
  setOpenId,
}: {
  sleeps: SleepSession[];
  openId: number | null;
  setOpenId: (id: number | null) => void;
}) {
  return (
    <section className="card" style={{ padding: 0 }}>
      {sleeps.map((s) => (
        <SleepSessionRow
          key={s.id}
          sleep={s}
          open={openId === s.id}
          onToggle={() => setOpenId(openId === s.id ? null : s.id)}
        />
      ))}
    </section>
  );
}

function SleepSessionRow({
  sleep,
  open,
  onToggle,
}: {
  sleep: SleepSession;
  open: boolean;
  onToggle: () => void;
}) {
  const [detail, setDetail] = useState<ReturnType<typeof mock.getSleepSessionDetail> | null>(null);
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    getSleepDetail(sleep.id).then((d) => {
      if (!cancelled) setDetail(d);
    });
    return () => {
      cancelled = true;
    };
  }, [sleep.id, open]);
  const dur =
    sleep.wakeAt !== null ? humanDuration(sleep.wakeAt - sleep.sleepAt) : '—';

  return (
    <div style={{ borderBottom: '1px solid var(--border)' }}>
      <button
        onClick={onToggle}
        style={{
          width: '100%',
          display: 'grid',
          gridTemplateColumns: '120px 1fr 160px 130px 24px',
          alignItems: 'center',
          gap: 16,
          padding: '18px 24px',
          background: open ? 'var(--bg-inset)' : 'transparent',
          textAlign: 'left',
          fontSize: 14,
          transition: 'background var(--dur-fast) var(--ease-out)',
        }}
      >
        <VerdictBadge verdict={sleep.verdict} />
        <div>
          <div style={{ fontWeight: 500 }}>
            {formatDateRange(sleep.sleepAt, sleep.wakeAt)}
          </div>
          <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
            {formatTimeAgo(sleep.sleepAt)} · slept for {dur} · DRIPS{' '}
            {sleep.dripsPercent?.toFixed(0)}%
          </div>
        </div>
        <div style={{ color: 'var(--text-subtle)' }}>
          {sleep.drainMwh?.toLocaleString()} mWh
          <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>
            {sleep.drainPercent?.toFixed(1)}% of battery
          </div>
        </div>
        <div
          style={{
            color: 'var(--text)',
            fontVariantNumeric: 'tabular-nums',
            fontWeight: 600,
          }}
        >
          {sleep.drainRateMw?.toFixed(0)} mW
          <div style={{ fontSize: 11, color: 'var(--text-muted)', fontWeight: 400 }}>
            avg drain rate
          </div>
        </div>
        <span style={{ color: 'var(--text-muted)' }}>{open ? '▾' : '▸'}</span>
      </button>

      {open && detail && <SleepSessionDetailView sleep={sleep} detail={detail} />}
      {open && !detail && (
        <div style={{ padding: '0 24px 24px 48px', color: 'var(--text-muted)', fontSize: 13 }}>
          loading sleep detail…
        </div>
      )}
    </div>
  );
}

function SleepSessionDetailView({
  sleep,
  detail,
}: {
  sleep: SleepSession;
  detail: ReturnType<typeof mock.getSleepSessionDetail>;
}) {
  if (detail.history.length === 0) {
    return (
      <div style={{ padding: '0 24px 24px 48px', color: 'var(--text-muted)' }}>
        No detail data available for this sleep session.
      </div>
    );
  }
  return (
    <div style={{ padding: '0 24px 24px 48px' }}>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'repeat(4, 1fr)',
          gap: 16,
          marginBottom: 20,
          paddingTop: 4,
        }}
      >
        <DetailStat
          label="Peak drain"
          value={`${detail.maxRateMw.toFixed(0)} mW`}
          context="brief background wake"
          danger={detail.maxRateMw > 800}
        />
        <DetailStat
          label="Quietest"
          value={`${detail.minRateMw.toFixed(0)} mW`}
          context="deep sleep low"
        />
        <DetailStat
          label="Average"
          value={`${detail.avgRateMw.toFixed(0)} mW`}
          context={verdictText(sleep.verdict)}
        />
        <DetailStat
          label="Total drained"
          value={`${detail.totalDrainMwh.toLocaleString()} mWh`}
          context={`${humanDuration(detail.durationSec)} of sleep`}
        />
      </div>

      {/* Capacity decline chart */}
      <div className="card" style={{ background: 'var(--bg-card)' }}>
        <div className="card-header">
          <div>
            <div className="card-title">Capacity during sleep</div>
            <div className="card-subtitle">
              {formatDateTime(sleep.sleepAt)}
              {sleep.wakeAt && ` → ${formatDateTime(sleep.wakeAt)}`}
            </div>
          </div>
        </div>
        <div style={{ height: 180 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart
              data={detail.history}
              margin={{ top: 8, right: 12, left: -4, bottom: 0 }}
            >
              <defs>
                <linearGradient id="sleepFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--accent)" stopOpacity={0.4} />
                  <stop offset="100%" stopColor="var(--accent)" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
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
                tickFormatter={(v) => `${(v / 1000).toFixed(1)}k`}
                width={48}
              />
              <Tooltip
                contentStyle={tooltipStyle}
                formatter={(v: number) => [`${v.toLocaleString()} mWh`, 'Capacity']}
                labelFormatter={(ts: number) =>
                  new Date(ts * 1000).toLocaleString()
                }
              />
              <Area
                type="monotone"
                dataKey="capacity"
                stroke="var(--accent)"
                strokeWidth={2}
                fill="url(#sleepFill)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </div>

      {/* Drain rate chart */}
      <div className="card" style={{ background: 'var(--bg-card)', marginTop: 16 }}>
        <div className="card-header">
          <div>
            <div className="card-title">Drain rate</div>
            <div className="card-subtitle">
              Spikes are background wakes (Wi-Fi, sync, mail)
            </div>
          </div>
        </div>
        <div style={{ height: 160 }}>
          <ResponsiveContainer width="100%" height="100%">
            <AreaChart
              data={detail.history}
              margin={{ top: 8, right: 12, left: -8, bottom: 0 }}
            >
              <defs>
                <linearGradient id="drainFill" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="var(--chart-5)" stopOpacity={0.4} />
                  <stop offset="100%" stopColor="var(--chart-5)" stopOpacity={0.02} />
                </linearGradient>
              </defs>
              <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" vertical={false} />
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
                tickFormatter={(v) => `${v.toFixed(0)}`}
                width={45}
              />
              <Tooltip
                contentStyle={tooltipStyle}
                formatter={(v: number) => [`${v.toFixed(0)} mW`, 'Drain']}
              />
              <Area
                type="monotone"
                dataKey="drainRateMw"
                stroke="var(--chart-5)"
                strokeWidth={1.6}
                fill="url(#drainFill)"
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
      </div>
    </div>
  );
}

// ─── Shared sub-components ───────────────────────────────────────────

function DetailStat({
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
    <div
      style={{
        padding: '12px 14px',
        background: 'var(--bg-card)',
        borderRadius: 'var(--radius-sm)',
        border: '1px solid var(--border)',
      }}
    >
      <div className="stat-label">{label}</div>
      <div
        className="stat-value"
        style={{
          fontSize: 20,
          marginTop: 4,
          color: danger ? '#ef4444' : 'var(--text)',
        }}
      >
        {value}
      </div>
      <div className="stat-context" style={{ marginTop: 2, fontSize: 11 }}>
        {context}
      </div>
    </div>
  );
}

function VerdictBadge({ verdict }: { verdict: SleepSession['verdict'] }) {
  const style: React.CSSProperties = {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    padding: '4px 12px',
    borderRadius: 999,
    fontSize: 12,
    fontWeight: 500,
    width: 'fit-content',
  };
  const dot: React.CSSProperties = { width: 6, height: 6, borderRadius: '50%' };
  switch (verdict) {
    case 'excellent':
      return (
        <span
          style={{
            ...style,
            background: 'var(--accent-soft)',
            color: 'var(--accent)',
          }}
        >
          <span style={{ ...dot, background: 'var(--accent)' }} />
          excellent
        </span>
      );
    case 'normal':
      return (
        <span
          style={{
            ...style,
            background: 'var(--bg-inset)',
            color: 'var(--text-subtle)',
          }}
        >
          <span style={{ ...dot, background: 'var(--text-muted)' }} />
          normal
        </span>
      );
    case 'high':
      return (
        <span
          style={{
            ...style,
            background: 'rgba(245, 158, 11, 0.12)',
            color: '#d97706',
          }}
        >
          <span style={{ ...dot, background: '#f59e0b' }} />
          high
        </span>
      );
    case 'very-high':
      return (
        <span
          style={{
            ...style,
            background: 'rgba(239, 68, 68, 0.12)',
            color: '#dc2626',
          }}
        >
          <span style={{ ...dot, background: '#ef4444' }} />
          very high
        </span>
      );
    default:
      return null;
  }
}

// ─── Helpers ─────────────────────────────────────────────────────────

function humanDuration(sec: number): string {
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.round(sec / 60)} min`;
  const h = Math.floor(sec / 3600);
  const m = Math.round((sec % 3600) / 60);
  return m > 0 ? `${h}h ${m}m` : `${h}h`;
}

function formatTimeAgo(unix: number): string {
  const diff = Math.floor(Date.now() / 1000) - unix;
  if (diff < 60) return 'just now';
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

function formatDateTime(unix: number): string {
  const d = new Date(unix * 1000);
  return d.toLocaleString(undefined, {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatDateRange(start: number, end: number | null): string {
  const s = new Date(start * 1000);
  if (end === null) {
    return `${formatShortDateTime(s)} → now`;
  }
  const e = new Date(end * 1000);
  // Same calendar day?
  if (
    s.getFullYear() === e.getFullYear() &&
    s.getMonth() === e.getMonth() &&
    s.getDate() === e.getDate()
  ) {
    return `${formatShortDateTime(s)} → ${formatShortTime(e)}`;
  }
  return `${formatShortDateTime(s)} → ${formatShortDateTime(e)}`;
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

function verdictText(v: SleepSession['verdict']): string {
  switch (v) {
    case 'excellent':
      return 'excellent — Modern Standby working well';
    case 'normal':
      return 'normal for this hardware';
    case 'high':
      return 'high — something may be waking the laptop';
    case 'very-high':
      return 'very high — investigate immediately';
    default:
      return '';
  }
}

const tooltipStyle = {
  background: 'var(--bg-card)',
  border: '1px solid var(--border)',
  borderRadius: 'var(--radius)',
  fontSize: 12,
  color: 'var(--text)',
  boxShadow: 'var(--shadow)',
};
