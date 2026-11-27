// 「完了後に送る依頼」まわりの、画面の出し分け（純粋関数。React に依存しない）。
import type { BackendKind, ChatLocalView, ChatQueue, HoldTarget } from "../ipc/types";

/** 追加指示にできない理由（できるときは null）。追加指示はモデル・権限・作業フォルダの変更には使えず、実行中のturnにだけ送れる。 */
export function steerBlockReason(running: boolean, changed: boolean, local: ChatLocalView | undefined, backend: BackendKind = "codex"): string | null {
  if (backend !== "codex") return "旧チャットには送信できません";
  if (!running) return "実行中のやりとりがないため、追加指示にはできません";
  if (changed) return "モデル・推論の強さ・速度を変更中のため、追加指示にはできません（変更は次の送信から適用されます）";
  if (local?.nextCwd) return "作業フォルダの変更が予定されているため、追加指示にはできません";
  return null;
}

/** キュー項目の「追加指示」ボタンを出すか。追加指示に対応しないチャットには最初から出さない（押しても必ず無効のため）。 */
export const steerButtonOffered = (backend: BackendKind): boolean => backend === "codex";

/** 追加指示のボタンを無効のまま押したときに出す説明（無言にしない）。 */
export const steerBlockedNote = (reason: string): string => `追加指示として送れません: ${reason}`;

/**
 * 親（このチャット自身）が原因の保留で、「このチャットを再開して送る」（利用者操作の再開）で解けるか。
 * - 受信（ライブ監視）が戻っていない（notLive）対象に親自身が含まれる。
 * - 親の状態を取得できない（stateUnknown で対象が空）、または親だけが対象（親が live でも、状態が不明・終了のときは、再開で状態を取り直す）。
 *   子孫の探索が未完了の保留は親が原因とは限らないので出さない。
 * 子孫だけが原因の保留（状態の再確認・確認して今すぐ送るで扱う）、停止中（別バナーの「キューを再開」）では出さない。
 * ホスト `rules::queue::parent_state_hold`（親が live のときの取り直し）と同じ判定。
 */
export function parentResumeOffer(q: Pick<ChatQueue, "run" | "hold" | "chat">): boolean {
  const h = q.hold;
  if (q.run.kind !== "active" || !h) return false;
  const root = q.chat.id;
  if (h.kind === "notLive") return h.targets.some((t) => t.agent.id === root);
  if (h.kind === "stateUnknown") {
    if (h.descendantScanIncomplete) return false;
    if (!h.targets.length) return true;
    return h.targets.every((t) => t.agent.id === root);
  }
  return false;
}

const WORKING_STATES = new Set(["running", "waiting", "initializing"]);

/**
 * 「作業中と記録されたまま、履歴だけ（今回の起動では受信していない）」の子孫（前回の起動の途中で止まった子など）。キューは、受信できている作業中の子孫
 * （切断・復帰の照合中を含む）は待つが、これらは終わりを観測できないので「受信が戻っていない」保留（notLive）にして出口を出す。親は含めない（親は再開で解く）。
 */
export function staleWorkingDescendants(q: Pick<ChatQueue, "run" | "hold" | "chat">): HoldTarget[] {
  const h = q.hold;
  if (q.run.kind !== "active" || !h || h.kind !== "notLive") return [];
  return h.targets.filter((t) => t.agent.id !== q.chat.id && WORKING_STATES.has(t.state));
}

/**
 * 「状態を再確認」（子孫の履歴を読み直すだけ）が役に立つか。作業中と記録されたまま live でない子孫だけが原因のときは、読み直しても
 * 記録（作業中）が変わらないので出さない（押しても何も変わらないボタンを出さない）。そのときの出口は「確認して今すぐ送る」。
 */
export function recheckOffered(q: Pick<ChatQueue, "run" | "hold" | "chat">): boolean {
  const h = q.hold;
  if (q.run.kind !== "active" || !h || (h.kind !== "stateUnknown" && h.kind !== "notLive")) return false;
  if (h.kind === "notLive" && h.targets.length > 0 && h.targets.every((t) => t.agent.id !== q.chat.id && WORKING_STATES.has(t.state))) return false;
  return true;
}
