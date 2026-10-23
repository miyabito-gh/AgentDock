// 会話ログの取得状態（チャットごと）。表示だけの状態で、保存・ホストには渡さない。
export type HistLoad =
  | { kind: "loading"; /** 取り直しのとき、すでに表示している内容を消さないための印 */ retry: boolean }
  | { kind: "failed"; reason: string }
  /** 最初の送信がまだ無い会話（想定内の応答。失敗ではない）。 */
  | { kind: "empty" }
  | { kind: "ok" };

/** 取得の開始。成功済みでも取り直しなら読み込み中にする（既存の表示は残す）。 */
export const histStart = (prev: HistLoad | undefined): HistLoad => ({ kind: "loading", retry: prev?.kind === "failed" || prev?.kind === "ok" });
export const histOk = (): HistLoad => ({ kind: "ok" });
export const histFail = (message: string | null | undefined): HistLoad => (isNotMaterialized(message) ? { kind: "empty" } : { kind: "failed", reason: failReason(message) });

/** まだ最初の送信が無い thread の履歴取得エラー（失敗ではなく、会話が空という意味）。 */
export const isNotMaterialized = (message: string | null | undefined): boolean => (message ?? "").toLowerCase().includes("not materialized");

/** 失敗理由の文言。ホストの失敗内容が無いときは「原因不明」。 */
export function failReason(message: string | null | undefined): string {
  const m = (message ?? "").trim();
  return m === "" ? "原因不明" : m;
}

/** 理由がタイムアウトを示しているか。 */
export const looksLikeTimeout = (reason: string): boolean => /timeout|timed out|タイムアウト/i.test(reason);

/** 失敗の表示文。「タイムアウトの可能性」は、理由がタイムアウトを示すときだけ添える。 */
export function failText(reason: string): string {
  return `会話の履歴を取得できませんでした: ${reason}${looksLikeTimeout(reason) ? "（タイムアウトの可能性）" : ""}`;
}

export type HistView = { kind: "none" } | { kind: "loading" } | { kind: "failed"; text: string } | { kind: "empty"; text: string };

/**
 * 中央ペインに出す取得状態の表示。
 * - 失敗は常に出す（空チャットの空表示とは別物。表示中のturnsがあっても消さず、失敗だけ追記する）。
 * - 読み込み中は、まだ1件も表示するものが無い初回だけ出す（取り直し中・表示済みは邪魔しない）。
 * - 状態が無い（取得を始めていない）・成功は何も出さない。
 */
export function histView(s: HistLoad | undefined, hasTurns: boolean): HistView {
  if (!s) return { kind: "none" };
  if (s.kind === "failed") return { kind: "failed", text: failText(s.reason) };
  if (s.kind === "empty") return hasTurns ? { kind: "none" } : { kind: "empty", text: "まだ会話がありません" };
  if (s.kind === "loading") return s.retry && hasTurns ? { kind: "none" } : { kind: "loading" };
  return { kind: "none" };
}
