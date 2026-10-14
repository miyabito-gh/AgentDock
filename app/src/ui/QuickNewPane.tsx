import type { RefObject } from "react";
import type { ModelInfo } from "../ipc/types";
import { Icon } from "./Icon";
import { canQuickSend } from "./quickNew";

/** クイック新規の下書き画面（まだ会話を作っていない）。最初の送信で会話を作る。作業フォルダは一般チャット固定、権限は初期値。 */
export function QuickNewPane(p: {
  models: ModelInfo[]; model: string; setModel: (id: string) => void;
  text: string; setText: (v: string) => void; enterMode: "ctrl" | "enter"; busy: boolean;
  onSend: () => void; onDetail: () => void; inputRef: RefObject<HTMLTextAreaElement>;
}) {
  const ok = canQuickSend(p.text, p.busy);
  const hint = p.enterMode === "ctrl" ? "Ctrl＋Enterで送信" : "Enterで送信（Shift＋Enterで改行）";
  return (
    <div className="msgs quick-new">
      <div className="quick-new-in">
        <h2>新しいチャット</h2>
        <p className="muted small">まだ作成していません。最初の依頼を送ると会話を作ります。作業フォルダなしの一般チャットで、権限は初期値です。</p>
        <textarea aria-label="最初の依頼" ref={p.inputRef} value={p.text} autoFocus placeholder={`最初の依頼を入力（${hint}）`}
          onChange={(e) => p.setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
            if ((p.enterMode === "ctrl" && e.ctrlKey) || (p.enterMode === "enter" && !e.shiftKey && !e.ctrlKey)) { e.preventDefault(); if (ok) p.onSend(); }
          }} />
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
