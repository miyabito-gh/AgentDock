/** Visual Editor の表示倍率（HTMLはWebViewの倍率、Markdownは文字の倍率）。 */
export const ZOOM_STEPS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3] as const;
export const ZOOM_MIN = ZOOM_STEPS[0];
export const ZOOM_MAX = ZOOM_STEPS[ZOOM_STEPS.length - 1];
export type ZoomStep = "in" | "out" | "reset";

/** 一段階ずつ拡大・縮小する（段階の間の値からは、近い次の段階へ）。 */
export function stepZoom(value: number, step: ZoomStep): number {
  if (step === "reset") return 1;
  if (step === "in") return ZOOM_STEPS.find((z) => z > value + 0.001) ?? ZOOM_MAX;
  return [...ZOOM_STEPS].reverse().find((z) => z < value - 0.001) ?? ZOOM_MIN;
}

export const zoomPercent = (value: number): string => Math.round(value * 100) + "%";

/** Ctrl＋＋／Ctrl＋−／Ctrl＋0（テンキー・JIS配列の；キーも含む）を倍率の操作として読む。 */
export function zoomKeyOf(event: { key: string; code?: string; ctrlKey: boolean; metaKey?: boolean; altKey: boolean }): ZoomStep | null {
  if (!(event.ctrlKey || event.metaKey) || event.altKey) return null;
  if (event.key === "+" || event.key === "=" || event.key === ";" || event.code === "NumpadAdd") return "in";
  if (event.key === "-" || event.key === "_" || event.code === "NumpadSubtract") return "out";
  if (event.key === "0" || event.code === "Numpad0") return "reset";
  return null;
}

/** ファイルごとの倍率（アプリを開いている間だけ覚える）。 */
const remembered = new Map<string, number>();
export const zoomKey = (rootPath: string, path: string): string => (rootPath + "|" + path).toLowerCase();
export const rememberedZoom = (key: string): number => remembered.get(key) ?? 1;
export function rememberZoom(key: string, value: number): void {
  if (value === 1) remembered.delete(key); else remembered.set(key, value);
  if (remembered.size > 200) remembered.delete(remembered.keys().next().value as string);
}
