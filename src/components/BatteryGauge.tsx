interface Props {
  percent: number;
  charging?: boolean;
}

export function BatteryGauge({ percent, charging = false }: Props) {
  const clamped = Math.max(0, Math.min(100, percent));
  const fillClass =
    clamped < 10 ? 'battery-gauge-fill bad' : clamped < 20 ? 'battery-gauge-fill warn' : 'battery-gauge-fill';

  return (
    <div className="battery-gauge" aria-label={`Battery ${clamped.toFixed(0)}%`}>
      <div className="battery-gauge-body">
        <div className={fillClass} style={{ width: `${clamped}%` }} />
      </div>
      <div className="battery-gauge-terminal" />
      <div className="battery-gauge-label">
        {charging && <span style={{ marginRight: 6 }}>⚡</span>}
        {clamped.toFixed(0)}%
      </div>
    </div>
  );
}
