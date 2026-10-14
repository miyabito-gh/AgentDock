// 送信ボタンを押してから受理（または未受理・受理不明）が分かるまでの、会話内の「送信中の依頼」の表示状態。
// 純粋ロジックのみ（画面は PendingSend.tsx）。新しい状態機械ではなく、ホストの SendAttempt の結果を表示用に写す。
// 実行: node --test src/ui/pendingSend.test.ts
import type { SendAttempt, SendPhase } from "../ipc/types";

/** この秒数（ミリ秒）を超えて応答がないときだけ、経過秒と段階を出す。 */
export const SLOW_AFTER_MS = 3000;
/** 受理後、会話の本文に現れないまま残す最長（これを過ぎたら行を外す）。 */
export const ACCEPTED_HOLD_MS = 10000;

export type PendingOutcome =
  | { kind: "accepted"; turnId: string }
  | { kind: "rejected"; attemptId: string; message: string }
  /** 受理を確認できない。照合で確定するまで再送しない（attemptId で後からの確定を受ける）。 */
  | { kind: "unknown"; attemptId: string }
  /** ホストが送る前に止めた（送信要求そのものが失敗した）。何も送っていない。 */
  | { kind: "failed"; message: string };

export interface PendingSend {
  id: string;
  text: string;
  startedAt: number;
  /** 追加指示（実行中のturnへ）の送信か。 */
  steer: boolean;
  /** ホストが今行っている段階（取れていなければ null）。 */
  phase: SendPhase | null;
  /** null＝応答待ち。 */
  outcome: PendingOutcome | null;
  /** 「もう一度送る」で再送できる間は true（画面側が付ける）。再送と「入力欄へ戻す」を両立させない（二重送信を避ける）。 */
  retryable?: boolean;
}

export type PendingView =
  | { kind: "sending"; slow: boolean; elapsedSec: number; stage: string | null }
  | { kind: "accepted" }
  | { kind: "unsent"; reason: string; canRestore: boolean }
  | { kind: "unknown"; canRestore: false };

/** 段階の表示文。実際に今行っている段階だけ（取れていない段階は null）。 */
export function stageLabel(phase: SendPhase | null): string | null {
  switch (phase) {
    case "preparing": return "送信の準備をしています";
    case "resuming": return "会話を再開しています";
    case "awaitingAcceptance": return "Codex が受理するのを待っています";
    default: return null;
  }
}

export function pendingView(p: PendingSend, now: number): PendingView {
  const o = p.outcome;
  if (!o) {
    const elapsed = Math.max(0, now - p.startedAt);
    const slow = elapsed >= SLOW_AFTER_MS;
    return { kind: "sending", slow, elapsedSec: Math.floor(elapsed / 1000), stage: slow ? stageLabel(p.phase) : null };
  }
  switch (o.kind) {
    case "accepted": return { kind: "accepted" };
    case "rejected": return { kind: "unsent", reason: o.message, canRestore: !p.retryable };
    case "failed": return { kind: "unsent", reason: o.message, canRestore: true };
    case "unknown": return { kind: "unknown", canRestore: false };
  }
}

/** ホストの送信結果（SendAttempt）を表示用の結果へ写す。まだ送信中（sending）なら null。 */
export function outcomeOfAttempt(a: SendAttempt): PendingOutcome | null {
  switch (a.state.kind) {
    case "sending": return null;
    case "accepted": return { kind: "accepted", turnId: a.state.turn.turnId };
    case "rejected": return { kind: "rejected", attemptId: a.attemptId, message: a.state.message };
    case "acceptanceUnknown": return { kind: "unknown", attemptId: a.attemptId };
    case "notFoundAfterReconcile": return { kind: "rejected", attemptId: a.attemptId, message: "履歴に受理の痕跡がありません" };
  }
}

/** 受理不明だった行へ、後の照合の結果（sendUpdated）を反映する。同じ試行の確定だけを受ける。 */
export function reconcilePending(list: PendingSend[], a: SendAttempt): PendingSend[] {
  const next = outcomeOfAttempt(a);
  if (!next || next.kind === "unknown") return list;
  let changed = false;
  const out = list.map((p) => {
    if (p.outcome?.kind === "unknown" && p.outcome.attemptId === a.attemptId) { changed = true; return { ...p, outcome: next }; }
    return p;
  });
  return changed ? out : list;
}

/** 再送（押したときだけ）の結果を、元の仮の行へ結び付ける。受理なら受理表示へ、受理不明なら不明へ、再び拒否なら新しい試行の未送信へ。 */
export function rebindPending(list: PendingSend[], oldAttemptId: string, a: SendAttempt): PendingSend[] {
  const next = outcomeOfAttempt(a);
  if (!next) return list;
  let changed = false;
  const out = list.map((p) => {
    if (p.outcome?.kind === "rejected" && p.outcome.attemptId === oldAttemptId) { changed = true; return { ...p, outcome: next }; }
    return p;
  });
  return changed ? out : list;
}

/** 応答待ちの行の段階を更新する（応答待ちが無ければ何もしない）。 */
export function setPhase(list: PendingSend[], phase: SendPhase): PendingSend[] {
  if (!list.some((p) => p.outcome === null)) return list;
  return list.map((p) => (p.outcome === null ? { ...p, phase } : p));
}

/** 試行の保持。sendUpdated（確定）が戻り値（Sending）より先に届いても、同じ attemptId の確定を送信中へ戻さない。 */
export function mergeAttempt(prev: SendAttempt | undefined, next: SendAttempt): SendAttempt {
  if (prev && prev.attemptId === next.attemptId && prev.state.kind !== "sending" && next.state.kind === "sending") return prev;
  return next;
}

/** 二重に送らないため、応答待ちの送信があるか。 */
export const hasInFlight = (list: PendingSend[]): boolean => list.some((p) => p.outcome === null);

type TurnLite = { key: { turnId: string }; entries: Array<{ kind: { kind: string }; text: { kind: string; value?: string } }> };

/** 受理された依頼が、会話の本文（履歴・live通知）に現れたか。現れたら仮の行は外す（二重表示しない）。
 *  新しいturnはturn IDで判定する。追加指示は既存のturnに入るので、そのturn内に同じ文の依頼があるかで判定する。 */
export function appearsInTurns(p: PendingSend, turns: TurnLite[]): boolean {
  if (p.outcome?.kind !== "accepted") return false;
  const id = p.outcome.turnId;
  const t = turns.find((x) => x.key.turnId === id);
  if (!t) return false;
  if (!p.steer) return true;
  const want = p.text.trim();
  return t.entries.some((e) => e.kind.kind === "userMessage" && e.text.kind === "value" && (e.text.value ?? "").trim() === want);
}
