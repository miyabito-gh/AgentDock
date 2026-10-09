// チャット単位の表示用状態（子孫込み）。純粋ロジック。送信条件・通知・停止確認の判定には使わない（それらはホスト側の規則）。
// 型だけに依存し、単体テスト（chatState.test.ts）をNodeで直接実行できるようにしている。
import type { AgentState, HistoryConfirmationEntry, HistoryOutcome } from "../ipc/types";

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
/** `outcome` は状態不明のものを履歴で終端確認できたときの結果。失敗・中断は注意が要る位置、完了・turnなしは完了の位置（状態は書き換えない）。 */
export function dockRank(st: AgentState, outcome?: HistoryOutcome | null): number {
  if (isActiveState(st)) return 0;
  if (st === "failed" || st === "interrupted") return 1;
  if (st === "unknown") {
    if (outcome === "failed" || outcome === "interrupted") return 1;
    if (outcome === "completed" || outcome === "noTurns") return 2;
    return 3;
  }
  return 2; // done / idle / closed
}
export const sortByDockRank = <T>(items: T[], stateOf: (t: T) => AgentState, outcomeOf?: (t: T) => HistoryOutcome | null): T[] =>
  items.map((t, i) => ({ t, i, r: dockRank(stateOf(t), outcomeOf?.(t)) })).sort((a, b) => a.r - b.r || a.i - b.i).map((x) => x.t);

/** 履歴での終端確認が、いまの状態不明のエージェントに有効か。状態不明でない・新しいturnが来た場合は無効（状態は書き換えない）。 */
export function validOutcome(state: AgentState, latestTurn: string | null, conf: HistoryConfirmationEntry | undefined): HistoryOutcome | null {
  if (!conf || state !== "unknown") return null;
  if (conf.outcome === "noTurns") return latestTurn === null ? "noTurns" : null;
  return latestTurn === null || latestTurn === conf.turn ? conf.outcome : null;
}

export const HISTORY_OUTCOME_TEXT: Record<HistoryOutcome, string> = {
  completed: "履歴で完了を確認",
  failed: "履歴で失敗を確認",
  interrupted: "履歴で中断を確認",
  noTurns: "履歴でturnなしを確認",
};

/** 「確認済みにして隠す」を解く署名。状態・原文ラベル・最新turn・直近活動の時刻が変われば別物（新しい活動として再表示する）。 */
export const unknownSignature = (label: string, latestTurn: string | null, activityAt: number | null): string => `${label}|${latestTurn ?? ""}|${activityAt ?? ""}`;
