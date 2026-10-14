// 新しいチャットの作成中の仮行（左一覧の先頭）の状態。純粋ロジックのみ（画面は CreatingRows.tsx）。
// 作成は取り消せない操作なので、押し直しは促さない。失敗でも不明でも自動再送しない。
// 実行: node --test src/ui/creating.test.ts
import type { IpcErrorCode } from "../ipc/types";

export type CreatingPhase = "worktree" | "starting";

export type CreatingResult =
  /** ホストが拒否した（作られていない）。同じ値でダイアログを開き直せる。 */
  | { kind: "failed"; message: string }
  /** 作られたか分からない。一覧の更新で確認するだけ（押し直しを促さない）。 */
  | { kind: "unknown"; message: string };

export interface CreatingChat<I = unknown> {
  tempId: string;
  label: string;
  startedAt: number;
  phase: CreatingPhase;
  /** 入力し直すための元の値（失敗のときだけ使う）。 */
  input: I;
  result: CreatingResult | null;
  /** 最初の依頼の本文（不明のときにコピーで取り戻す用。送らない）。 */
  body?: string | null;
  /** 「一覧を更新」で確認した結果（見つからなかったときだけ残す。作られていないとは断定しない）。 */
  checkedNote?: string;
}

/** 一覧を更新して、作成開始以降に作られたチャットがあるか。cwd 指定があれば同じフォルダのものだけ。時刻が取れないチャットは数えない。 */
export function foundSince(
  items: Array<{ createdAt: { kind: string; value?: number }; cwd: { kind: string; value?: string } }>,
  startedAt: number,
  cwd: string | null,
): number {
  const norm = (s: string) => s.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  const want = cwd ? norm(cwd) : null;
  return items.filter((c) => {
    if (c.createdAt.kind !== "value" || typeof c.createdAt.value !== "number" || c.createdAt.value < startedAt - 5000) return false;
    if (want === null) return true;
    return c.cwd.kind === "value" && typeof c.cwd.value === "string" && norm(c.cwd.value) === want;
  }).length;
}

export const setCheckedNote = <I,>(list: CreatingChat<I>[], id: string, note: string): CreatingChat<I>[] =>
  list.some((c) => c.tempId === id) ? list.map((c) => (c.tempId === id ? { ...c, checkedNote: note } : c)) : list;

export function phaseText(phase: CreatingPhase): string {
  return phase === "worktree" ? "worktreeを作成しています" : "チャットを作成しています";
}

/** 仮行の表示名: 最初の依頼の先頭、なければフォルダ名、なければ「新しいチャット」。 */
export function creatingLabel(firstMessage: string | null, cwd: string | null): string {
  const line = firstMessage?.split(/\r?\n/).find((l) => l.trim())?.trim();
  if (line) return line.length > 40 ? `${line.slice(0, 40)}…` : line;
  const base = cwd?.split(/[\\/]/).filter(Boolean).pop();
  return base || "新しいチャット";
}

/** IPC エラーの種類から、作られていないと言えるか（failed）、分からないか（unknown）を決める。 */
export function resultOfError(code: IpcErrorCode, message: string): CreatingResult {
  switch (code) {
    case "notConnected": case "unsupported": case "rejected": case "invalidArgs": case "notFound": case "blocked":
      return { kind: "failed", message };
    default: // outcomeUnknown・io・protocol
      return { kind: "unknown", message };
  }
}

export const addCreating = <I,>(list: CreatingChat<I>[], c: CreatingChat<I>): CreatingChat<I>[] => [c, ...list];

const patch = <I,>(list: CreatingChat<I>[], id: string, f: (c: CreatingChat<I>) => CreatingChat<I>): CreatingChat<I>[] =>
  list.some((c) => c.tempId === id) ? list.map((c) => (c.tempId === id ? f(c) : c)) : list;

export const setCreatingPhase = <I,>(list: CreatingChat<I>[], id: string, phase: CreatingPhase): CreatingChat<I>[] =>
  patch(list, id, (c) => (c.result ? c : { ...c, phase }));

/** 成功・閉じる・一覧更新後の削除。 */
export const removeCreating = <I,>(list: CreatingChat<I>[], id: string): CreatingChat<I>[] =>
  list.some((c) => c.tempId === id) ? list.filter((c) => c.tempId !== id) : list;

export const resolveCreating = <I,>(list: CreatingChat<I>[], id: string, result: CreatingResult): CreatingChat<I>[] =>
  patch(list, id, (c) => ({ ...c, result }));
