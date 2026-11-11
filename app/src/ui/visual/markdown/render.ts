// Markdown → HTML（サニタイズ前）と、ブロックごとの元の行の範囲・見出しの一覧（純粋ロジック）。
// 各ブロック（見出し・段落・リスト項目・引用・コード・表・区切り線）の開始タグに data-src-start / data-src-end（1始まり・両端を含む行番号）を付ける。
// サニタイズは sanitize.ts（ブラウザのDOMが要る）。実行: node --test src/ui/visual/markdown/render.test.ts （tscの対象外）
import { Lexer, Parser, Renderer, type Token, type Tokens } from "marked";
import { createHeadingSlugger } from "./lumarkSecurity.ts";

export interface OutlineItem { level: number; text: string; /** 1始まりの行番号 */ line: number; slug: string }
export interface Rendered { html: string; outline: OutlineItem[] }
interface Span { srcStart: number; srcEnd: number }
type Annotated<T> = T & Partial<Span>;

const newlines = (s: string): number => { let n = 0; for (let i = s.indexOf("\n"); i >= 0; i = s.indexOf("\n", i + 1)) n++; return n; };
const trimEndNl = (s: string): string => s.replace(/\n+$/, "");

/** 各トークンへ元の行の範囲を付ける（0始まりで開始行を渡す。rawは連結すると元の文章になる）。 */
function annotate(tokens: Token[], base: number): void {
  let line = base;
  for (const t of tokens as Annotated<Token>[]) {
    const raw = t.raw ?? "";
    t.srcStart = line + 1;
    t.srcEnd = line + 1 + newlines(trimEndNl(raw));
    if (t.type === "list") {
      let at = line;
      for (const item of (t as Tokens.List).items as Annotated<Tokens.ListItem>[]) {
        item.srcStart = at + 1;
        item.srcEnd = at + 1 + newlines(trimEndNl(item.raw));
        annotate(item.tokens, at);
        at += newlines(item.raw);
      }
    } else if (t.type === "blockquote") {
      annotate((t as Tokens.Blockquote).tokens, line);
    }
    line += newlines(raw);
  }
}

function withSpan(html: string, token: Annotated<Token>, extra = ""): string {
  if (token.srcStart === undefined || token.srcEnd === undefined) return html;
  return html.replace(/^<([a-zA-Z][a-zA-Z0-9]*)/, `<$1 data-src-start="${token.srcStart}" data-src-end="${token.srcEnd}"${extra}`);
}

class SpanRenderer extends Renderer {
  private slug = createHeadingSlugger();
  override heading(token: Tokens.Heading): string { return withSpan(super.heading(token), token, ` data-slug="${this.slug(token.text).replace(/"/g, "")}"`); }
  override paragraph(token: Tokens.Paragraph): string { return withSpan(super.paragraph(token), token); }
  override code(token: Tokens.Code): string { return withSpan(super.code(token), token); }
  override blockquote(token: Tokens.Blockquote): string { return withSpan(super.blockquote(token), token); }
  override table(token: Tokens.Table): string { return withSpan(super.table(token), token); }
  override hr(token: Tokens.Hr): string { return withSpan(super.hr(token), token); }
  override listitem(item: Tokens.ListItem): string { return withSpan(super.listitem(item), item); }
}

const plain = (s: string): string => s.replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1").replace(/[`*_~]/g, "").replace(/<[^>]*>/g, "").trim();

export function renderMarkdown(text: string): Rendered {
  const tokens = new Lexer({ gfm: true, breaks: false }).lex(text);
  annotate(tokens, 0);
  const slug = createHeadingSlugger();
  const outline: OutlineItem[] = [];
  for (const t of tokens as Annotated<Token>[]) {
    if (t.type === "heading") outline.push({ level: (t as Tokens.Heading).depth, text: plain((t as Tokens.Heading).text), line: t.srcStart ?? 1, slug: slug((t as Tokens.Heading).text).replace(/"/g, "") });
  }
  const html = Parser.parse(tokens, { renderer: new SpanRenderer(), gfm: true, breaks: false });
  return { html, outline };
}

/** 見出しの一覧だけが欲しいとき（描画しない）。 */
export const outlineOf = (text: string): OutlineItem[] => renderMarkdown(text).outline;

const TAG_LABEL: Record<string, string> = { h1: "見出し", h2: "見出し", h3: "見出し", h4: "見出し", h5: "見出し", h6: "見出し", p: "段落", li: "リスト項目", pre: "コード", table: "表", blockquote: "引用", hr: "区切り線" };
export const tagLabel = (tag: string): string => TAG_LABEL[tag.toLowerCase()] ?? "ブロック";

/** 元の文章の start〜end 行（1始まり・両端を含む）の抜粋。空白をつぶし、max文字で切り詰める。 */
export function excerptOf(text: string, start: number, end: number, max = 160): string {
  const lines = text.split("\n").slice(Math.max(0, start - 1), Math.max(start, end));
  const flat = lines.join(" ").replace(/\s+/g, " ").trim();
  const chars = Array.from(flat);
  return chars.length > max ? chars.slice(0, max).join("") + "…" : flat;
}
