import { useId, useState } from "react";
import type { Known, SpawnSpec } from "../ipc/types";
import { modelBadgeText, modelTooltip } from "./modelLabel";

/** カードの名前の行に置く短いモデル表示（記録値）。常に表示し、全文はホバー／フォーカスのツールチップ（キーボードで届く）。
 *  取れていないときは「モデル未取得」と出す（空や既定値で代用しない）。幅が足りなければ省略記号で切る。
 *  `root` は親（メイン）のカード。ツールチップに親の記録値であることを添える。 */
export function ModelBadge({ model, effort, spawn, root = false }: { model: Known<string>; effort: Known<string>; spawn?: Known<SpawnSpec>; root?: boolean }) {
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const id = useId();
  const badge = modelBadgeText(model, effort);
  const tip = modelTooltip(model, effort, spawn, root);
  const show = (el: HTMLElement) => { const r = el.getBoundingClientRect(); setPos({ x: Math.max(4, Math.min(r.left, window.innerWidth - 300)), y: r.bottom + 4 }); };
  return (
    <span className="mdl" tabIndex={0} role="note" aria-label={tip} aria-describedby={pos ? id : undefined}
      onMouseEnter={(e) => show(e.currentTarget)} onMouseLeave={() => setPos(null)}
      onFocus={(e) => show(e.currentTarget)} onBlur={() => setPos(null)}
      onKeyDown={(e) => { if (e.key === "Escape" && pos) { e.stopPropagation(); setPos(null); } }}>
      {badge}
      {pos ? <span id={id} role="tooltip" className="mdl-tip" style={{ left: pos.x, top: pos.y }}>{tip}</span> : null}
    </span>
  );
}
