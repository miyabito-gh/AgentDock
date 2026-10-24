// ダイアログ・確認のフォーカス管理（DOM 側）。要素の選別と循環は focusTrap.ts（純粋）。
// - 開いたとき: 最初の操作可能な要素（なければ見出し）へフォーカス。すでに中の要素がフォーカスを持っていれば動かさない。
// - Tab: 最前面のダイアログの中を循環（背景へ抜けない）。
// - 閉じたとき: 開く前にフォーカスのあった要素（開いたボタン）へ戻す。要素が消えていれば戻さない。
import { useEffect, type RefObject } from "react";
import { cycleIndex, FOCUSABLE_SELECTOR, needsTrap, tabbableIndexes, type FocusCand } from "./focusTrap";

// メニュー（Popover）を除き、最後にフォーカスのあった要素を覚える。メニュー項目から開いた確認は、メニューを閉じた後の戻り先に使う。
let lastOutsideMenu: HTMLElement | null = null;
if (typeof document !== "undefined") {
  document.addEventListener("focusin", (e) => {
    const t = e.target;
    if (t instanceof HTMLElement && !t.closest('[role="menu"],[role="dialog"],[role="alertdialog"],.dialog')) lastOutsideMenu = t;
  }, true);
}

/** 開く前のフォーカス位置（開いたボタン）。 */
export function captureOpener(): HTMLElement | null {
  const a = document.activeElement;
  if (a instanceof HTMLElement && a !== document.body) return a;
  return lastOutsideMenu && document.contains(lastOutsideMenu) ? lastOutsideMenu : null;
}

/** 開く前の位置へフォーカスを戻す（要素が消えている・無効になっているときは何もしない）。 */
export function restoreOpener(el: HTMLElement | null): void {
  if (!el || !document.contains(el)) return;
  if (el instanceof HTMLButtonElement && el.disabled) return;
  el.focus({ preventScroll: true });
}

const cand = (el: HTMLElement): FocusCand => ({
  disabled: (el as HTMLButtonElement).disabled === true || el.closest("fieldset:disabled") !== null || el.getAttribute("aria-hidden") === "true",
  hidden: el.getClientRects().length === 0 || getComputedStyle(el).visibility === "hidden",
  tabIndex: el.tabIndex,
  hiddenInput: el instanceof HTMLInputElement && el.type === "hidden",
});

/** root の中で Tab で到達できる要素（文書順）。 */
export function tabbables(root: HTMLElement): HTMLElement[] {
  const all = Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
  return tabbableIndexes(all.map(cand)).map((i) => all[i]);
}

/** 開いている順の、ダイアログの根。最後のものだけが Tab を処理する。 */
const stack: HTMLElement[] = [];

/** モーダルなダイアログの根に付ける。`initialFocus` に false を渡すと、開いたときのフォーカス移動をしない。 */
export function useDialogFocus(ref: RefObject<HTMLElement | null>, initialFocus = true): void {
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const opener = captureOpener();
    stack.push(root);
    if (initialFocus && !root.contains(document.activeElement)) {
      // ヘッダーの「閉じる」ではなく、本文・フッターの最初の操作を優先する。
      const list = tabbables(root);
      const first = list.find((el) => !el.closest("header")) ?? list[0];
      if (first) first.focus({ preventScroll: true });
      else {
        const h = root.querySelector<HTMLElement>("h3,h2,h1") ?? root;
        h.tabIndex = -1;
        h.focus({ preventScroll: true });
      }
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || e.isComposing || e.defaultPrevented || stack[stack.length - 1] !== root) return;
      const list = tabbables(root);
      const cur = list.indexOf(document.activeElement as HTMLElement);
      if (!needsTrap(list.length, cur, e.shiftKey)) return;
      e.preventDefault();
      const next = cycleIndex(list.length, cur, e.shiftKey);
      if (next === null) root.focus({ preventScroll: true });
      else list[next].focus({ preventScroll: true });
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      const i = stack.lastIndexOf(root);
      if (i >= 0) stack.splice(i, 1);
      restoreOpener(opener);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
