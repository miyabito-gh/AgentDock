// 入力欄の送信キーの判定（純粋ロジック）。実行中の「送信」キーの動作は設定（runningInput）で選び、Ctrl＋Shift＋Enter は常にもう一方。
// 実行: node --test src/ui/sendKeys.test.ts （tscの対象外）
export type RunningInputMode = "queue" | "steer";
export type SendAction = "send" | "queue" | "steer";
export interface SendKeyEvent { key: string; ctrlKey: boolean; shiftKey: boolean; isComposing?: boolean }

/** 実行中の「送信」キーの既定の送り方（追加指示か完了後送信か）。 */
export const defaultRunningAction = (m: RunningInputMode): "queue" | "steer" => (m === "steer" ? "steer" : "queue");
/** もう一方（Ctrl＋Shift＋Enter）の送り方。 */
export const otherRunningAction = (m: RunningInputMode): "queue" | "steer" => (m === "steer" ? "queue" : "steer");

/**
 * 入力欄でこのキーが何を送るか。null は送らない（改行・無関係なキー）。
 * - 実行中でなければ、送信キーだけが「送信」。Ctrl＋Shift＋Enter は送らない（追加指示にできる turn がない）。
 * - 実行中は、送信キー＝設定の送り方、Ctrl＋Shift＋Enter＝もう一方。
 */
export function sendActionForKey(e: SendKeyEvent, enterMode: "ctrl" | "enter", running: boolean, mode: RunningInputMode): SendAction | null {
  if (e.key !== "Enter" || e.isComposing) return null;
  if (e.ctrlKey && e.shiftKey) return running ? otherRunningAction(mode) : null;
  const primary = (enterMode === "ctrl" && e.ctrlKey) || (enterMode === "enter" && !e.shiftKey);
  if (!primary) return null;
  return running ? defaultRunningAction(mode) : "send";
}
