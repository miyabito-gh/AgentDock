import { useEffect, useState } from "react";
import { Icon } from "./Icon";
import { COPY_LABEL, useCopy } from "./Markdown";
import { pendingView, type PendingSend } from "./pendingSend";

/** 送信ボタンを押した直後から、受理・未送信・受理不明が分かるまでの依頼の行。
 *  受理（accepted）は通常の依頼と同じ見た目で、会話の本文に現れるまでの間だけ置く。 */
export function PendingSendRow({ p, onRestore, onDismiss }: { p: PendingSend; onRestore: (p: PendingSend) => void; onDismiss: (p: PendingSend) => void }) {
  const waiting = p.outcome === null;
  const [now, setNow] = useState(() => Date.now());
  // 応答待ちの間だけ、経過秒の表示のために1秒ごとに更新する。
  useEffect(() => {
    if (!waiting) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [waiting]);
  const { state, copy } = useCopy();
  const v = pendingView(p, now);
  const body = p.text.trim() ? p.text : null;
  return (
    <div className={`msg user pend ${v.kind}`} data-pending={v.kind}>
      <div className="sr">あなた</div>
      <div className="bubble" data-find="body">{body ?? <span className="muted">（本文なし）</span>}</div>
      {v.kind === "sending" ? (
        <div className="pstat" role="status">
          <span className="spinner" aria-hidden="true" />
          <b>送信中</b>
          {v.slow ? <span className="muted">{v.elapsedSec}秒{v.stage ? `　${v.stage}` : ""}</span> : null}
        </div>
      ) : v.kind === "unsent" ? (
        <div className="pstat bad" role="alert">
          <b>未送信</b>
          <span className="muted" title={v.reason}>{v.reason}</span>
          {v.canRestore ? <button className="small" onClick={() => onRestore(p)}>入力欄へ戻す</button> : null}
          <button className="small" onClick={() => copy(p.text)} aria-live="polite">{state === "idle" ? COPY_LABEL.idle : COPY_LABEL[state]}</button>
          <button className="small" aria-label="この表示を閉じる" onClick={() => onDismiss(p)}><Icon name="x" /></button>
        </div>
      ) : v.kind === "unknown" ? (
        <div className="pstat warn" role="alert">
          <b>受理を確認できません</b>
          <button className="small" onClick={() => copy(p.text)} aria-live="polite">{state === "idle" ? COPY_LABEL.idle : COPY_LABEL[state]}</button>
        </div>
      ) : null}
    </div>
  );
}
