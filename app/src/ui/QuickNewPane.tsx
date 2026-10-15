import { useLayoutEffect, type RefObject } from "react";
import type { ModelInfo } from "../ipc/types";
import { Icon } from "./Icon";
import { canQuickSend, isNewChatSubmitKey } from "./quickNew";

/** クイック新規の下書き画面（まだ会話を作っていない）。最初の送信で会話を作る。作業フォルダは一般チャット固定、権限は初期値。 */
export function QuickNewPane(p: {
  models: ModelInfo[]; model: string; setModel: (id: string) => void;
  text: string; setText: (v: string) => void; enterMode: "ctrl" | "enter"; busy: boolean;
  onSend: () => void; onDetail: () => void; inputRef: RefObject<HTMLTextAreaElement>;
}) {
  const ok = canQuickSend(p.text, p.busy);
  const hint = p.enterMode === "ctrl" ? "Ctrl＋Enterで送信、Enterで改行" : "EnterまたはCtrl＋Enterで送信、Shift＋Enterで改行";
  // 自動表示（選択が外れた等）ではフォーカスを奪わない: 他に操作対象が無い（body／未設定）ときだけ入れる。
  // 明示操作（ボタン・Ctrl+N）の確実なフォーカスは App の newChatQuick が行う。窓が戻った時も同じ条件で入れ直す。
  const ref = p.inputRef;
  useLayoutEffect(() => {
    const onWin = () => {
      const a = document.activeElement;
      if ((!a || a === document.body) && ref.current && !ref.current.disabled) ref.current.focus();
    };
    onWin();
    window.addEventListener("focus", onWin);
    return () => window.removeEventListener("focus", onWin);
  }, [ref]);
  return (
    <div className="msgs quick-new">
      <div className="quick-new-in">
        <h2>新しいチャット</h2>
        <p className="muted small">まだ作成していません。最初の依頼を送ると会話を作ります。作業フォルダなしの一般チャットで、権限は初期値です。</p>
        <textarea aria-label="最初の依頼" ref={p.inputRef} value={p.text} readOnly={p.busy} placeholder={`最初の依頼を入力（${hint}）`}
          onChange={(e) => p.setText(e.target.value)}
          onKeyDown={(e) => {
            if (isNewChatSubmitKey({ key: e.key, ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey, metaKey: e.metaKey, isComposing: e.nativeEvent.isComposing }, p.enterMode)) { e.preventDefault(); if (ok) p.onSend(); }
          }} />
        <p className="small muted" role="status">{p.busy ? "会話を作成して送信しています。完了すると新しいチャットへ移ります。" : hint}</p>
        <div className="quick-new-ctl">
          <span className="sel"><select aria-label="モデル" title="モデル" value={p.model} onChange={(e) => p.setModel(e.target.value)}>
            {p.models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></span>
          <button className="btn-line" onClick={p.onDetail}><Icon name="folder" />詳細を指定…</button>
          <button className="btn-main" disabled={!ok} onClick={p.onSend}>{p.busy ? "作成中…" : "送信して作成"}</button>
        </div>
      </div>
    </div>
  );
}
