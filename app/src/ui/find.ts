// 会話内検索の一致位置計算（純粋関数）。DOMは扱わない。
// 区間（segments）は1つの本文要素の文字ノードの並び。連結した文字列で探すので、太字・コード等の境目をまたぐ一致も見つかる。

export interface Pos { seg: number; offset: number }
/** 開始は一致の最初の文字がある区間と位置、終了は最後の文字がある区間の末尾側の位置（終端は含まない）。 */
export interface Hit { start: Pos; end: Pos }

/** 英字（ASCII）の大文字小文字だけ同一視する。長さは変えない。全角半角・かなは同一視しない。 */
export function foldAscii(s: string): string {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
}

/** 左から非重複で最大 limit 件。空の語は0件。 */
export function findInSegments(segments: string[], query: string, limit: number): Hit[] {
  if (!query || limit <= 0) return [];
  const q = foldAscii(query);
  // 連結位置 → 区間の開始位置の表（二分探索用）
  const starts: number[] = [];
  let total = 0;
  for (const s of segments) { starts.push(total); total += s.length; }
  const text = foldAscii(segments.join(""));
  const locate = (idx: number): Pos => {
    let lo = 0, hi = segments.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (starts[mid] <= idx) lo = mid; else hi = mid - 1;
    }
    // 空の区間を飛ばして、文字を持つ区間に合わせる
    while (lo < segments.length - 1 && segments[lo].length === 0) lo++;
    return { seg: lo, offset: idx - starts[lo] };
  };
  const out: Hit[] = [];
  let from = 0;
  while (out.length < limit) {
    const i = text.indexOf(q, from);
    if (i < 0) break;
    const last = locate(i + q.length - 1);
    out.push({ start: locate(i), end: { seg: last.seg, offset: last.offset + 1 } });
    from = i + q.length;
  }
  return out;
}
