// チャット削除ダイアログの表示文（純粋）。実行: node --test src/ui/chatDeleteUi.test.ts （tscの対象外）
// 流儀は組織ワークフローの削除（orgDeleteUi.ts）と同じ: 経過秒を出し、90秒を超えたら閉じてよいと案内する。ホストにチャット削除の進行イベントは無いため、N/M 件目は出さない。
import { elapsedLabel, isSlow, SLOW_MS } from "./orgDeleteUi.ts";

/** ダイアログ1行目。 */
export const DELETE_HEADLINE = "このチャットを削除します";

/** 元に戻せないことと、アーカイブという代わりの手段（一覧から隠すだけで、後から戻せる）。 */
export const DELETE_IRREVERSIBLE = "削除すると会話の記録は元に戻せません。残しておきたいときは「アーカイブ」を使うと、一覧から隠すだけで後から戻せます。";

/** 削除ボタンの実行中の表示。 */
export const deleteRunningLabel = (ms: number): string => `削除しています…（${elapsedLabel(ms)}）`;

/** 実行中の説明。90秒までは閉じられず、超えたら閉じてよい（処理は続く）。 */
export function deleteRunningNotice(ms: number): string {
  return isSlow(ms)
    ? "時間がかかっています。閉じても処理は続きます。結果は一覧に反映され、終わらなかった分はもう一度「削除」を開くと続けられます。"
    : `削除しています。終わるまでこの画面は閉じられません（${SLOW_MS / 1000}秒を超えると閉じられます。閉じても処理は続きます）。`;
}

/** 実行中に閉じられない間か。 */
export const deleteLocked = (running: boolean, ms: number): boolean => running && !isSlow(ms);

/** アーカイブ完了のトースト。元に戻す手段（一覧のアーカイブ）を添える。 */
export const archivedToast = (working: boolean): string =>
  working
    ? "アーカイブしました。作業は続きます。Codex への反映は、作業が終わり停止を確認してから行います。元に戻すには、一覧の下の「アーカイブ」から解除してください。"
    : "アーカイブしました。元に戻すには、一覧の下の「アーカイブ」から解除してください。";
