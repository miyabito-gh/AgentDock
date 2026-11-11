// Markdown のプレビュー表示。ブロック（data-src-start/end）を押して選び、行の範囲と抜粋を依頼に添える。
// 画像はフォルダ内の相対パスだけをホスト経由で読む（外部URLは読み込まない）。リンクは移動しない。
import { forwardRef, useImperativeHandle, useLayoutEffect, useRef } from "react";
import type { PreviewElement } from "../api";
import { visualMarkdownImage } from "../api";
import { blockElement } from "./context";
import { excerptOf } from "./render";

export interface PreviewHandle {
  /** 指定行（1始まり）を含むブロックを表示位置へ出す。 */
  scrollToLine: (line: number) => void;
}

const SELECTABLE = "[data-src-start]";
const num = (el: Element, name: string): number => Number(el.getAttribute(name) ?? 0);
const sameRange = (a: PreviewElement, start: number, end: number): boolean => a.selector === (start === end ? `L${start}` : `L${start}-${end}`);

/** 描画済みのブロックのうち、キー操作で渡り歩く対象（引用の外枠と、段落を含む項目は飛ばす）。 */
function navigable(box: HTMLElement): HTMLElement[] {
  return Array.from(box.querySelectorAll<HTMLElement>(SELECTABLE)).filter((el) => el.tagName !== "BLOCKQUOTE" && !(el.tagName === "LI" && el.querySelector(":scope > p[data-src-start]")));
}

