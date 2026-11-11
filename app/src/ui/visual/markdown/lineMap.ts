// 編集欄の位置（文字の位置）と行・列の変換（純粋ロジック）。行・列は1始まり。改行はLFにそろえた本文が前提。
// 実行: node --test src/ui/visual/markdown/lineMap.test.ts （tscの対象外）

export function lineOfOffset(text: string, offset: number): number {
  let line = 1;
  const end = Math.min(Math.max(0, offset), text.length);
  for (let i = text.indexOf("\n"); i >= 0 && i < end; i = text.indexOf("\n", i + 1)) line++;
  return line;
}
/** line 行目の先頭の位置。行数を超えたら末尾。 */
export function offsetOfLine(text: string, line: number): number {
  let at = 0;
  for (let n = 1; n < line; n++) {
    const next = text.indexOf("\n", at);
    if (next < 0) return text.length;
    at = next + 1;
  }
  return at;
}
export function lineCol(text: string, offset: number): { line: number; col: number } {
  const end = Math.min(Math.max(0, offset), text.length);
  const start = text.lastIndexOf("\n", end - 1) + 1;
  return { line: lineOfOffset(text, end), col: Array.from(text.slice(start, end)).length + 1 };
}
