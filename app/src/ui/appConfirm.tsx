// アプリ内の確認ダイアログ（OS標準の window.confirm の代わり）。`await askConfirm(spec)` が true なら実行する。
// 開いたら「やめる」へフォーカス、Tab は枠内、Esc・背景クリックはやめる、閉じたら開く前の位置へ戻す。
import { useRef } from "react";
import { createRoot } from "react-dom/client";
import type { ConfirmSpec } from "./confirmTexts";
import { useDialogFocus } from "./useDialogFocus";

function ConfirmView({ spec, done }: { spec: ConfirmSpec; done: (ok: boolean) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useDialogFocus(ref);
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) done(false); }}
      onKeyDown={(e) => { if (e.key === "Escape" && !e.nativeEvent.isComposing) { e.preventDefault(); done(false); } }}>
      <div ref={ref} tabIndex={-1} className="dialog" role="alertdialog" aria-modal="true" aria-label={spec.title}>
        <header><h3>{spec.title}</h3></header>
        <div className="content"><p style={{ whiteSpace: "pre-line" }}>{spec.body}</p></div>
        <footer>
          <div className="foot-l" />
          <div className="foot-r">
            <button className="btn" onClick={() => done(false)}>{spec.cancelLabel ?? "やめる"}</button>
            <button className={spec.danger ? "btn danger" : "btn primary"} onClick={() => done(true)}>{spec.okLabel}</button>
          </div>
        </footer>
      </div>
    </div>
  );
}

/** 確認を出して結果を待つ。同時に複数出ても、それぞれ別に答えを返す。 */
export function askConfirm(spec: ConfirmSpec): Promise<boolean> {
  return new Promise((resolve) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const root = createRoot(host);
    const done = (ok: boolean) => {
      resolve(ok);
      queueMicrotask(() => { root.unmount(); host.remove(); });
    };
    root.render(<ConfirmView spec={spec} done={done} />);
  });
}
