import { useMemo, useState } from 'react';
import { getTopApps } from '@/api';
import { useApi } from '@/hooks/useApi';

type SortKey = 'total' | 'cpu' | 'gpu' | 'name';

export function Apps() {
  const response = useApi(getTopApps, 2000);
  const rawApps = response?.apps ?? [];
  const totalW = rawApps.reduce((acc, a) => acc + a.totalW, 0);
  const apps = rawApps.map((a) => ({
    ...a,
    iconHint: a.name.charAt(0).toUpperCase(),
    hog: (totalW > 0 && a.totalW / totalW > 0.30) || a.totalW > 3,
  }));

  const [query, setQuery] = useState('');
  const [sortKey, setSortKey] = useState<SortKey>('total');

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const list = q
      ? apps.filter((a) => a.name.toLowerCase().includes(q))
      : apps.slice();
    list.sort((a, b) => {
      switch (sortKey) {
        case 'cpu':
          return b.cpuW - a.cpuW;
        case 'gpu':
          return b.gpuW - a.gpuW;
        case 'name':
          return a.name.localeCompare(b.name);
        default:
          return b.totalW - a.totalW;
      }
    });
    return list;
  }, [apps, query, sortKey]);

  const confidence = response?.confidencePercent ?? 0;
  const hogs = apps.filter((a) => a.hog).length;

  return (
    <div className="page">
      <header className="page-header">
        <h1 className="page-title">Apps</h1>
        <p className="page-subtitle">
          Per-process power consumption · {apps.length} processes active
        </p>
      </header>

      {/* ─── Summary strip ──────────────────────────────────────────── */}
      <section className="grid grid-3">
        <SummaryCard
          label="Total process power"
          value={totalW.toFixed(2)}
          unit="W"
          context="sum of all attributed processes"
        />
        <SummaryCard
          label="Power hogs"
          value={hogs.toString()}
          unit=""
          context="processes drawing > 3 W"
          accent={hogs > 0}
        />
        <SummaryCard
          label="Attribution confidence"
          value={confidence.toFixed(0)}
          unit="%"
          context={confidenceContext(confidence)}
        />
      </section>

      {/* ─── Filter + sort toolbar ──────────────────────────────────── */}
      <section
        className="card"
        style={{ padding: 'var(--space-4) var(--space-5)' }}
      >
        <div
          style={{
            display: 'flex',
            gap: 12,
            alignItems: 'center',
            flexWrap: 'wrap',
          }}
        >
          <input
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter by process name…"
            style={{
              flex: 1,
              minWidth: 240,
              padding: '10px 14px',
              background: 'var(--bg-input)',
              border: '1px solid var(--border)',
              borderRadius: 'var(--radius-sm)',
              outline: 'none',
              fontSize: 14,
            }}
          />
          <div style={{ display: 'flex', gap: 4 }}>
            {(['total', 'cpu', 'gpu', 'name'] as SortKey[]).map((k) => (
              <button
                key={k}
                onClick={() => setSortKey(k)}
                style={{
                  padding: '8px 14px',
                  borderRadius: 'var(--radius-sm)',
                  background:
                    sortKey === k ? 'var(--accent-soft)' : 'var(--bg-inset)',
                  color: sortKey === k ? 'var(--accent)' : 'var(--text-subtle)',
                  border: '1px solid var(--border)',
                  fontSize: 12,
                  fontWeight: sortKey === k ? 600 : 400,
                  textTransform: 'uppercase',
                  letterSpacing: '0.04em',
                }}
              >
                {k === 'total' ? 'Total' : k.toUpperCase()}
              </button>
            ))}
          </div>
        </div>
      </section>

      {/* ─── Process list ───────────────────────────────────────────── */}
      <section className="card" style={{ padding: 0 }}>
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: '40px 1fr 90px 90px 90px 80px',
            padding: '14px 20px',
            borderBottom: '1px solid var(--border)',
            fontSize: 11,
            fontWeight: 600,
            color: 'var(--text-muted)',
            textTransform: 'uppercase',
            letterSpacing: '0.04em',
          }}
        >
          <div></div>
          <div>Process</div>
          <div style={{ textAlign: 'right' }}>CPU W</div>
          <div style={{ textAlign: 'right' }}>GPU W</div>
          <div style={{ textAlign: 'right' }}>Total W</div>
          <div style={{ textAlign: 'right' }}>Flag</div>
        </div>
        <div>
          {filtered.map((app, i) => (
            <AppRow key={app.pid} app={app} rank={i + 1} />
          ))}
          {filtered.length === 0 && (
            <div
              style={{
                padding: '40px 20px',
                textAlign: 'center',
                color: 'var(--text-muted)',
              }}
            >
              {query
                ? `No processes match "${query}"`
                : 'Collecting per-process power data… (first attribution pass can take a few seconds)'}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

// ─── Sub-components ────────────────────────────────────────────────

function SummaryCard({
  label,
  value,
  unit,
  context,
  accent,
}: {
  label: string;
  value: string;
  unit: string;
  context: string;
  accent?: boolean;
}) {
  return (
    <div className="card">
      <div className="stat-label">{label}</div>
      <div
        className="stat-value"
        style={{
          fontSize: 28,
          marginTop: 8,
          color: accent ? 'var(--accent)' : 'var(--text)',
        }}
      >
        {value}
        {unit && <span className="stat-unit">{unit}</span>}
      </div>
      <div className="stat-context" style={{ marginTop: 4 }}>
        {context}
      </div>
    </div>
  );
}

function AppRow({
  app,
  rank,
}: {
  app: {
    pid: number;
    name: string;
    cpuW: number;
    gpuW: number;
    totalW: number;
    iconHint: string;
    hog: boolean;
  };
  rank: number;
}) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: '40px 1fr 90px 90px 90px 80px',
        padding: '12px 20px',
        borderBottom: '1px solid var(--border)',
        alignItems: 'center',
        fontSize: 14,
        transition: 'background var(--dur-fast) var(--ease-out)',
      }}
      onMouseEnter={(e) =>
        (e.currentTarget.style.background = 'var(--bg-inset)')
      }
      onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
    >
      <div
        style={{
          width: 28,
          height: 28,
          borderRadius: 6,
          background: rank <= 3 ? 'var(--accent-soft)' : 'var(--bg-inset)',
          color: rank <= 3 ? 'var(--accent)' : 'var(--text-subtle)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 12,
          fontWeight: 700,
        }}
      >
        {app.iconHint}
      </div>
      <div style={{ minWidth: 0 }}>
        <div
          style={{
            fontWeight: 500,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {app.name}
        </div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>
          pid {app.pid}
        </div>
      </div>
      <div
        style={{
          textAlign: 'right',
          fontVariantNumeric: 'tabular-nums',
          color: app.cpuW > 0.01 ? 'var(--text)' : 'var(--text-muted)',
        }}
      >
        {app.cpuW.toFixed(2)}
      </div>
      <div
        style={{
          textAlign: 'right',
          fontVariantNumeric: 'tabular-nums',
          color: app.gpuW > 0.01 ? 'var(--text)' : 'var(--text-muted)',
        }}
      >
        {app.gpuW > 0.01 ? app.gpuW.toFixed(2) : '—'}
      </div>
      <div
        style={{
          textAlign: 'right',
          fontWeight: 600,
          fontVariantNumeric: 'tabular-nums',
        }}
      >
        {app.totalW.toFixed(2)}
      </div>
      <div style={{ textAlign: 'right' }}>
        {app.hog && <span className="badge badge-warn">hog</span>}
      </div>
    </div>
  );
}

function confidenceContext(pct: number): string {
  if (pct >= 85) return 'accounting for most of system power';
  if (pct >= 70) return 'most processes tracked';
  return 'partial — some power unattributed';
}
