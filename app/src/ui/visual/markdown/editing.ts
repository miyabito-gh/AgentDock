// Markdown 編集欄の編集補助（純粋ロジック）。Enter の続き入力、検索・置換。
// 書式（太字・斜体・リンク・見出しレベル）とインデントは Lumark からコピーした lumark/ の関数を使う。
// 位置はすべて、LFにそろえた本文の中の文字の位置。実行: node --test src/ui/visual/markdown/editing.test.ts （tscの対象外）
import { isInsideCodeFence } from "./lumark/markdownStructure.js";

export interface TextEdit { start: number; end: number; text: string; selectionStart: number; selectionEnd: number }

const QUOTE = /^(?:[ \t]{0,3}>[ \t]?)*/;
const ITEM = /^([ \t]*)(?:([-+*])|(\d{1,9})([.)]))([ \t]+)(\[[ xX]\][ \t]+)?/;

/**
 * Enter で、リスト・番号付きリスト・チェックリスト・引用を続ける。
 * - 項目の中身があるとき: 次の項目の印（番号は+1、チェックは未完了）を付けて改行する。
 * - 項目の中身が空のとき: その印を消して、リスト（引用）を終える。
 * 続け入力の対象でない（範囲選択・コードの中・印の途中・通常の行）なら null＝通常の改行。
 */
export function continueList(text: string, start: number, end: number): TextEdit | null {
  if (start !== end || isInsideCodeFence(text, start)) return null;
  const lineStart = text.lastIndexOf("\n", start - 1) + 1;
  const nl = text.indexOf("\n", start);
  const lineEnd = nl < 0 ? text.length : nl;
  const line = text.slice(lineStart, lineEnd);
  const quote = (QUOTE.exec(line) ?? [""])[0];
  const rest = line.slice(quote.length);
  const item = ITEM.exec(rest);
  const markerEnd = lineStart + quote.length + (item ? item[0].length : 0);
  if (item) {
    if (start < markerEnd) return null; // 印の途中では通常の改行
    const content = line.slice(markerEnd - lineStart);
    if (!content.trim()) return { start: lineStart, end: lineEnd, text: quote, selectionStart: lineStart + quote.length, selectionEnd: lineStart + quote.length };
    const [, indent, bullet, num, delim, space, task] = item;
    const marker = bullet ?? `${Number(num) + 1}${delim}`;
    const prefix = quote + indent + marker + space + (task ? "[ ] " : "");
    return { start, end: start, text: "\n" + prefix, selectionStart: start + 1 + prefix.length, selectionEnd: start + 1 + prefix.length };
  }
  if (quote && quote.includes(">")) {
    if (start < lineStart + quote.length) return null;
    if (!rest.trim()) return { start: lineStart, end: lineEnd, text: "", selectionStart: lineStart, selectionEnd: lineStart };
    return { start, end: start, text: "\n" + quote, selectionStart: start + 1 + quote.length, selectionEnd: start + 1 + quote.length };
  }
  return null;
}

export interface Range { start: number; end: number }
const escapeRegex = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
export const FIND_LIMIT = 5000;

/** 検索語（文字そのもの。正規表現ではない）の一致位置。上限を超えたら打ち切る。 */
export function findRanges(text: string, query: string, caseSensitive: boolean): { ranges: Range[]; capped: boolean } {
  if (!query) return { ranges: [], capped: false };
  const re = new RegExp(escapeRegex(query), caseSensitive ? "g" : "gi");
  const ranges: Range[] = [];
  for (const m of text.matchAll(re)) {
    if (ranges.length >= FIND_LIMIT) return { ranges, capped: true };
    ranges.push({ start: m.index ?? 0, end: (m.index ?? 0) + m[0].length });
  }
  return { ranges, capped: false };
}

/** 位置 pos から見て次（backward なら前）の一致の番号。端では反対側へ回る。一致がなければ -1。 */
export function pickMatch(ranges: Range[], pos: number, backward: boolean): number {
  if (!ranges.length) return -1;
  if (backward) {
    for (let i = ranges.length - 1; i >= 0; i--) if (ranges[i].end <= pos) return i;
    return ranges.length - 1;
  }
  const at = ranges.findIndex((r) => r.start >= pos);
  return at < 0 ? 0 : at;
}

/** すべて置換した本文（置換文字列は文字そのもの）。 */
export function replaceAllText(text: string, query: string, replacement: string, caseSensitive: boolean): { text: string; count: number } {
  if (!query) return { text, count: 0 };
  const re = new RegExp(escapeRegex(query), caseSensitive ? "g" : "gi");
  let count = 0;
  const next = text.replace(re, () => { count++; return replacement; });
  return { text: next, count };
}
