import { useLayoutEffect, type RefObject } from "react";
import type { BackendKind, ModelInfo } from "../ipc/types";
import { Icon } from "./Icon";
import { aiFullName } from "./backendModels";
import type { EnterMode } from "./sendKeys";
import { canQuickSend, isNewChatSubmitKey } from "./quickNew";

/** クイック新規の下書き画面（まだ会話を作っていない）。最初の送信で会話を作る。作業フォルダは一般チャット固定、権限は初期値。 */
export function QuickNewPane(p: {
  /** いま選んでいる担当AIのモデル一覧。 */
  models: ModelInfo[]; model: string; setModel: (id: string) => void;
  /** 担当AI（Codex / Antigravity）。Antigravity は agyReady（モデル一覧が取れている）のときだけ選べる。agyError は取れなかった理由。 */
  provider: BackendKind; setProvider: (p: BackendKind) => void; agyReady: boolean; agyError: string | null;
  text: string; setText: (v: string) => void; enterMode: EnterMode; busy: boolean;
  onSend: () => void; onDetail: () => void; inputRef: RefObject<HTMLTextAreaElement>;
}) {
  const agyBlocked = p.provider === "antigravity" && !p.agyReady;
  const ok = canQuickSend(p.text, p.busy) && !agyBlocked;
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
        {!p.agyReady ? (
          <p className="small rwarn" role={agyBlocked ? "alert" : undefined}>
            <Icon name="alert" /> {aiFullName("antigravity")}は使えません。{p.agyError ? `取得できなかった理由: ${p.agyError}` : "モデル一覧を取得できていません（取得中、または agy が未導入・未ログイン・起動不可の可能性があります）。"}
            {agyBlocked ? <> Codex で作成できます。<button type="button" className="btn sm" style={{ marginLeft: 6 }} onClick={() => p.setProvider("codex")}>Codex に切り替える</button></> : <> いまは Codex だけ選べます。</>}
          </p>
        ) : p.provider === "antigravity" ? <p className="small muted">承認が必要な操作は自動で拒否されます。追加の指示・添付には対応していません。</p> : null}
        <div className="quick-new-ctl">
          <span className="sel"><select aria-label="担当AI" title="担当AI" value={p.provider} disabled={p.busy} onChange={(e) => p.setProvider(e.target.value as BackendKind)}>
            <option value="codex">{aiFullName("codex")}</option>
            <option value="antigravity" disabled={!p.agyReady && p.provider !== "antigravity"}>{aiFullName("antigravity")}</option></select></span>
          <span className="sel"><select aria-label="モデル" title="モデル" value={p.model} onChange={(e) => p.setModel(e.target.value)}>
            {p.models.length === 0 ? <option value="" disabled>モデルを取得できていません</option> : null}{p.models.map((m) => <option key={m.id} value={m.id}>{m.displayName}</option>)}</select></span>
          <button className="btn-line" onClick={p.onDetail}><Icon name="folder" />詳細を指定…</button>
          <button className="btn-main" disabled={!ok} title={agyBlocked ? `${aiFullName("antigravity")}が使えないため作成できません。担当AIを Codex に切り替えてください` : undefined} onClick={p.onSend}>{p.busy ? "作成中…" : "送信して作成"}</button>
        </div>
      </div>
    </div>
  );
}
