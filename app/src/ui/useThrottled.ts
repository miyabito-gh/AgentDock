import { useEffect, useRef, useState } from "react";

/** 表示更新の間引き（最小間隔 ms）。`bypass` の値が変わったとき（承認・質問の増減など）は即時に反映する。 */
export function useThrottled<T>(value: T, ms: number, bypass: string): T {
  const [out, setOut] = useState(value);
  const last = useRef(0);
  const lastBypass = useRef(bypass);
  useEffect(() => {
    const wait = ms - (Date.now() - last.current);
    if (bypass !== lastBypass.current || wait <= 0) {
      lastBypass.current = bypass; last.current = Date.now(); setOut(value); return;
    }
    const t = window.setTimeout(() => { last.current = Date.now(); setOut(value); }, wait);
    return () => window.clearTimeout(t);
  }, [value, ms, bypass]);
  return out;
}
