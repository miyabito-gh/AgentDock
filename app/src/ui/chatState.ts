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

/** Codexの原状態ラベルに添える説明（既知のものだけ。他は従来どおり「根拠なし」）。原文のラベルと「根拠なし」の意味は残す。 */
const RAW_STATE_NOTE: Record<string, string> = {
  notLoaded: "Codexが読み込んでいない状態。完了・失敗の根拠なし",
};
/** 「履歴で再確認」の結果の1文。件数は実際に履歴で終端を確認できた数だけ（読めない・確認できないものを成功に数えない）。 */
export function recheckSummary(outcomes: Array<{ kind: "terminalFound" | "stillUnknown" | "unreadable" | "skipped" }>): string {
  const found = outcomes.filter((o) => o.kind === "terminalFound").length;
  const unreadable = outcomes.filter((o) => o.kind === "unreadable").length;
  const rest = outcomes.length - found;
  const tail = unreadable > 0 ? `（うち${unreadable}件は履歴を読めませんでした）` : "";
  if (found > 0) return `${found}件の終端を履歴で確認しました。${rest > 0 ? `残り${rest}件は確認できませんでした（状態不明のまま）${tail}。` : ""}`;
  return `履歴では確認できませんでした（状態不明のまま）${tail}。`;
}

export const rawStateText =(label: string): string => `原状態: ${label}（${RAW_STATE_NOTE[label] ?? "根拠なし"}）`;

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

/** 折りたたみグループの振り分け。状態不明の子孫（自分以下も全て不明のもの）のうち、履歴で終端確認済みは "history"、未確認は "unknown"。それ以外は木に残す。 */
export const parkGroup = (state: AgentState, outcome: HistoryOutcome | null): "tree" | "history" | "unknown" =>
  state !== "unknown" ? "tree" : outcome ? "history" : "unknown";

/** 履歴のみで確認した子の内訳。 */
export function historyBreakdown(outcomes: HistoryOutcome[]): { completed: number; failed: number; interrupted: number; noTurns: number; attention: boolean } {
  const n = (o: HistoryOutcome) => outcomes.filter((x) => x === o).length;
  const failed = n("failed"); const interrupted = n("interrupted");
  return { completed: n("completed"), failed, interrupted, noTurns: n("noTurns"), attention: failed + interrupted > 0 };
}
export function historyBreakdownText(b: ReturnType<typeof historyBreakdown>): string {
  return [`完了${b.completed}`, `失敗${b.failed}`, `中断${b.interrupted}`, ...(b.noTurns ? [`turnなし${b.noTurns}`] : [])].join("／");
}

export const HISTORY_OUTCOME_TEXT: Record<HistoryOutcome, string> = {
  completed: "履歴で完了を確認",
  failed: "履歴で失敗を確認",
  interrupted: "履歴で中断を確認",
  noTurns: "履歴でturnなしを確認",
};

/** 「確認済みにして隠す」を解く署名。状態・原文ラベル・最新turn・直近活動の時刻が変われば別物（新しい活動として再表示する）。 */
export const unknownSignature = (label: string, latestTurn: string | null, activityAt: number | null): string => `${label}|${latestTurn ?? ""}|${activityAt ?? ""}`;
