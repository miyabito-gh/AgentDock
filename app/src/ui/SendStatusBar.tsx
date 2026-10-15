import { useEffect, useRef, useState } from "react";
import { formatElapsed, sendStatusLines, slowNote, type StatusInput } from "./sendStatus";

/** 入力欄の近くの段階表示。待っている間は消えず（スピナー＋段階の文言＋経過時間）、結果が分かった段階に切り替わる。
 *  経過時間は、送信の段階は送信した時刻から、それ以外はその段階の表示を始めた時刻から数える。 */
export function SendStatusBar({ input, compact = false }: { input: StatusInput; compact?: boolean }) {
  const lines = sendStatusLines(input);
  const seen = useRef(new Map<string, number>());
  const [now, setNow] = useState(() => Date.now());
  const ticking = lines.some((l) => l.spin);
  useEffect(() => {
    if (!ticking) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [ticking]);
  // 表示が消えた段階の起点は忘れる（次に同じ段階が来たら数え直す）
  const live = new Set(lines.map((l) => l.id));
  for (const k of [...seen.current.keys()]) if (!live.has(k)) seen.current.delete(k);
  for (const l of lines) if (!seen.current.has(l.id)) seen.current.set(l.id, Date.now());
  if (!lines.length) return null;
  return (
    <div className={`sendstat ${compact ? "compact" : ""}`}>
      {lines.map((l) => {
        const from = l.since ?? seen.current.get(l.id) ?? now;
        const elapsed = Math.max(0, now - from);
        const slow = slowNote(l.id, elapsed);
        return (
          <div key={l.id} className={`pstat ${l.tone === "warn" ? "warn" : ""}`} role={l.tone === "warn" ? "alert" : "status"} data-stage={l.id.split(":")[0]}>
            {l.spin ? <span className="spinner" aria-hidden="true" /> : null}
            <b>{l.text}</b>
            {l.spin ? <span className="muted">　{formatElapsed(elapsed)}</span> : null}
            {slow ? <span className="muted">　{slow}</span> : null}
          </div>
        );
      })}
    </div>
  );
}
