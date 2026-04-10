import { useEffect, useState } from 'react';

/**
 * Run an async loader on a polling interval and return the latest value.
 * Initial value is `null` until the first response arrives.
 *
 * Usage:
 *   const status = useApi(() => getBatteryStatus(), 2000);
 *   if (!status) return <Skeleton />;
 */
export function useApi<T>(
  loader: () => Promise<T>,
  intervalMs: number = 2000,
): T | null {
  const [value, setValue] = useState<T | null>(null);

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const v = await loader();
        if (!cancelled) setValue(v);
      } catch (e) {
        if (!cancelled) console.error('useApi loader failed:', e);
      }
    };
    tick();
    const id = setInterval(tick, intervalMs);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intervalMs]);

  return value;
}
