// 文書内リンク（#見出し）の行き先を、見出しの data-slug と同じ形にそろえる。
// marked はリンク先の日本語などを %XX に符号化するので戻す。見出し側（createHeadingSlugger）と同じく NFKC・小文字にする。
export function anchorKey(raw: string): string {
  let s = raw;
  try { s = decodeURIComponent(raw); } catch { /* 壊れた %XX はそのまま比べる */ }
  return s.normalize("NFKC").toLowerCase();
}
