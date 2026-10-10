// チャット一覧の固定行高ウィンドウ表示の範囲計算（P3-9）。純粋関数。依存追加なし。
/** これを超えたらウィンドウ表示にする。 */
export const WINDOW_THRESHOLD = 200;
/** 1行表示の行の高さ（px。「標準」の値。CSSの `--row-h` と合わせる）。 */
export const ROW_HEIGHT = 30;

/** 画面の文字サイズ別の一覧の行の高さ（px。styles.css の `--row-h` と同じ値: 小28／標準30／大34）。 */
export function rowHeight(uiSize: "Small" | "Normal" | "Large"): number {
  return uiSize === "Large" ? 34 : uiSize === "Small" ? 28 : ROW_HEIGHT;
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

/** 見出し行の高さ（px。styles.css の `.list-grp` と同じ値）。 */
export const GROUP_HEADER_HEIGHT = 28;

/** 可変高さ（見出し行込み）の累積の上端。`tops[i]` が行 i の上端、`tops[n]` が全体の高さ。 */
export function cumulativeTops(heights: number[]): number[] {
  const tops = new Array<number>(heights.length + 1);
  tops[0] = 0;
  for (let i = 0; i < heights.length; i++) tops[i + 1] = tops[i] + heights[i];
  return tops;
}

/** 可変高さ版の表示範囲 [start, end)。`scrollTop` は一覧の先頭からの相対。上下に `overscan` 行ずつ余分に描く。 */
export function windowRangeVar(heights: number[], scrollTop: number, viewport: number, overscan: number = OVERSCAN): { start: number; end: number } {
  const n = heights.length;
  if (n === 0) return { start: 0, end: 0 };
  const tops = cumulativeTops(heights);
  const y = Math.max(0, scrollTop);
  const bottom = y + Math.max(0, viewport);
  // 上端が y 以下の最後の行（二分探索）
  let lo = 0, hi = n - 1;
  while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (tops[mid] <= y) lo = mid; else hi = mid - 1; }
  const first = lo;
  // 上端が bottom 未満の最後の行
  lo = first; hi = n - 1;
  while (lo < hi) { const mid = (lo + hi + 1) >> 1; if (tops[mid] < bottom) lo = mid; else hi = mid - 1; }
  const start = Math.max(0, first - overscan);
  const end = Math.min(n, lo + 1 + overscan);
  return { start, end: Math.max(start, end) };
}

/** 可変高さ版: 行 `index` を完全に見せるために必要な scrollTop。 */
export function scrollTopToRevealVar(heights: number[], index: number, scrollTop: number, viewport: number): number {
  if (index < 0 || index >= heights.length) return scrollTop;
  const tops = cumulativeTops(heights);
  const top = tops[index], bottom = tops[index + 1];
  if (top < scrollTop) return top;
  if (bottom > scrollTop + viewport) return bottom - viewport;
  return scrollTop;
}