export const MarkdownPreview = forwardRef<PreviewHandle, {
  html: string; text: string; rootPath: string; path: string; /** 読み直しのたびに増える（画像を読み直す）。 */ epoch: number;
  selection: PreviewElement[]; onSelect: (items: PreviewElement[]) => void; /** 選んだブロックの先頭行を通知する（編集欄の位置合わせ用）。 */ onBlockPick?: (line: number) => void;
  /** 編集位置の行（1始まり）。そのブロックを目立たせる。 */ currentLine?: number | null;
}>(function MarkdownPreview({ html, text, rootPath, path, epoch, selection, onSelect, onBlockPick, currentLine }, ref) {
  const box = useRef<HTMLDivElement>(null);
  const images = useRef(new Map<string, string>());
  const imageKey = useRef("");
  const selectionRef = useRef(selection);
  selectionRef.current = selection;

  useImperativeHandle(ref, () => ({
    scrollToLine(line) {
      const el = blockAt(box.current, line);
      el?.scrollIntoView({ block: "nearest", behavior: "auto" });
    },
  }), []);

  // 画像: 描画のたびに、相対パスの画像だけをホスト経由で読む（同じ参照は覚えておく）。
  useLayoutEffect(() => {
    const root = box.current;
    if (!root) return;
    const key = `${rootPath}|${path}|${epoch}`;
    if (imageKey.current !== key) { images.current.clear(); imageKey.current = key; }
    let live = true;
    const off = (img: HTMLImageElement, why: string) => {
      const span = document.createElement("span");
      span.className = "md-img-off";
      span.textContent = `（画像を表示しません: ${why}）`;
      img.replaceWith(span);
    };
    for (const img of Array.from(root.querySelectorAll<HTMLImageElement>("img"))) {
      const href = img.getAttribute("data-local-src");
      if (img.getAttribute("data-blocked")) { off(img, "外部の画像は読み込みません"); continue; }
      if (!href) continue;
      const cached = images.current.get(href);
      if (cached !== undefined) { if (cached) img.src = cached; else off(img, "見つからないか、フォルダの外です"); continue; }
      void visualMarkdownImage(rootPath, path, href).then((url) => { images.current.set(href, url); if (live && img.isConnected) img.src = url; })
        .catch(() => { images.current.set(href, ""); if (live && img.isConnected) off(img, "見つからないか、フォルダの外です"); });
    }
    return () => { live = false; };
  }, [html, rootPath, path, epoch]);

  // 選択中・編集位置のブロックを目立たせる（描画のたびにやり直す）。
  useLayoutEffect(() => {
    const root = box.current;
    if (!root) return;
    const cur = currentLine ? blockAt(root, currentLine) : null;
    for (const el of Array.from(root.querySelectorAll<HTMLElement>(SELECTABLE))) {
      const start = num(el, "data-src-start"), end = num(el, "data-src-end");
      el.classList.toggle("md-sel", selection.some((s) => sameRange(s, start, end)));
      el.classList.toggle("md-cur", el === cur);
    }
  }, [html, selection, currentLine]);

  const pick = (el: HTMLElement, additive: boolean) => {
    const start = num(el, "data-src-start"), end = num(el, "data-src-end");
    const item = blockElement(start, end, el.tagName.toLowerCase(), excerptOf(text, start, end));
    const cur = selectionRef.current;
    const has = cur.some((s) => s.selector === item.selector);
    if (additive) onSelect(has ? cur.filter((s) => s.selector !== item.selector) : [...cur, item].sort((a, b) => num0(a) - num0(b)));
    else onSelect(has && cur.length === 1 ? [] : [item]);
    onBlockPick?.(start);
  };

  const onClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    const link = target.closest("a");
    if (link) {
      event.preventDefault();
      const anchor = link.getAttribute("data-anchor");
      if (anchor) box.current?.querySelector<HTMLElement>(`[data-slug="${CSS.escape(anchor)}"]`)?.scrollIntoView({ block: "start" });
      return;
    }
    const picked = window.getSelection();
    if (picked && !picked.isCollapsed && box.current?.contains(picked.anchorNode)) return; // 文字をドラッグで選んだだけのときは、ブロックの選択を変えない
    const el = target.closest<HTMLElement>(SELECTABLE);
    if (!el) { if (!(event.ctrlKey || event.metaKey || event.shiftKey)) onSelect([]); return; }
    pick(el, event.ctrlKey || event.metaKey || event.shiftKey);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.nativeEvent.isComposing || event.target !== event.currentTarget) return;
    if (event.key === "Escape") { if (selectionRef.current.length) { event.preventDefault(); onSelect([]); } return; }
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const root = box.current;
    if (!root) return;
    event.preventDefault();
    const list = navigable(root);
    if (!list.length) return;
    const last = selectionRef.current[selectionRef.current.length - 1];
    const at = last ? list.findIndex((el) => sameRange(last, num(el, "data-src-start"), num(el, "data-src-end"))) : -1;
    const next = list[Math.max(0, Math.min(list.length - 1, at < 0 ? (event.key === "ArrowDown" ? 0 : list.length - 1) : at + (event.key === "ArrowDown" ? 1 : -1)))];
    pick(next, false);
    next.scrollIntoView({ block: "nearest" });
  };

  return <div ref={box} className="md md-doc md-preview" tabIndex={0} role="document" aria-label="Markdownのプレビュー（クリックでブロックを選択、↑↓で移動、Escで解除）"
    onClick={onClick} onKeyDown={onKeyDown} dangerouslySetInnerHTML={{ __html: html }} />;
});

const num0 = (item: PreviewElement): number => Number(/^L(\d+)/.exec(item.selector)?.[1] ?? 0);

/** 指定行（1始まり）を含む、いちばん内側のブロック。 */
export function blockAt(root: HTMLElement | null, line: number): HTMLElement | null {
  if (!root) return null;
  let best: HTMLElement | null = null, before: HTMLElement | null = null;
  for (const el of Array.from(root.querySelectorAll<HTMLElement>(SELECTABLE))) {
    const start = num(el, "data-src-start"), end = num(el, "data-src-end");
    if (start > line) break;
    before = el;
    if (line <= end) best = el; // 文書順に後ろのものほど内側
  }
  return best ?? before; // 空行の上なら、直前のブロック
}

export { SELECTABLE };
