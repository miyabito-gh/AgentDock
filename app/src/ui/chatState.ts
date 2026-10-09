// チャット単位の表示用状態（子孫込み）。純粋ロジック。送信条件・通知・停止確認の判定には使わない（それらはホスト側の規則）。
// 型だけに依存し、単体テスト（chatState.test.ts）をNodeで直接実行できるようにしている。
import type { AgentState } from "../ipc/types";

export type ChatDisplayKind =
  | "root" // 親の状態をそのまま表示
  | "childFail" // 親が作業中で、子孫に失敗がある
  | "childWorking"; // 親は完了・待機だが、子孫が作業中

export const isActiveState = (st: AgentState): boolean => st === "running" || st === "waiting" || st === "initializing";

/** 親の状態と子孫（親以外）の状態から、表示の種別を決める。 */
export function chatDisplayKind(root: AgentState, descendants: AgentState[]): ChatDisplayKind {
  if (root === "running" && descendants.includes("failed")) return "childFail";
  if (root === "done" || root === "idle") {
    if (descendants.some(isActiveState)) return "childWorking";
  }
  return "root";
}

export const CHAT_DISPLAY_TEXT: Record<Exclude<ChatDisplayKind, "root">, string> = {
  childFail: "作業中・子の失敗あり",
  childWorking: "作業中（子が作業中）",
};

/** 子孫のうち状態不明の件数。表示の注記（「子N件の状態不明」）にだけ使い、親の状態は変えない。 */
export const unknownCount = (descendants: AgentState[]): number => descendants.filter((s) => s === "unknown").length;
export const unknownNote = (n: number): string => `子${n}件の状態不明`;

/** ドックの子カードの並び順: 実行中・対応待ち(0) → 失敗・中断(1) → 完了(2) → 状態不明(3)。同順位は元の順（安定ソート）。 */
export function dockRank(st: AgentState): number {
  if (isActiveState(st)) return 0;
  if (st === "failed" || st === "interrupted") return 1;
  if (st === "unknown") return 3;
  return 2; // done / idle / closed
}
export const sortByDockRank = <T>(items: T[], stateOf: (t: T) => AgentState): T[] =>
  items.map((t, i) => ({ t, i, r: dockRank(stateOf(t)) })).sort((a, b) => a.r - b.r || a.i - b.i).map((x) => x.t);

/** 「確認済みにして隠す」を解く署名。状態・原文ラベル・最新turn・直近活動の時刻が変われば別物（新しい活動として再表示する）。 */
export const unknownSignature = (label: string, latestTurn: string | null, activityAt: number | null): string => `${label}|${latestTurn ?? ""}|${activityAt ?? ""}`;
