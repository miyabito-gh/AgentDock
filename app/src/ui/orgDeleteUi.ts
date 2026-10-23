// 組織ワークフローの削除ダイアログの表示ロジック（純粋）。実行: node --test src/ui/orgDeleteUi.test.ts （tscの対象外）
// 時間のかかる削除は、画面が待ち続けない（経過・N/M件目を出し、一定時間後は閉じてよいと案内する）。処理はホスト側で続く。

/** これを超えたら「時間がかかっています」と案内し、ダイアログを閉じられるようにする。 */
export const SLOW_MS = 90_000;

export interface DeleteProgress { done: number; total: number }

/** 「N/M 件目」。済んだ段の次を処理中として数える（全部済んだら M/M）。全体が不明（0）なら出さない。 */
export function progressText(p: DeleteProgress | null): string | null {
  if (!p || p.total <= 0) return null;
  const now = Math.min(p.done + 1, p.total);
  return `${now}/${p.total} 件目`;
}

export function elapsedLabel(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}秒` : `${Math.floor(s / 60)}分${String(s % 60).padStart(2, "0")}秒`;
}

export function isSlow(ms: number): boolean {
  return ms >= SLOW_MS;
}

export const SLOW_NOTICE = "時間がかかっています。処理は続いています。閉じても削除は続きます。後から「削除を続ける…」で再開できます。";

/** 削除（アーカイブ）ボタンを無効にしている理由を、ボタンの横に短く出す文。有効なら null。 */
export function manageOffReason(o: { connected: boolean; finished: boolean; allConfirmed: boolean }): string | null {
  if (!o.connected) return "Codex に接続していません。接続してから操作してください";
  if (!o.finished) return "終わっていないタスクがあります。先に「取り消す」か完了させてください";
  if (!o.allConfirmed) return "状態が確定していないタスクがあります。「状態を取り直す」で確認してください";
  return null;
}
