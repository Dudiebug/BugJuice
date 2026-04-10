import { useEffect, useState } from 'react';

/**
 * Return a monotonically-incrementing integer that bumps every `intervalMs`
 * milliseconds. Components that depend on it will re-render on the tick,
 * which is the simplest way to pull fresh mock data without setting up a
 * state store.
 */
export function useTick(intervalMs: number = 2000): number {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => setTick((t) => t + 1), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return tick;
}
