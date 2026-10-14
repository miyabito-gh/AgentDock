import { useState } from "react";
import { phaseText, type CreatingChat } from "./creating";

/** 作成中の仮行。選択・メニューは出さない。失敗は「入力し直す…」（送らない）、不明は「一覧を更新」だけ（押し直しを促さない）。 */
export function CreatingRows({ items, onAct }: { items: CreatingChat[]; onAct: (id: string, op: "retry" | "dismiss" | "refresh") => void }) {
  const [copied, setCopied] = useState<string | null>(null);
  return (
    <div className="creating-rows" aria-label="作成中のチャット">
      {items.map((c) => (
        <div key={c.tempId} className={`row-creating ${c.result?.kind ?? "busy"}`} role="status">
          <div className="rc-title">{c.label}</div>
          {!c.result ? (
            <div className="rc-sub"><span className="spinner" aria-hidden="true" /> 作成中…（{phaseText(c.phase)}）</div>
          ) : c.result.kind === "failed" ? (
            <>
              <div className="rc-sub lt-fail">作成できませんでした: {c.result.message}</div>
              <div className="rc-act"><button type="button" className="btn sm" onClick={() => onAct(c.tempId, "retry")}>入力し直す…</button><button type="button" className="btn sm" onClick={() => onAct(c.tempId, "dismiss")}>閉じる</button></div>
            </>
          ) : (
            <>
              <div className="rc-sub">作成を確認できません。一覧を更新して確認してください。{c.result.message ? `（${c.result.message}）` : ""}</div>
              {c.checkedNote ? <div className="rc-sub">{c.checkedNote}</div> : null}
              <div className="rc-act">
                <button type="button" className="btn sm" onClick={() => onAct(c.tempId, "refresh")}>一覧を更新</button>
                {c.body ? <button type="button" className="btn sm" onClick={() => void navigator.clipboard.writeText(c.body ?? "").then(() => setCopied(c.tempId), () => setCopied(null))}>{copied === c.tempId ? "コピーしました" : "依頼の本文をコピー"}</button> : null}
                {c.checkedNote ? <button type="button" className="btn sm" onClick={() => onAct(c.tempId, "retry")}>入力し直す…</button> : null}
                <button type="button" className="btn sm" onClick={() => onAct(c.tempId, "dismiss")}>閉じる</button>
              </div>
            </>
          )}
        </div>
      ))}
    </div>
  );
}
