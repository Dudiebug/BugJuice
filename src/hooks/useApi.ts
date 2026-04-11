import { useEffect, useRef, useState } from 'react';

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

  // Keep the latest loader in a ref so the effect always calls the
  // current version without needing it in the dependency array.
  const loaderRef = useRef(loader);
  loaderRef.current = loader;

  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const v = await loaderRef.current();
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
  }, [intervalMs]);

  return value;
}
