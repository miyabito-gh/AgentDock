// 左一覧・ドックの幅のハンドル（UI-7）。ドラッグ中は .body の CSS変数を直接更新し、React状態・保存は触らない。
import { useEffect, useRef } from "react";
import { PANE, type PaneKind } from "./paneWidth";

const KEY_SAVE_MS = 500;

interface Props {
  kind: PaneKind;
  /** 現在の表示幅（px）。 */
  width: number;
  /** 出せる最大幅（中央480pxを確保した上限）。 */
  max: number;
  /** 保存。null＝既定に戻す（保存値を未設定へ）。 */
  onCommit: (w: number | null) => void;
}

const META: Record<PaneKind, { label: string; controls: string; cssVar: "--lw" | "--rw"; dir: 1 | -1; cls: string }> = {
  left: { label: "チャット一覧の幅", controls: "pane-left", cssVar: "--lw", dir: 1, cls: "split-l" },
  dock: { label: "ドックの幅", controls: "pane-dock", cssVar: "--rw", dir: -1, cls: "split-r" },
};

export function PaneSplitter({ kind, width, max, onCommit }: Props) {
  const m = META[kind];
  const spec = PANE[kind];
  const ref = useRef<HTMLDivElement>(null);
  const cur = useRef(width);
  const drag = useRef<{ startX: number; startW: number; id: number } | null>(null);
  const raf = useRef(0);
  const keyTimer = useRef<number | undefined>(undefined);
  const commitRef = useRef(onCommit);
  commitRef.current = onCommit;
  const maxRef = useRef(max);
  maxRef.current = max;

  // 操作中でなければ表示幅へ追従する。
  if (!drag.current && keyTimer.current === undefined) cur.current = width;

  // 外れる（パネルを閉じる・一時パネルへ切替）ときに、保留中のキー操作の保存を捨てない。
  useEffect(() => () => {
    cancelAnimationFrame(raf.current);
    if (keyTimer.current !== undefined) {
      window.clearTimeout(keyTimer.current);
      keyTimer.current = undefined;
      commitRef.current(cur.current);
    }
  }, []);

  const clamp = (w: number) => Math.min(maxRef.current, Math.max(spec.min, Math.round(w)));
  const body = () => ref.current?.closest(".body") as HTMLElement | null;
  const apply = (w: number) => {
    cur.current = w;
    body()?.style.setProperty(m.cssVar, `${w}px`);
    ref.current?.setAttribute("aria-valuenow", String(w));
  };
  const applyFrame = (w: number) => {
    cur.current = w;
    cancelAnimationFrame(raf.current);
    raf.current = requestAnimationFrame(() => apply(w));
  };
  const endDrag = () => {
    const el = ref.current;
    if (drag.current && el?.hasPointerCapture(drag.current.id)) el.releasePointerCapture(drag.current.id);
    drag.current = null;
    body()?.classList.remove("resizing");
  };

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    window.clearTimeout(keyTimer.current);
    keyTimer.current = undefined;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = { startX: e.clientX, startW: cur.current, id: e.pointerId };
    body()?.classList.add("resizing");
    e.currentTarget.focus();
    e.preventDefault();
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    applyFrame(clamp(d.startW + m.dir * (e.clientX - d.startX)));
  };
  const onPointerUp = () => {
    const d = drag.current;
    if (!d) return;
    cancelAnimationFrame(raf.current);
    apply(cur.current);
    endDrag();
    if (cur.current !== d.startW) commitRef.current(cur.current); // 離したとき1回だけ
  };
  const onLostCapture = () => { if (drag.current) onPointerUp(); };

  const reset = () => {
    window.clearTimeout(keyTimer.current);
    keyTimer.current = undefined;
    apply(clamp(spec.def));
    commitRef.current(null);
  };
  const key = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      const d = drag.current;
      if (!d) return;
      cancelAnimationFrame(raf.current);
      apply(d.startW);
      endDrag(); // 保存しない
      e.preventDefault();
      return;
    }
    if (drag.current) return;
    const step = e.shiftKey ? 32 : 8;
    let next: number | null = null;
    if (e.key === "ArrowRight") next = cur.current + m.dir * step;
    else if (e.key === "ArrowLeft") next = cur.current - m.dir * step;
    else if (e.key === "Home") next = spec.min;
    else if (e.key === "End") next = maxRef.current;
    else if (e.key === "Enter") { e.preventDefault(); reset(); return; }
    if (next === null) return;
    e.preventDefault();
    apply(clamp(next));
    window.clearTimeout(keyTimer.current);
    keyTimer.current = window.setTimeout(() => {
      keyTimer.current = undefined;
      commitRef.current(cur.current);
    }, KEY_SAVE_MS);
  };

  return (
    <div
      ref={ref} className={`splitter ${m.cls}`} role="separator" aria-orientation="vertical"
      aria-label={m.label} aria-controls={m.controls} aria-valuemin={spec.min} aria-valuemax={max} aria-valuenow={width}
      tabIndex={0}
      onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp} onLostPointerCapture={onLostCapture}
      onKeyDown={key} onDoubleClick={reset}
    />
  );
}
