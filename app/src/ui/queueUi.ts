// 「完了後に送る依頼」まわりの、画面の出し分け（純粋関数。React に依存しない）。
import type { BackendKind, ChatLocalView, ChatQueue } from "../ipc/types";
import { aiFullName } from "./backendModels.ts";

/** 追加指示にできない理由（できるときは null）。追加指示はモデル・権限・作業フォルダの変更には使えず、実行中のturnにだけ送れる。 */
export function steerBlockReason(running: boolean, changed: boolean, local: ChatLocalView | undefined, backend: BackendKind = "codex"): string | null {
  if (backend === "antigravity") return `${aiFullName("antigravity")}は追加指示に対応していません（完了後に送信を使います）`;
  if (!running) return "実行中のやりとりがないため、追加指示にはできません";
  if (changed) return "モデル・推論の強さ・速度を変更中のため、追加指示にはできません（変更は次の送信から適用されます）";
  if (local?.nextCwd) return "作業フォルダの変更が予定されているため、追加指示にはできません";
  return null;
}

/** キュー項目の「追加指示」ボタンを出すか。追加指示に対応しない担当AI（Antigravity）には最初から出さない（押しても必ず無効のため）。 */
export const steerButtonOffered = (backend: BackendKind): boolean => backend !== "antigravity";

/** 追加指示のボタンを無効のまま押したときに出す説明（無言にしない）。 */
export const steerBlockedNote = (reason: string): string => `追加指示として送れません: ${reason}`;

/**
 * 親（このチャット自身）が原因の保留で、「このチャットを再開して送る」（利用者操作の再開）で解けるか。
 * - 受信（ライブ監視）が戻っていない（notLive）対象に親自身が含まれる。
 * - 親の状態を取得できない（stateUnknown で対象が空、または親だけが対象で鮮度が live でない）。子孫の探索が未完了の保留は親が原因とは限らないので出さない。
 * 子孫だけが原因の保留（状態の再確認・確認して今すぐ送るで扱う）、停止中（別バナーの「キューを再開」）では出さない。
 */
export function parentResumeOffer(q: Pick<ChatQueue, "run" | "hold" | "chat">): boolean {
  const h = q.hold;
  if (q.run.kind !== "active" || !h) return false;
  const root = q.chat.id;
  if (h.kind === "notLive") return h.targets.some((t) => t.agent.id === root);
  if (h.kind === "stateUnknown") {
    if (h.descendantScanIncomplete) return false;
    if (!h.targets.length) return true;
    return h.targets.every((t) => t.agent.id === root && t.freshness !== "live");
  }
  return false;
}
