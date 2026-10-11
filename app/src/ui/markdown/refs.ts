// 会話内のチャットID・ファイルパスの検出（純粋部分。React・DOMに依存しない。node --test で検証する）。
// 検出するだけで、リンク・メニューにするかは表示側が「一覧にあるか」「実在を確認できたか」で決める。

/** セッションID（UUIDv7形式。小文字 8-4-4-4-12）。前後が英数字・ハイフンのものは別の文字列の一部なので対象外。 */
const SESSION_ID = "(?<![0-9A-Za-z-])[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}(?![0-9A-Za-z-])";
/** Codexが本文に入れるチャットへの参照 `::created-thread{threadId="…"}`。 */
const DIRECTIVE = "::created-thread\\{\\s*threadId\\s*=\\s*[\"']([^\"'\\s{}]{1,100})[\"']\\s*\\}";

export type RefPiece =
  | { t: "text"; v: string }
  | { t: "thread"; id: string; directive: boolean; raw: string };

/** 文字列を、通常の文字・チャット参照へ分ける。参照が無ければ1要素（元の文字列）。 */
export function splitThreadRefs(text: string): RefPiece[] {
  const re = new RegExp(`${DIRECTIVE}|(${SESSION_ID})`, "g");
  const out: RefPiece[] = [];
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push({ t: "text", v: text.slice(last, m.index) });
    const directive = m[1] !== undefined;
    out.push({ t: "thread", id: (directive ? m[1] : m[2]).toLowerCase(), directive, raw: m[0] });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ t: "text", v: text.slice(last) });
  return out.length ? out : [{ t: "text", v: text }];
}

/** 全体がちょうど1つのチャット参照か（インラインコード用）。 */
export function wholeThreadRef(text: string): { id: string; directive: boolean; raw: string } | null {
  const t = text.trim();
  const p = splitThreadRefs(t);
  return p.length === 1 && p[0].t === "thread" ? { id: p[0].id, directive: p[0].directive, raw: t } : null;
}

export interface PathCandidate { path: string; line: number | null; col: number | null }

const DRIVE_ABS = /^[A-Za-z]:[\\/]/;

/**
 * ファイルパスの候補か。URL・識別子・拡張子の無い単語は除く（実在の確認は別。ここは「確認に出す価値があるか」だけ）。
 * `path:12` `path:12:3` `path#L12` の行番号は切り出す。作業フォルダ基準の相対は `src/a.rs` `./a.md` `a.md`。
 */
export function pathCandidate(raw: string): PathCandidate | null {
  let s = raw.trim();
  if (s.length < 2 || s.length > 300 || /[\r\n\t\0<>"|?*]/.test(s)) return null;
  // ネットワーク・デバイスの場所（UNC、//server/、\\?\UNC\、\\.\）は、自動の存在確認で触れない（認証情報の送信を避ける）ので対象外。
  if (/^[\\/]{2}/.test(s)) return null;
  s = s.replace(/^\/(?=[A-Za-z]:[\\/])/, "");
  if (/^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(s) && !DRIVE_ABS.test(s)) return null;
  let line: number | null = null;
  let col: number | null = null;
  const anchor = /#L(\d+)(?:C(\d+))?(?:-L?\d+(?:C\d+)?)?$/.exec(s);
  if (anchor) { line = Number(anchor[1]); col = anchor[2] ? Number(anchor[2]) : null; s = s.slice(0, anchor.index); }
  else {
    const m = /^(.*?[^:\\/]):(\d+)(?::(\d+))?$/.exec(s);
    if (m && m[1].length > 1) { s = m[1]; line = Number(m[2]); col = m[3] ? Number(m[3]) : null; }
  }
  if (s.length < 2 || /^\.+$/.test(s)) return null;
  const abs = DRIVE_ABS.test(s);
  if ((abs ? s.slice(2) : s).includes(":")) return null;
  if (!abs) {
    if (/\s/.test(s) || /^[-/\\~]/.test(s) || /[(){}=;,$`']/.test(s)) return null;
    const hasSep = /[\\/]/.test(s);
    // 区切りの無い単語は、拡張子があるものだけ（バージョン番号 `1.2.3` は除く）。`.gitignore` のような先頭ドットの名前も許す。
    if (!hasSep && !(/\.[A-Za-z0-9]{1,10}$/.test(s) && !/^\d+(?:\.\d+)+$/.test(s))) return null;
  }
  return { path: s, line, col };
}

/** リンクの href からパス候補を取る（`%20` を戻す）。 */
export function pathFromHref(href: string | null | undefined): PathCandidate | null {
  if (!href) return null;
  let h = href;
  try { h = decodeURIComponent(href); } catch { /* そのまま */ }
  return pathCandidate(h);
}

/** パスの最後の要素（コピー用のファイル名）。 */
export function baseNameOf(p: string): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : p;
}

interface MdNode { type: string; value?: string; children?: MdNode[]; data?: Record<string, unknown> }

const refNode = (kind: "thread" | "path", props: Record<string, string>, text: string): MdNode => ({
  type: kind === "thread" ? "threadRef" : "pathRef",
  data: { hName: "span", hProperties: { "data-ref": kind, ...props } },
  children: [{ type: "text", value: text }],
});

const SKIP = new Set(["code", "link", "linkReference", "definition", "inlineCode", "html"]);

/**
 * remark プラグイン: 本文のチャット参照を `<span data-ref="thread" data-id data-directive>` に、
 * インラインコードのID・パスを `data-ref="thread"|"path"`（`data-code`）にする。コードブロック・リンクの中は変えない。
 * 実際にボタン・メニューにするかは表示側（一覧にあるか・実在を確認できたか）で決め、そうでなければ元の見た目のまま。
 */
export function remarkRefs() {
  const walk = (node: MdNode): void => {
    if (!node.children) return;
    const next: MdNode[] = [];
    for (const c of node.children) {
      if (c.type === "text" && c.value) {
        const pieces = splitThreadRefs(c.value);
        if (pieces.length === 1 && pieces[0].t === "text") { next.push(c); continue; }
        for (const p of pieces) {
          if (p.t === "text") next.push({ type: "text", value: p.v });
          else next.push(refNode("thread", { "data-id": p.id, ...(p.directive ? { "data-directive": "1" } : {}) }, p.raw));
        }
      } else if (c.type === "inlineCode" && c.value) {
        const t = wholeThreadRef(c.value);
        const pc = t ? null : pathCandidate(c.value);
        if (t) next.push(refNode("thread", { "data-id": t.id, "data-code": "1", ...(t.directive ? { "data-directive": "1" } : {}) }, t.raw));
        else if (pc) next.push(refNode("path", { "data-path": pc.path, "data-code": "1" }, c.value));
        else next.push(c);
      } else {
        if (!SKIP.has(c.type)) walk(c);
        next.push(c);
      }
    }
    node.children = next;
  };
  return (tree: MdNode) => { walk(tree); };
}
