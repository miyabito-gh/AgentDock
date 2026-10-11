import { useId, useState } from "react";
import type { Known } from "../ipc/types";
import { modelBadge, modelTooltip } from "./modelLabel";

/** 子カードの名前の行に置く短いモデル表示（記録値）。全文はホバー／フォーカスのツールチップ（キーボードで届く）。
 *  取れていないときは印を出さず、見えない代わりに名前の行の高さも変えない（ツールチップは親の `title` 側で補う）。
 *  `compact`（コンパクト監視窓）は印を省いて「i」だけ出し、全文はツールチップで読める。 */
export function ModelBadge({ model, effort, compact }: { model: Known<string>; effort: Known<string>; compact: boolean }) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const id = useId();
  const badge = modelBadge(model, effort);
  if (badge === null) return null;
  const tip = modelTooltip(model, effort);
  const show = (el: HTMLElement) => { const r = el.getBoundingClientRect(); setPos({ x: Math.max(4, Math.min(r.left, window.innerWidth - 300)), y: r.bottom + 4 }); };
  return (
    <span className="mdl" tabIndex={0} role="note" aria-label={tip} aria-describedby={pos ? id : undefined}
      onMouseEnter={(e) => show(e.currentTarget)} onMouseLeave={() => setPos(null)}
      onFocus={(e) => show(e.currentTarget)} onBlur={() => setPos(null)}
      onKeyDown={(e) => { if (e.key === "Escape" && pos) { e.stopPropagation(); setPos(null); } }}>
      {compact ? "i" : badge}
      {pos ? <span id={id} role="tooltip" className="mdl-tip" style={{ left: pos.x, top: pos.y }}>{tip}</span> : null}
    </span>
  );
}
