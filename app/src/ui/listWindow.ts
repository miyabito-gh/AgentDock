// チャット一覧の固定行高ウィンドウ表示の範囲計算（P3-9）。純粋関数。依存追加なし。
/** これを超えたらウィンドウ表示にする。 */
export const WINDOW_THRESHOLD = 200;
/** 窓表示のときの1行の高さ（px。「標準」の値。CSSの `--row-h` と合わせる）。 */
export const ROW_HEIGHT = 40;

/** 画面の文字サイズ別の一覧の行の高さ（px。styles.css の `--row-h` と同じ値）。 */
export function rowHeight(uiSize: "Small" | "Normal" | "Large"): number {
  return uiSize === "Large" ? 44 : 40;
}
const OVERSCAN = 8;

/**
 * 表示する行の範囲 [start, end)。`scrollTop` は一覧の先頭からの相対（先頭より上なら負でも可）、`viewport` は見える高さ。
 * 上下に少し余分に描く。総数が0なら空。
 */
export function windowRange(total: number, scrollTop: number, viewport: number, rowH: number = ROW_HEIGHT, overscan: number = OVERSCAN): { start: number; end: number } {
  if (total <= 0 || rowH <= 0) return { start: 0, end: 0 };
  const first = Math.floor(Math.max(0, scrollTop) / rowH);
  const visible = Math.ceil(Math.max(0, viewport) / rowH) + 1;
  const start = Math.min(total, Math.max(0, first - overscan));
  const end = Math.min(total, Math.max(start, first + visible + overscan));
  return { start, end };
}

/** 行 `index` を完全に見せるために必要な scrollTop（一覧先頭からの相対）。すでに見えていれば現在値のまま。 */
export function scrollTopToReveal(index: number, scrollTop: number, viewport: number, rowH: number = ROW_HEIGHT): number {
  const top = index * rowH;
  const bottom = top + rowH;
  if (top < scrollTop) return top;
  if (bottom > scrollTop + viewport) return bottom - viewport;
  return scrollTop;
}
