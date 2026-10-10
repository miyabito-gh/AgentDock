import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { findInSegments } from "./find";
import { Icon } from "./Icon";

/** 件数の上限。超えたら「1000件以上」。 */
export const FIND_LIMIT = 1000;
const TYPE_DELAY_MS = 150;
const WATCH_DELAY_MS = 300;

/** CSS Custom Highlight API（非対応の環境では undefined）。 */
type HighlightCtor = new (...r: Range[]) => unknown;
const registry = (): Map<string, unknown> | undefined =>
  (typeof CSS !== "undefined" ? (CSS as unknown as { highlights?: Map<string, unknown> }).highlights : undefined);
const highlightCtor = (): HighlightCtor | undefined => (globalThis as unknown as { Highlight?: HighlightCtor }).Highlight;

/** 本文要素（data-find="body"）ごとに文字ノードを集め、連結文字列で一致を探して Range にする。ボタン・読み上げ専用の文字は除く。 */
function collect(root: HTMLElement, query: string): { ranges: Range[]; capped: boolean } {
  const ranges: Range[] = [];
  let capped = false;
  for (const body of Array.from(root.querySelectorAll<HTMLElement>('[data-find="body"]'))) {
    const nodes: Text[] = [];
    const w = document.createTreeWalker(body, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => (n.parentElement?.closest("button,.sr,[aria-hidden=true]") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
    });
    for (let n = w.nextNode(); n; n = w.nextNode()) nodes.push(n as Text);
    const hits = findInSegments(nodes.map((n) => n.data), query, FIND_LIMIT + 1 - ranges.length);
    for (const h of hits) {
      const r = document.createRange();
      r.setStart(nodes[h.start.seg], h.start.offset);
      r.setEnd(nodes[h.end.seg], h.end.offset);
      ranges.push(r);
    }
    if (ranges.length > FIND_LIMIT) { capped = true; ranges.length = FIND_LIMIT; break; }
  }
  return { ranges, capped };
}

function clearHighlights() {
  const reg = registry();
  reg?.delete("find-hit");
  reg?.delete("find-cur");
}

export function FindBar({ root, query, setQuery, focusTick, partial, onClose }: {
  /** 検索対象の会話領域（.msgs）。 */
  root: RefObject<HTMLElement | null>;
  query: string;
  setQuery: (q: string) => void;
  /** 増えるたびに入力欄へフォーカスして全選択する（Ctrl+F の再押下）。 */
  focusTick: number;
  /** 本文が部分取得のとき true。 */
  partial: boolean;
  onClose: () => void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const ranges = useRef<Range[]>([]);
  const cur = useRef(0);
  const [info, setInfo] = useState({ n: 0, cur: 0, capped: false });
  const timer = useRef<number | null>(null);

  const paint = useCallback((scroll: boolean) => {
    const rs = ranges.current;
    const reg = registry();
    const H = highlightCtor();
    const c = rs[cur.current];
    if (reg && H) {
      reg.set("find-hit", new H(...rs.filter((_, i) => i !== cur.current)));
      if (c) reg.set("find-cur", new H(c)); else reg.delete("find-cur");
    } else if (c) {
      // 非対応環境: 現在位置だけ Selection で示す（入力欄のフォーカスは保つ）
      const sel = window.getSelection();
      sel?.removeAllRanges();
      sel?.addRange(c);
      input.current?.focus({ preventScroll: true });
    }
    if (scroll && c) c.startContainer.parentElement?.scrollIntoView({ block: "center" });
  }, []);

  /** 検索し直す。keepPos のときは直前の現在位置以降で最初の一致に合わせる（逐次表示で飛ばない）。 */
  const run = useCallback((keepPos: boolean, scroll: boolean) => {
    const el = root.current;
    if (!el || !query) {
      ranges.current = []; cur.current = 0;
      clearHighlights();
      setInfo({ n: 0, cur: 0, capped: false });
      return;
    }
    const prev = ranges.current[cur.current];
    const prevIdx = cur.current;
    const { ranges: rs, capped } = collect(el, query);
    let idx = 0;
    if (keepPos && prev && rs.length) {
      if (prev.startContainer.isConnected) {
        const j = rs.findIndex((r) => r.compareBoundaryPoints(Range.START_TO_START, prev) >= 0);
        idx = j < 0 ? rs.length - 1 : j;
      } else idx = Math.min(prevIdx, rs.length - 1);
    }
    ranges.current = rs; cur.current = idx;
    paint(scroll);
    setInfo({ n: rs.length, cur: idx, capped });
  }, [root, query, paint]);

  // 検索語の変更は150ms待つ。チャットの切替（FindBarの再生成）では同じ語で直ちに検索する。
  const first = useRef(true);
  useEffect(() => {
    const wait = first.current ? 0 : TYPE_DELAY_MS;
    first.current = false;
    const t = window.setTimeout(() => run(false, true), wait);
    return () => window.clearTimeout(t);
  }, [run]);

  // 本文の更新（逐次表示・Markdown再描画）を見て、300msごとに検索し直す。DOMは書き換えない。
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const mo = new MutationObserver(() => {
      if (timer.current !== null) return;
      timer.current = window.setTimeout(() => { timer.current = null; run(true, false); }, WATCH_DELAY_MS);
    });
    mo.observe(el, { childList: true, subtree: true, characterData: true });
    return () => { mo.disconnect(); if (timer.current !== null) { window.clearTimeout(timer.current); timer.current = null; } };
  }, [root, run]);

  useEffect(() => () => clearHighlights(), []);

  useEffect(() => { input.current?.focus(); input.current?.select(); }, [focusTick]);

  const move = useCallback((d: 1 | -1) => {
    const n = ranges.current.length;
    if (!n) return;
    cur.current = (cur.current + d + n) % n;
    paint(true);
    setInfo((i) => ({ ...i, cur: cur.current }));
  }, [paint]);

  // F3／Shift+F3 は帯の外にフォーカスがあっても効く（WebViewの既定の検索へ渡さない）
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key !== "F3" || e.ctrlKey || e.altKey || e.metaKey) return;
      e.preventDefault();
      move(e.shiftKey ? -1 : 1);
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [move]);

  const status = !query ? "" : info.n === 0 ? "見つかりません" : info.capped ? `${info.cur + 1} / ${FIND_LIMIT}件以上` : `${info.cur + 1} / ${info.n}件`;
  return (
    <div className="findbar" role="search" title="依頼と応答の本文を検索します（作業の記録は対象外）">
      <input ref={input} type="text" value={query} placeholder="会話内を検索" aria-label="会話内を検索"
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.nativeEvent.isComposing) return;
          if (e.key === "Enter") { e.preventDefault(); move(e.shiftKey ? -1 : 1); }
          else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); }
        }} />
      <span className={`find-count${query && info.n === 0 ? " none" : ""}`} role="status" aria-live="polite">{status}</span>
      {partial ? <span className="find-note">読み込み済みの範囲を検索</span> : null}
      <button type="button" className="find-btn" onClick={() => move(-1)} disabled={info.n === 0} aria-label="前の一致" title="前の一致（Shift+Enter）">↑</button>
      <button type="button" className="find-btn" onClick={() => move(1)} disabled={info.n === 0} aria-label="次の一致" title="次の一致（Enter）">↓</button>
      <button type="button" className="find-btn" onClick={onClose} aria-label="検索を閉じる" title="閉じる（Esc）"><Icon name="x" /></button>
    </div>
  );
}
