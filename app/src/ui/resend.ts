// 編集して再送・再生成（M50、DESIGN_P5 §5）の可否。純粋関数。型だけに依存し、Nodeで直接テストできる。
// 理由の順と文言は、ホストの `rules::thread_ops::resend_blocker` と揃える。最終判定はホスト（ここは押す前の案内）。
//   能力 → 接続 → 削除保留 → 外部実行中 → 停止未確認 → 作業中 → 状態不明 → 表示が古い → 追加指示 → 依頼の文なし(UIのみ) → 直前の終端未確認
import type { ForkPurpose, TurnEnd } from "../ipc/types";

/** 履歴のturn1件の要約（メインのエージェントのturnを古い順に並べる）。 */
export interface TurnLite {
  turnId: string;
  /** null＝終端未確認（進行中・切断で不明）。 */
  end: TurnEnd | null;
  /** このturnの依頼（ユーザー発話）の件数。2件以上は実行中の追加指示を含む。 */
  userCount: number;
}

/** チャット単位の判断材料（UIが snapshot から作る）。 */
export interface ResendCtx {
  /** 分岐の能力が使えないときの理由（使えるなら null。`availability` と同じ文言）。 */
  capProblem: string | null;
  connected: boolean;
  deletePending: boolean;
  /** 外部で実行中の可能性（外部作成で live でない）。 */
  externalRunning: boolean;
  stopUnconfirmed: boolean;
  /** メイン・子孫に作業中・対応待ちがある。 */
  busy: boolean;
  /** メインの状態が不明で、履歴で終端を確認できていない。 */
  rootUnknown: boolean;
  turns: TurnLite[];
}

/** 対象。edit は依頼の吹き出し（userIndex＝そのturn内で何番目の依頼か）、regenerate は応答。 */
export interface ResendTarget {
  turnId: string;
  purpose: Exclude<ForkPurpose, "fork">;
  /** 編集の対象が、turn内で何番目の依頼か（0始まり）。再生成では null。 */
  userIndex: number | null;
  /** 依頼の文（取得できていないとき null）。 */
  requestText: string | null;
}

export type ResendPlan =
  | { kind: "blocked"; reason: string }
  /** 直前のturnまでを分岐して送る（`resend_as_fork`）。 */
  | { kind: "fork"; throughTurn: string; targetTurn: string; turnNo: number }
  /** 最初のturn。分岐せず、新しいチャットの入力欄へ入れるだけ。 */
  | { kind: "first"; targetTurn: string; turnNo: number };

const blocked = (reason: string): ResendPlan => ({ kind: "blocked", reason });

export const REASON = {
  notConnected: "Codex に接続していません",
  deletePending: "削除保留中のチャットです",
  externalRunning: "外部で実行中か確認できないため使えません（再開の案内から確認してください）",
  stopUnconfirmed: "停止を確認できていないため使えません",
  busy: "実行中は使えません。完了するか、中断と停止の確認の後に使えます",
  stateUnknown: "状態を確認できないため使えません（「状態を再照合」で確認できます）",
  stale: "表示が古いため、会話を読み直してからやり直してください",
  multiple: "追加指示を含むturnは、編集・再生成できません",
  followUp: "追加指示は編集して送り直せません",
  noText: "このturnの依頼の文を取得できていないため使えません（会話を読み直してください）",
  previousNotEnded: "直前のturnの終了を確認できていません",
} as const;

/** 最初に当たった理由を返す。ブロックされなければ、分岐して送る／最初のturnの経路のどちらかを返す。 */
export function resendAvailability(ctx: ResendCtx, target: ResendTarget): ResendPlan {
  if (ctx.capProblem) return blocked(ctx.capProblem);
  if (!ctx.connected) return blocked(REASON.notConnected);
  if (ctx.deletePending) return blocked(REASON.deletePending);
  if (ctx.externalRunning) return blocked(REASON.externalRunning);
  if (ctx.stopUnconfirmed) return blocked(REASON.stopUnconfirmed);
  if (ctx.busy) return blocked(REASON.busy);
  if (ctx.rootUnknown) return blocked(REASON.stateUnknown);
  const i = ctx.turns.findIndex((t) => t.turnId === target.turnId);
  if (i < 0) return blocked(REASON.stale);
  const t = ctx.turns[i];
  if (target.purpose === "editResend" && target.userIndex !== null && target.userIndex > 0) return blocked(REASON.followUp);
  if (t.userCount >= 2) return blocked(REASON.multiple);
  if (target.requestText === null) return blocked(REASON.noText);
  if (i === 0) return { kind: "first", targetTurn: t.turnId, turnNo: 1 };
  const prev = ctx.turns[i - 1];
  if (prev.end === null || !prev.turnId) return blocked(REASON.previousNotEnded);
  return { kind: "fork", throughTurn: prev.turnId, targetTurn: t.turnId, turnNo: i + 1 };
}

/** 結果のタグ・題名に使う目的の表示名。 */
export const PURPOSE_TAG: Record<ForkPurpose, string> = { fork: "分岐", editResend: "編集から", regenerate: "再生成" };
/** 分岐先の名前の接尾（ホストの名付けと同じ）。 */
export const PURPOSE_SUFFIX: Record<Exclude<ForkPurpose, "fork">, string> = { editResend: "（編集）", regenerate: "（再生成）" };
