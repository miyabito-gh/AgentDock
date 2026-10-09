// 左一覧・ドックの幅（UI-7）。純粋関数だけ。保存値は窓幅で変えず、表示幅だけを縮める。
export type PaneKind = "left" | "dock";

export interface PaneSpec { def: number; min: number; max: number }

export const PANE: Record<PaneKind, PaneSpec> = {
  left: { def: 240, min: 180, max: 400 },
  dock: { def: 320, min: 260, max: 520 },
};

/** 中央（会話）の最小幅。 */
export const CENTER_MIN = 480;

/** 整数に丸め、最小・最大の範囲へ入れる。数でない値は既定。 */
export function clampPane(kind: PaneKind, v: number | null | undefined): number {
  const s = PANE[kind];
  if (v == null || !Number.isFinite(v)) return s.def;
  return Math.min(s.max, Math.max(s.min, Math.round(v)));
}

/** 保存値の読み込み。未設定は null のまま、あれば丸める。 */
export function loadSaved(kind: PaneKind, v: number | null | undefined): number | null {
  return v == null || !Number.isFinite(v) ? null : clampPane(kind, v);
}

export interface WidthInput {
  winW: number;
  /** 保存値（null＝既定）。 */
  left: number | null;
  dock: number | null;
  /** ハンドルを出して幅を指定する対象か（一時パネル・閉じているペインは false）。 */
  leftShown: boolean;
  dockShown: boolean;
}

export interface Widths {
  left: number;
  dock: number;
  /** ドラッグ・キー操作で出せる最大幅（中央480pxを確保。最小幅は下回らない）。 */
  leftMax: number;
  dockMax: number;
}

/** 表示幅＝min(保存値, 窓幅−もう一方の表示幅−480)、ただし最小幅は下回らない。表示しないペインは0。 */
export function displayWidths(i: WidthInput): Widths {
  const sl = clampPane("left", i.left);
  const sd = clampPane("dock", i.dock);
  // 左を先に決める（ドックは最小幅だけ残す）。次にドックを左の表示幅から決める。
  const leftMax0 = i.leftShown ? Math.max(PANE.left.min, Math.min(PANE.left.max, i.winW - (i.dockShown ? PANE.dock.min : 0) - CENTER_MIN)) : 0;
  const left = i.leftShown ? Math.max(PANE.left.min, Math.min(sl, leftMax0)) : 0;
  const dockMax = i.dockShown ? Math.max(PANE.dock.min, Math.min(PANE.dock.max, i.winW - left - CENTER_MIN)) : 0;
  const dock = i.dockShown ? Math.max(PANE.dock.min, Math.min(sd, dockMax)) : 0;
  // 左の上限は、実際のドック表示幅を見て取り直す。
  const leftMax = i.leftShown ? Math.max(PANE.left.min, Math.min(PANE.left.max, i.winW - dock - CENTER_MIN)) : 0;
  return { left, dock, leftMax, dockMax };
}
