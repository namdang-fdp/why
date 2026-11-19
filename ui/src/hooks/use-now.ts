import { useEffect, useState } from "react";

/** The current time in Unix seconds, refreshed every `everyMs` (for "2m ago" labels). */
export function useNowSeconds(everyMs = 30_000) {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), everyMs);
    return () => clearInterval(timer);
  }, [everyMs]);
  return now;
}
