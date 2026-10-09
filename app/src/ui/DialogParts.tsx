// ダイアログの共通部品（UI-3）。外枠 Shell（Dialogs.tsx）のフッターへ、本文側から操作を差し込む。
// 構造: ヘッダー／本文／フッター（左＝副操作、右＝閉じる（キャンセル）＋主操作）。主操作がいちばん右。
import { createContext, useContext, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { shortId } from "./revertView";

interface FootSlots { left: HTMLElement | null; main: HTMLElement | null }
export const FootCtx = createContext<FootSlots>({ left: null, main: null });

/** Shell のフッターの置き場所（左＝副操作、主操作は閉じるの右）を作り、本文へ渡す。 */
export function useFootSlots(): { slots: FootSlots; setLeft: (e: HTMLElement | null) => void; setMain: (e: HTMLElement | null) => void } {
  const [left, setLeft] = useState<HTMLElement | null>(null);
  const [main, setMain] = useState<HTMLElement | null>(null);
  return { slots: { left, main }, setLeft, setMain };
}

/** フッターへ操作を差し込む。left は左の副操作、main は閉じるの右の主操作（無効のときは灰色表示）。 */
export function FootActions({ left, main }: { left?: ReactNode; main?: ReactNode }) {
  const s = useContext(FootCtx);
  return (
    <>
      {left && s.left ? createPortal(left, s.left) : null}
      {main && s.main ? createPortal(main, s.main) : null}
    </>
  );
}

/** 長い説明を「この操作について」に畳む（既定は閉じる）。「未確認」の1文と危険・要確認の文は入れない。 */
export function About({ children }: { children: ReactNode }) {
  return (
    <details className="about">
      <summary>この操作について</summary>
      <div>{children}</div>
    </details>
  );
}

/** 危険な確定の確認ブロック（赤枠）。中に危険ボタンと「やめる」を置く。 */
export function ConfirmBlock({ label, children, actions, wide }: { label: string; children: ReactNode; actions: ReactNode; wide?: boolean }) {
  return (
    <div className={`confirm${wide ? " wide" : ""}`} role="alertdialog" aria-label={label}>
      {children}
      <div className="acts end">{actions}</div>
    </div>
  );
}

export function ShortId({ id }: { id: string }) {
  return <span className="mono" title={id}>{shortId(id)}</span>;
}
