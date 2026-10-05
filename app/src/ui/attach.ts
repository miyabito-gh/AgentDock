// 添付・成果物の表示用の純粋関数（画面の部品から切り離したもの）。

/** パスの最後の要素（区切りは \ と / の両方）。 */
export const baseName = (path: string): string => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

const pad = (n: number, w = 2): string => String(n).padStart(w, "0");

/** クリップボード画像の表示名（`clipboard-YYYYMMDD-HHMMSS.png`、ローカル時刻）。ホストの `layout::clipboard_image_name` と同じ形。 */
export const clipboardImageName = (d: Date): string =>
  `clipboard-${pad(d.getFullYear(), 4)}${pad(d.getMonth() + 1)}${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}.png`;

const MIME: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", bmp: "image/bmp" };

/** 画像のプレビュー用のMIME。拡張子から決める（ホストも同じ拡張子だけを画像として返す）。 */
export const mimeOf = (name: string): string => MIME[name.split(".").pop()?.toLowerCase() ?? ""] ?? "application/octet-stream";

/** 本文中の最長のバッククォート連続より長いフェンスで囲む（本文の中のコード枠で閉じてしまわないように）。 */
export function fenceCode(text: string): string {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((m) => m.length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}\n${text.replace(/\n$/, "")}\n${fence}`;
}

/** カーソル位置へ枠付きの文章を挿入する。前後が行の途中なら改行を足す。挿入後のカーソル位置も返す。 */
export function insertBlock(draft: string, pos: number, block: string): { text: string; cursor: number } {
  const at = Math.min(Math.max(pos, 0), draft.length);
  const before = draft.slice(0, at);
  const after = draft.slice(at);
  const head = before.length > 0 && !before.endsWith("\n") ? "\n" : "";
  const tail = after.length > 0 && !after.startsWith("\n") ? "\n" : "";
  const inserted = `${head}${block}${tail}`;
  return { text: `${before}${inserted}${after}`, cursor: before.length + inserted.length };
}
