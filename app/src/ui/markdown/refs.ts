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

/** `file:` URL のパス部分。`file:///C:/a`・`file://localhost/C:/a`・`file:/C:/a` は `/C:/a`、ホスト付き（`file://server/share`＝ネットワーク）と他の形は null。 */
export function fileUrlPath(href: string): string | null {
  const m = /^file:(?:\/\/([^/]*))?(\/.*)?$/i.exec(href.trim());
  if (!m || !m[2]) return null;
  if (m[1] !== undefined && m[1] !== "" && m[1].toLowerCase() !== "localhost") return null;
  return m[2];
}

/** リンクの href からパス候補を取る（`%20` を戻す。`file:` URL も扱う）。 */
export function pathFromHref(href: string | null | undefined): PathCandidate | null {
  if (!href) return null;
  let h = href;
  try { h = decodeURIComponent(href); } catch { /* そのまま */ }
  if (/^file:/i.test(h)) {
    const p = fileUrlPath(h);
    return p ? pathCandidate(p) : null;
  }
  return pathCandidate(h);
}

/** ホストの実行形式判定（attach.rs の一覧）と同じ拡張子。表示用の先読みで、開く可否の決定はホストが行う。 */
const EXECUTABLE_EXTS = new Set(["exe", "bat", "cmd", "ps1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "msi", "msp", "lnk", "com", "scr", "pif", "cpl", "hta", "reg", "jar", "psm1", "appx", "msix", "url", "chm", "msc"]);

export function isExecutableName(path: string): boolean {
  const name = baseNameOf(path).replace(/[. ]+$/, "");
  const i = name.lastIndexOf(".");
  return i > 0 && EXECUTABLE_EXTS.has(name.slice(i + 1).toLowerCase());
}

/** 実在確認の結果（PathInfo の必要な部分）。 */
export interface PathStat { kind: "file" | "folder" | "missing" | "unconfirmed"; absolute: string | null; relative: string | null }

/** 会話中のリンクの出し方。null は確認中（そのまま通常の表示）。 */
export type PathLinkView =
  | { kind: "missing" }
  | { kind: "unconfirmed" }
  /** メニュー。open／reveal が false の項目は出さず、理由を添える。パスのコピーは常に出す。 */
  | { kind: "menu"; open: boolean; reveal: boolean; reason: string | null };

/** 作業フォルダ内（相対パスが取れる）のファイルだけ開く・表示する。外は開かずコピーのみ。実行形式は開かない（表示はできる）。
 *  作業フォルダ自身（フォルダで相対なし）は内とみなす。シンボリックリンク経由の脱出の最終判定はホスト。 */
export function pathLinkView(info: PathStat | undefined, rootAbs?: string | null): PathLinkView | null {
  if (!info) return null;
  if (info.kind === "missing") return { kind: "missing" };
  if (info.kind === "unconfirmed" || !info.absolute) return { kind: "unconfirmed" };
  const norm = (s: string) => s.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  // 相対パスが無いフォルダは、作業フォルダ自身（実在確認した絶対パスが一致）のときだけ内。それ以外は外。
  const isRoot = info.kind === "folder" && !!rootAbs && norm(rootAbs) === norm(info.absolute);
  const inside = info.relative !== null || isRoot;
  if (!inside) return { kind: "menu", open: false, reveal: false, reason: "作業フォルダの外にあるため、開けません（パスのコピーのみ）" };
  if (info.kind === "file" && isExecutableName(info.absolute)) return { kind: "menu", open: false, reveal: true, reason: "実行形式のファイルのため、リンクからは開きません" };
  return { kind: "menu", open: true, reveal: true, reason: null };
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
