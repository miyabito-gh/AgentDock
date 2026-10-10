// Markdown整形表示の純粋部分（DESIGN_P5 §3）。React・DOMに依存しない（node --test で検証する）。

/** これを超える応答は整形せず文字で出す（§3.4）。 */
export const FORMAT_LIMIT_CHARS = 100_000;
export const FORMAT_LIMIT_LINES = 3_000;
/** 逐次表示中にこれを超えたら、完了まで整形しない（§3.5）。 */
export const STREAMING_LIMIT_CHARS = 30_000;
/** 逐次表示の描き直しの最短間隔（ms）。 */
export const STREAM_THROTTLE_MS = 120;
/** この行数を超えるコードブロックは高さを制限する（§3 CodeBlock）。 */
export const CODE_COLLAPSE_LINES = 40;
export const CODE_COLLAPSED_VIEW_LINES = 20;

export const countLines = (text: string): number => {
  let n = 1;
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", i + 1)) n++;
  return n;
};

/** 整形せず文字で出すべきか。逐次表示中は30,000字、完了後は10万字または3,000行。 */
export function exceedsFormatLimit(text: string, streaming: boolean): boolean {
  if (streaming) return text.length > STREAMING_LIMIT_CHARS;
  return text.length > FORMAT_LIMIT_CHARS || countLines(text) > FORMAT_LIMIT_LINES;
}

/** リンクの扱い。原文が `http(s)://ホスト` の形のものだけ確認を経て開く。それ以外（空・相対・file・mailto・javascript・data・`https:///x`・`http:a.com` 等）は開かない。 */
export function linkPolicy(url: string | null | undefined): "open" | "copyOnly" {
  if (!url || !/^https?:\/\/[^/\s?#]/i.test(url)) return "copyOnly";
  try {
    const u = new URL(url);
    return (u.protocol === "http:" || u.protocol === "https:") && u.hostname !== "" ? "open" : "copyOnly";
  } catch {
    return "copyOnly";
  }
}

export const hostOf = (url: string): string => {
  try { return new URL(url).hostname; } catch { return ""; }
};

/** 長いURL（data: 等）の表示用に先頭40文字で切る。 */
export const shortUrl = (url: string, max = 40): string => (url.length > max ? `${url.slice(0, max)}…` : url);

/** 国際化ドメイン名（punycode の xn-- を含む）か。 */
export const isIdnHost = (host: string): boolean => host.toLowerCase().split(".").some((l) => l.startsWith("xn--"));

/** ホスト比較用の正規化（小文字・末尾ドット・先頭 www. を除く）。 */
const normHost = (h: string): string => h.toLowerCase().replace(/\.$/, "").replace(/^www\./, "");

const DOMAIN_LIKE = /^(?:[\p{L}\p{N}-]+\.)+[\p{L}\p{N}-]{2,}(?::\d+)?(?:[/?#]\S*)?$/u;

/** 表示の文字がURL・www.・ドメイン（xxx.yyy）の形で、実際の行き先のホストと厳密に違うか（偽装の警告用）。 */
export function textMismatchesUrl(text: string, url: string): boolean {
  const t = text.trim();
  let shown: string;
  if (/^https?:\/\/\S+$/i.test(t)) shown = hostOf(t);
  else if (/^\S+$/.test(t) && DOMAIN_LIKE.test(t)) shown = hostOf(`http://${t}`);
  else return false;
  const real = hostOf(url);
  return shown !== "" && real !== "" && normHost(shown) !== normHost(real);
}

interface MdNode { type: string; value?: string; children?: MdNode[] }

const BLOCK_PARENTS = new Set(["root", "blockquote", "listItem", "footnoteDefinition"]);

/**
 * remark プラグイン: 生HTML（mdast の html ノード）を同じ値の text ノードへ置き換え、見える文字として残す。
 * 黙って消さない・解釈しない（rehype-raw は使わない）。ブロック位置のものは段落で包む。
 */
export function remarkHtmlAsText() {
  const walk = (node: MdNode): void => {
    if (!node.children) return;
    node.children = node.children.map((c) => {
      if (c.type === "html") {
        const text: MdNode = { type: "text", value: c.value ?? "" };
        return BLOCK_PARENTS.has(node.type) ? { type: "paragraph", children: [text] } : text;
      }
      walk(c);
      return c;
    });
  };
  return (tree: MdNode) => { walk(tree); };
}
