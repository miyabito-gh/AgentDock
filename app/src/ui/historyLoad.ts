// 会話ログの取得状態（チャットごと）。表示だけの状態で、保存・ホストには渡さない。
export type HistLoad =
  | { kind: "loading"; /** 取り直しのとき、すでに表示している内容を消さないための印 */ retry: boolean }
  | { kind: "failed"; reason: string }
  | { kind: "ok" };

/** 取得の開始。成功済みでも取り直しなら読み込み中にする（既存の表示は残す）。 */
export const histStart = (prev: HistLoad | undefined): HistLoad => ({ kind: "loading", retry: prev?.kind === "failed" || prev?.kind === "ok" });
export const histOk = (): HistLoad => ({ kind: "ok" });
export const histFail = (message: string | null | undefined): HistLoad => ({ kind: "failed", reason: failReason(message) });

/** 失敗理由の文言。ホストの失敗内容が無いときは「原因不明」。 */
export function failReason(message: string | null | undefined): string {
  const m = (message ?? "").trim();
  return m === "" ? "原因不明" : m;
}

/** 失敗の表示文。タイムアウトの可能性はいつも添える（理由の文面だけでは判別できないため）。 */
export function failText(reason: string): string {
  return `会話の履歴を取得できませんでした: ${reason}（タイムアウトの可能性）`;
}

export type HistView = { kind: "none" } | { kind: "loading" } | { kind: "failed"; text: string };

/**
 * 中央ペインに出す取得状態の表示。
 * - 失敗は常に出す（空チャットの空表示とは別物。表示中のturnsがあっても消さず、失敗だけ追記する）。
 * - 読み込み中は、まだ1件も表示するものが無い初回だけ出す（取り直し中・表示済みは邪魔しない）。
 * - 状態が無い（取得を始めていない）・成功は何も出さない。
 */
export function histView(s: HistLoad | undefined, hasTurns: boolean): HistView {
  if (!s) return { kind: "none" };
  if (s.kind === "failed") return { kind: "failed", text: failText(s.reason) };
  if (s.kind === "loading") return s.retry && hasTurns ? { kind: "none" } : { kind: "loading" };
  return { kind: "none" };
}
