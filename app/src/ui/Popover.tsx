import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from "react";

/** 小さなポップオーバー（DESIGN_UI §2.3）。開閉・Esc・外側クリック・フォーカス返却だけを担い、中身は呼び出し側が決める。
 *  開くボタンは `aria-expanded` を持ち、中は `role="menu"`（操作の並び）か `role="dialog"`（説明と選択欄）。
 *  閉じたらフォーカスを開いたボタンへ戻す（外側クリックで閉じたときは、押した先を優先して戻さない）。 */
export function Popover({ trigger, label, title, role = "dialog", align = "left", triggerClassName = "btn subtle sm", textual = false, onOpenChange, children }: {
  /** 開くボタンの中身（文字・アイコン） */
  trigger: ReactNode;
  /** ボタンの `aria-label`（文字だけで意味が分かるときは省略可） */
  label?: string;
  /** ボタンの `title`（ホバーの説明） */
  title?: string;
  role?: "menu" | "dialog";
  align?: "left" | "right";
  triggerClassName?: string;
  /** 本文だけの小さな説明に使う（余白と文字サイズを説明向けにする） */
  textual?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** 中身。関数のときは `close()`（閉じてボタンへフォーカスを戻す）を受け取る */
  children: ReactNode | ((close: () => void) => ReactNode);
}) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLSpanElement>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const id = useId();
  const change = useCallback((v: boolean) => { setOpen(v); onOpenChange?.(v); }, [onOpenChange]);
  const close = useCallback(() => { change(false); btn.current?.focus(); }, [change]);

  useEffect(() => {
    if (!open) return;
    // 開いた直後: メニューは先頭の有効な項目、説明・選択欄は面そのものへフォーカス（キーボードで読める・操作できる）
    const first = panel.current?.querySelector<HTMLElement>(role === "menu" ? "button:not(:disabled)" : "button:not(:disabled),input,select,textarea,a[href]");
    (first ?? panel.current)?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation(); // 下の窓・ダイアログの Esc 処理へ渡さない
      e.preventDefault();
      close();
    };
    const onDown = (e: PointerEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) change(false);
    };
    document.addEventListener("keydown", onKey, true);
    document.addEventListener("pointerdown", onDown, true);
    return () => { document.removeEventListener("keydown", onKey, true); document.removeEventListener("pointerdown", onDown, true); };
  }, [open, role, close, change]);

  const onPanelKey = (e: React.KeyboardEvent) => {
    if (role !== "menu" || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
    const items = Array.from(panel.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? []);
    if (!items.length) return;
    e.preventDefault();
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length].focus();
  };

  return (
    <span className="pop-wrap" ref={wrap}
      onBlur={(e) => { if (open && e.relatedTarget && !wrap.current?.contains(e.relatedTarget as Node)) change(false); }}>
      <button ref={btn} type="button" className={triggerClassName} aria-expanded={open} aria-haspopup={role === "menu" ? "menu" : "dialog"} aria-controls={open ? id : undefined}
        aria-label={label} title={title} onClick={() => change(!open)}>{trigger}</button>
      {open ? (
        <div id={id} ref={panel} role={role} aria-label={label} tabIndex={-1} className={`pop${align === "right" ? " right" : ""}${textual ? " text" : ""}`} onKeyDown={onPanelKey}>
          {typeof children === "function" ? children(close) : children}
        </div>
      ) : null}
    </span>
  );
}
