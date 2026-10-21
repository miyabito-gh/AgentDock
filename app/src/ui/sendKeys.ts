// 入力欄の送信キーの判定（純粋ロジック）。実行中の「送信」キーの動作は設定（runningInput）で選び、Ctrl＋Shift＋Enter は常にもう一方。
// 実行: node --test src/ui/sendKeys.test.ts （tscの対象外）
/** 送信キー（AppSettings.sendKey の画面側の形）。ctrl＝Ctrl＋Enter のみ送信、enter＝Enter または Ctrl＋Enter で送信（Shift＋Enter は改行）。 */
export type EnterMode = "ctrl" | "enter";
export const enterModeFromSetting = (k: "ctrlEnter" | "enter" | undefined): EnterMode => (k === "ctrlEnter" ? "ctrl" : "enter");
export const enterModeToSetting = (m: EnterMode): "ctrlEnter" | "enter" => (m === "ctrl" ? "ctrlEnter" : "enter");
/** 入力欄に出す送信キーの説明。 */
export const enterHint = (m: EnterMode): string => (m === "ctrl" ? "Ctrl＋Enter で送信、Enter で改行" : "Enter または Ctrl＋Enter で送信、Shift＋Enter で改行");
export const sendKeyName = (m: EnterMode): string => (m === "ctrl" ? "Ctrl＋Enter" : "Enter");

/** 実行中の入力（AppSettings.runningInput）。queue＝完了後に送信、steer＝追加指示で送る。 */
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
export function sendActionForKey(e: SendKeyEvent, enterMode: EnterMode, running: boolean, mode: RunningInputMode): SendAction | null {
  if (e.key !== "Enter" || e.isComposing) return null;
  if (e.ctrlKey && e.shiftKey) return running ? otherRunningAction(mode) : null;
  const primary = (enterMode === "ctrl" && e.ctrlKey) || (enterMode === "enter" && !e.shiftKey);
  if (!primary) return null;
  return running ? defaultRunningAction(mode) : "send";
}

/**
 * 実行中の送り分けのない入力欄（side相談・編集して再送・組織の割り込み指示）で、このキーが送信か。
 * ctrl 方式は Ctrl（または Meta）＋Enter のみ。enter 方式は Enter（Shift なし）または Ctrl（Meta）＋Enter。Shift＋Enter は改行。変換中は送らない。
 */
export function isSendKey(e: { key: string; ctrlKey: boolean; shiftKey: boolean; metaKey?: boolean; isComposing?: boolean }, enterMode: EnterMode): boolean {
  if (e.key !== "Enter" || e.isComposing) return false;
  if (e.ctrlKey || e.metaKey) return true;
  return enterMode === "enter" && !e.shiftKey;
}
