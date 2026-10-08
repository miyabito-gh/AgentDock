// チャット単位の表示用状態（子孫込み）。純粋ロジック。送信条件・通知・停止確認の判定には使わない（それらはホスト側の規則）。
// 型だけに依存し、単体テスト（chatState.test.ts）をNodeで直接実行できるようにしている。
import type { AgentState } from "../ipc/types";

export type ChatDisplayKind =
  | "root" // 親の状態をそのまま表示
  | "childFail" // 親が作業中で、子孫に失敗がある
  | "childWorking" // 親は完了・待機だが、子孫が作業中
  | "childUnknown"; // 親は完了・待機だが、子孫の状態が不明（完了へ変換しない）

export const isActiveState = (st: AgentState): boolean => st === "running" || st === "waiting" || st === "initializing";

/** 親の状態と子孫（親以外）の状態から、表示の種別を決める。 */
export function chatDisplayKind(root: AgentState, descendants: AgentState[]): ChatDisplayKind {
  if (root === "running" && descendants.includes("failed")) return "childFail";
  if (root === "done" || root === "idle") {
    if (descendants.some(isActiveState)) return "childWorking";
    if (descendants.includes("unknown")) return "childUnknown";
  }
  return "root";
}

export const CHAT_DISPLAY_TEXT: Record<Exclude<ChatDisplayKind, "root">, string> = {
  childFail: "作業中・子の失敗あり",
  childWorking: "作業中（子が作業中）",
  childUnknown: "確認中（子の状態不明）",
};
