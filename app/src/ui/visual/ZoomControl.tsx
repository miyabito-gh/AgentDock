import { Icon } from "../Icon";
import { ZOOM_MAX, ZOOM_MIN, zoomPercent, type ZoomStep } from "./zoom";

/** 表示倍率の操作（−・倍率・＋）。倍率の数字を押すと100%に戻す。100%のときは数字を控えめに表示する。 */
export function ZoomControl({ value, disabled, onStep }: { value: number; disabled?: boolean; onStep: (step: ZoomStep) => void }) {
  const normal = Math.abs(value - 1) < 0.001;
  return <span className="vp-zoom" role="group" aria-label="表示倍率">
    <button type="button" className="btn subtle sm vp-ico" aria-label="縮小" title="縮小（Ctrl＋−／Ctrl＋ホイール）" disabled={disabled || value <= ZOOM_MIN + 0.001} onClick={() => onStep("out")}><Icon name="minus" /></button>
    <button type="button" className={"btn subtle sm vp-zoom-val" + (normal ? " normal" : "")} aria-label={"表示倍率 " + zoomPercent(value) + (normal ? "" : "（押すと100%に戻す）")} title={normal ? "表示倍率（Ctrl＋ホイールで拡大・縮小）" : "100%に戻す（Ctrl＋0）"} disabled={disabled} onClick={() => { if (!normal) onStep("reset"); }}>{zoomPercent(value)}</button>
    <button type="button" className="btn subtle sm vp-ico" aria-label="拡大" title="拡大（Ctrl＋＋／Ctrl＋ホイール）" disabled={disabled || value >= ZOOM_MAX - 0.001} onClick={() => onStep("in")}><Icon name="plus" /></button>
  </span>;
}
