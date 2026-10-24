// 左一覧の「依頼整理の会話をまとめて削除」の確認と実行。押したときだけ実行し、1件ずつ順に既存のチャット削除を呼ぶ。
// キャンセル・Esc・背景クリックでは実行しない。実行中は二重実行を防ぎ、1件失敗しても止めない。判定の最終はホスト。
import { useEffect, useRef, useState } from "react";
import type { ChatKey, DeleteOutcome } from "../ipc/types";
import * as host from "../ipc/client";
import { useDialogFocus } from "./useDialogFocus";
import { elapsedLabel, isSlow } from "./orgDeleteUi";
import { BULK_HEADLINE, BULK_HOST_NOTE, BULK_IRREVERSIBLE, previewLines, reasonOf, resultText, stateCounts, type BulkFailure, type IntakeDeleteRow } from "./intakeDelete";

/** 1件分の結果を理由つきの文にする（deleted 以外は消えていない）。 */
function outcomeReason(o: DeleteOutcome): string | null {
  if (o.kind === "deleted") return null;
  if (o.kind === "pending") return "停止を確認できていません（削除保留にしました）";
  return `一部を削除できませんでした: ${o.failed.join("、")}`;
}

export function IntakeBulkDeleteDialog({ rows, onFinished, onClose }: {
  rows: IntakeDeleteRow[];
  /** 結果の一言（トーストなどで残す）。1件でも処理したあとに呼ぶ。 */
  onFinished: (text: string) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useDialogFocus(ref);
  const [phase, setPhase] = useState<"confirm" | "running" | "done">("confirm");
  const [idx, setIdx] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const [result, setResult] = useState<{ deleted: number; failed: BulkFailure[] } | null>(null);
  const started = useRef(0);
  const busy = useRef(false); // 連打の二重実行を防ぐ
  useEffect(() => {
    if (phase !== "running") return;
    const t = setInterval(() => setElapsed(Date.now() - started.current), 1000);
    return () => clearInterval(t);
  }, [phase]);
  const counts = stateCounts(rows);
  const run = async () => {
    if (busy.current || rows.length === 0) return;
    busy.current = true;
    started.current = Date.now(); setElapsed(0); setIdx(0); setPhase("running");
    let deleted = 0;
    const failed: BulkFailure[] = [];
    for (let i = 0; i < rows.length; i++) {
      setIdx(i);
      const r = rows[i];
      try {
        const o = await host.intakeDeleteChat(r.chat as ChatKey);
        const why = outcomeReason(o);
        if (why === null) deleted++; else failed.push({ title: r.title, reason: why });
      } catch (e) {
        failed.push({ title: r.title, reason: reasonOf(host.asIpcError(e).message) });
      }
    }
    setResult({ deleted, failed });
    setPhase("done");
    onFinished(resultText(deleted, failed));
    busy.current = false;
  };
  // 実行中はEsc・背景で閉じない（90秒を超えたら閉じてよい。処理は続く）。確認中・終了後は閉じてよい（実行はしない）。
  const canClose = phase !== "running" || isSlow(elapsed);
  const close = () => { if (canClose) onClose(); };
  return (
    <div className="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) close(); }}
      onKeyDown={(e) => { if (e.key === "Escape" && !e.nativeEvent.isComposing) { e.preventDefault(); close(); } }}>
      <div ref={ref} tabIndex={-1} className="dialog" role="alertdialog" aria-modal="true" aria-label="依頼整理の会話をまとめて削除">
        <header><h3>{phase === "done" ? "依頼整理の会話の削除の結果" : BULK_HEADLINE(rows.length)}</h3></header>
        <div className="content">
          {phase === "done" && result ? (
            <>
              <p role="status">{resultText(result.deleted, result.failed)}</p>
              {result.failed.length > 0 ? <ul className="check-list">{result.failed.map((f, i) => <li key={i}>{f.title}<span className="why err">　できませんでした: {f.reason}</span></li>)}</ul> : null}
            </>
          ) : (
            <>
              <p>{BULK_IRREVERSIBLE}</p>
              <p className="small muted">開始済み {counts.started} 件／やめたもの {counts.abandoned} 件。{BULK_HOST_NOTE}</p>
              <ul className="check-list">{previewLines(rows).map((l, i) => <li key={i}>{l}</li>)}</ul>
              {phase === "running" ? (
                <div className="banner warn" role="status"><div>
                  削除しています… {Math.min(idx + 1, rows.length)}/{rows.length} 件目（{elapsedLabel(elapsed)}）
                  {isSlow(elapsed) ? <div>時間がかかっています。閉じても処理は続きます。結果は一覧に反映されます。</div> : <div className="small muted">終わるまでこの画面は閉じられません（90秒を超えると閉じられます）。</div>}
                </div></div>
              ) : null}
            </>
          )}
        </div>
        <footer>
          <div className="foot-l" />
          <div className="foot-r">
            {phase === "confirm" ? (
              <>
                <button className="btn" onClick={onClose}>やめる</button>
                <button className="btn danger" disabled={rows.length === 0} onClick={() => void run()}>削除する</button>
              </>
            ) : <button className="btn" disabled={!canClose} onClick={close}>閉じる</button>}
          </div>
        </footer>
      </div>
    </div>
  );
}
